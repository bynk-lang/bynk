//! #3562 (G5): a context that hosts agents must not change scripts unannounced.
//!
//! A Durable Object class's state belongs to its Worker's script. A context that moves to
//! another script (into or out of a `[deploy.workers.<key>]` group, between groups, or with
//! its group renamed or re-keyed) redeclares its classes in the new script, where Cloudflare
//! creates empty namespaces and says nothing: the data stays behind in the old script. So
//! `bynk deploy` refuses the move before it pushes anything, unless the context is named in
//! `--accept-fresh-agent`, which starts it fresh and records that the old script still holds
//! the data. (A supported, state-preserving move is future work, ADR 0874.)

use std::collections::BTreeMap;

use super::config::Resources;
use super::ledger::DeployLock;
use super::plan::{DeployOptions, GroupNames};

/// One context with agent state that the plan would put on another script.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentMove {
    pub(crate) context: String,
    pub(crate) classes: Vec<String>,
    pub(crate) from_worker: String,
    pub(crate) from_script: String,
    pub(crate) to_worker: String,
    pub(crate) to_script: String,
    /// The old script is still a Worker of this build (a group that lost the member), so
    /// its classes would have to be dropped from a Worker that keeps running.
    pub(crate) old_script_built: bool,
    /// The same Worker (key) now has another script (a group renamed in place): Cloudflare cannot
    /// rename a script, and the ledger would keep no record of the script left holding the data.
    pub(crate) renamed_in_place: bool,
}

/// The moves of the Workers this run deploys (`order`): each context that hosts agents whose
/// ledger record names a different script than the one it is planned onto.
pub(crate) fn agent_moves(
    lock: &DeployLock,
    environment: &str,
    resources: &BTreeMap<String, Resources>,
    order: &[String],
    groups: &GroupNames,
) -> Vec<AgentMove> {
    let built_scripts: Vec<String> = resources.keys().map(|w| groups.script(w)).collect();
    let mut moves = Vec::new();
    for worker in order {
        let to_script = groups.script(worker);
        for (context, classes) in &resources[worker].agent_contexts {
            let from_worker = lock.worker_of(environment, context);
            if !lock.is_deployed(environment, &from_worker) {
                continue;
            }
            let from_script = lock.script_of(environment, &from_worker);
            if from_script == to_script {
                continue;
            }
            moves.push(AgentMove {
                context: context.clone(),
                classes: classes.clone(),
                old_script_built: built_scripts.contains(&from_script),
                renamed_in_place: from_worker == *worker,
                from_worker,
                from_script,
                to_worker: worker.clone(),
                to_script: to_script.clone(),
            });
        }
    }
    moves
}

/// Why a move is refused: it is not named in `accepted`, or it is but cannot be accepted.
pub(crate) fn refusals(moves: &[AgentMove], accepted: &[String]) -> Vec<String> {
    let named = |m: &AgentMove| {
        accepted
            .iter()
            .any(|a| a == &m.context || a.replace('-', ".") == m.context)
    };
    moves
        .iter()
        .filter_map(|m| {
            let what = format!(
                "bynk.deploy.agent_move_refused: context `{}` hosts {} and would move from Worker script `{}` to `{}`; an agent's state belongs to its script, so it would start empty there and the old data would stay behind",
                m.context,
                classes_phrase(&m.classes),
                m.from_script,
                m.to_script
            );
            if !named(m) {
                Some(format!(
                    "{what}.\n  Keep `{}` on `{}`, or, if its state may be discarded, start it fresh with `--accept-fresh-agent {}`.",
                    m.context, m.from_script, m.context
                ))
            } else if m.renamed_in_place {
                Some(format!(
                    "{what}, and the Worker is renamed in place: its old script would keep the data with nothing in the ledger to say so, so renaming a group that hosts agents is not supported (not even with `--accept-fresh-agent`); change its key as well, or keep its `name`."
                ))
            } else if m.old_script_built {
                Some(format!(
                    "{what}, and `{}` keeps running without it: Cloudflare cannot drop a class from a Worker that stays, so this move is not supported (not even with `--accept-fresh-agent`).",
                    m.from_script
                ))
            } else {
                None
            }
        })
        .collect()
}

/// `--accept-fresh-agent` names that match no move of this run (a typo, or a move already
/// done): the flag is never a blanket, so a stray name is an error.
pub(crate) fn unused_accepts(moves: &[AgentMove], accepted: &[String]) -> Vec<String> {
    accepted
        .iter()
        .filter(|a| {
            !moves
                .iter()
                .any(|m| *a == &m.context || a.replace('-', ".") == m.context)
        })
        .cloned()
        .collect()
}

/// The warning for an accepted move: the class and the old script that keeps the data.
pub(crate) fn fresh_warning(m: &AgentMove) -> String {
    format!(
        "bynk: warning: `{}` starts with fresh {} on `{}`; the old state stays in script `{}`, which is no longer a Worker of this project: do not delete it if the data matters.\n",
        m.context,
        classes_phrase(&m.classes),
        m.to_script,
        m.from_script
    )
}

/// The orphan report's line for a script that holds agent state (context → classes).
pub(crate) fn agent_state_note(worker: &str, state: &BTreeMap<String, Vec<String>>) -> String {
    let held = state
        .iter()
        .map(|(context, classes)| format!("{} of `{context}`", classes.join(", ")))
        .collect::<Vec<_>>()
        .join("; ");
    format!(
        "  worker {worker} still holds agent state ({held}): it is not empty, do not delete it\n"
    )
}

fn classes_phrase(classes: &[String]) -> String {
    match classes {
        [one] => format!("agent `{one}`"),
        many => format!(
            "agents {}",
            many.iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Record, once `worker` has been pushed, that each accepted move onto it left the data on the
/// old script.
pub(crate) fn record_fresh_moves(
    lock: &mut DeployLock,
    opts: &DeployOptions,
    worker: &str,
    moves: &[AgentMove],
) {
    let accepted = &opts.accept_fresh_agent;
    for m in moves.iter().filter(|m| {
        m.to_worker == worker
            && accepted
                .iter()
                .any(|a| a == &m.context || a.replace('-', ".") == m.context)
    }) {
        lock.record_fresh_agents(&opts.environment, &m.from_worker, &m.context, &m.classes);
    }
}

/// The moves of this run, if it may go on: [`gate`] over [`agent_moves`], printing what it must.
pub(crate) fn checked_agent_moves(
    lock: &DeployLock,
    opts: &DeployOptions,
    resources: &BTreeMap<String, Resources>,
    order: &[String],
    groups: &GroupNames,
) -> Option<Vec<AgentMove>> {
    let moves = agent_moves(lock, &opts.environment, resources, order, groups);
    gate(&moves, &opts.accept_fresh_agent).then_some(moves)
}

/// Print what the moves of this run need said, and whether the run may go on: every refusal and
/// every `--accept-fresh-agent` name that matches no move stops it; an accepted move warns.
pub(crate) fn gate(moves: &[AgentMove], accepted: &[String]) -> bool {
    let refused = refusals(moves, accepted);
    let unused = unused_accepts(moves, accepted);
    for why in &refused {
        eprintln!("bynk: {why}");
    }
    for name in &unused {
        eprintln!(
            "bynk: `--accept-fresh-agent {name}` names no context that would move with agent state in this run"
        );
    }
    if !refused.is_empty() || !unused.is_empty() {
        return false;
    }
    for m in moves {
        eprint!("{}", fresh_warning(m));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::deploy::ledger::find_orphans;

    fn names(manifest: &str) -> GroupNames {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "bynk-agent-move-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("bynk.toml"),
            format!("[project]\nname = \"p\"\nversion = \"0.1.0\"\n{manifest}"),
        )
        .unwrap();
        let groups = GroupNames::of(&root);
        let _ = std::fs::remove_dir_all(&root);
        groups
    }

    /// A Worker of the build hosting `agents` (context, class) pairs.
    fn worker(agents: &[(&str, &str)]) -> Resources {
        let mut r = Resources::default();
        for (context, class) in agents {
            r.agent_contexts
                .entry(context.to_string())
                .or_default()
                .push(class.to_string());
        }
        r
    }

    fn lock() -> DeployLock {
        toml::from_str("version = 1\n").unwrap()
    }

    fn deployed(lock: &mut DeployLock, worker: &str, members: &[&str], script: Option<&str>) {
        lock.record_deployed("default", worker, Some(BTreeMap::new()));
        let members: Vec<String> = members.iter().map(|m| m.to_string()).collect();
        lock.record_members("default", worker, &members);
        if let Some(script) = script {
            lock.record_script("default", worker, script);
        }
    }

    fn moves(
        lock: &DeployLock,
        groups: &GroupNames,
        build: &[(&str, Resources)],
    ) -> Vec<AgentMove> {
        let resources: BTreeMap<String, Resources> = build
            .iter()
            .map(|(w, r)| {
                (
                    w.to_string(),
                    Resources {
                        agent_contexts: r.agent_contexts.clone(),
                        ..Resources::default()
                    },
                )
            })
            .collect();
        let order: Vec<String> = build.iter().map(|(w, _)| w.to_string()).collect();
        agent_moves(lock, "default", &resources, &order, groups)
    }

    const CORE: &str = "\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pay\"]\n";

    /// A standalone context joining a group: its ledger record is under its own dashed name
    /// (also all an older ledger has), and the group is another script.
    #[test]
    fn a_context_moving_into_a_group_is_refused() {
        let mut l = lock();
        deployed(&mut l, "shop-orders", &[], None);
        let m = moves(
            &l,
            &names(CORE),
            &[("core", worker(&[("shop.orders", "Cart")]))],
        );
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(
            (m[0].from_script.as_str(), m[0].to_script.as_str()),
            ("shop-orders", "core")
        );
        assert!(!m[0].old_script_built);
        let why = refusals(&m, &[]);
        assert!(
            why[0].contains("bynk.deploy.agent_move_refused") && why[0].contains("`Cart`"),
            "{why:?}"
        );
        assert!(
            why[0].contains("--accept-fresh-agent shop.orders"),
            "{why:?}"
        );
    }

    /// A member leaving a group that stays: even `--accept-fresh-agent` cannot do it.
    #[test]
    fn a_context_moving_out_of_a_group_that_stays_is_never_accepted() {
        let mut l = lock();
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], None);
        let build = [
            ("core", worker(&[])),
            ("shop-orders", worker(&[("shop.orders", "Cart")])),
        ];
        let m = moves(
            &l,
            &names("\n[deploy.workers.core]\ncontexts = [\"shop.pay\", \"shop.audit\"]\n"),
            &build,
        );
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(m[0].old_script_built);
        let accepted = vec!["shop.orders".to_string()];
        let why = refusals(&m, &accepted);
        assert!(why[0].contains("not supported"), "{why:?}");
    }

    #[test]
    fn a_context_moving_between_groups_is_refused() {
        let mut l = lock();
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], None);
        let groups = names(
            "\n[deploy.workers.hub]\ncontexts = [\"shop.orders\"]\n\n[deploy.workers.core]\ncontexts = [\"shop.pay\"]\n",
        );
        let build = [
            ("hub", worker(&[("shop.orders", "Cart")])),
            ("core", worker(&[])),
        ];
        let m = moves(&l, &groups, &build);
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(
            (m[0].from_worker.as_str(), m[0].to_worker.as_str()),
            ("core", "hub")
        );
    }

    /// A rename (`name =`) is a new script for every member; a key change with no `name` too.
    #[test]
    fn renaming_or_rekeying_a_group_moves_its_members_agents() {
        let build_core = [("core", worker(&[("shop.orders", "Cart")]))];
        let mut l = lock();
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], None);
        let renamed = names(
            "\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pay\"]\nname = \"core-v2\"\n",
        );
        let m = moves(&l, &renamed, &build_core);
        assert_eq!(
            (m[0].from_script.as_str(), m[0].to_script.as_str()),
            ("core", "core-v2")
        );
        let rekeyed = names("\n[deploy.workers.hub]\ncontexts = [\"shop.orders\", \"shop.pay\"]\n");
        let m = moves(&l, &rekeyed, &[("hub", worker(&[("shop.orders", "Cart")]))]);
        assert_eq!(
            (m[0].from_script.as_str(), m[0].to_script.as_str()),
            ("core", "hub")
        );
    }

    /// A key change that keeps the script (`name` fixed) is no move: the state stays put.
    #[test]
    fn rekeying_a_group_that_keeps_its_script_is_no_move() {
        let mut l = lock();
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], Some("shared"));
        let groups = names(
            "\n[deploy.workers.hub]\ncontexts = [\"shop.orders\", \"shop.pay\"]\nname = \"shared\"\n",
        );
        let m = moves(&l, &groups, &[("hub", worker(&[("shop.orders", "Cart")]))]);
        assert!(m.is_empty(), "{m:?}");
    }

    #[test]
    fn a_first_deploy_an_agentless_context_and_an_unmoved_one_are_no_move() {
        let groups = names(CORE);
        // No ledger record: nothing to move from.
        assert!(
            moves(
                &lock(),
                &groups,
                &[("core", worker(&[("shop.orders", "Cart")]))]
            )
            .is_empty()
        );
        // The same group as recorded, and a context with no agents moving.
        let mut l = lock();
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], None);
        assert!(moves(&l, &groups, &[("core", worker(&[("shop.orders", "Cart")]))]).is_empty());
        deployed(&mut l, "shop-pay", &[], None);
        assert!(moves(&l, &groups, &[("core", worker(&[]))]).is_empty());
    }

    /// `--accept-fresh-agent`: the move goes ahead with a warning, the old script is recorded as
    /// holding the data and the orphan report says so; a name that matches no move is an error.
    #[test]
    fn an_accepted_move_records_that_the_old_script_holds_the_data() {
        let mut l = lock();
        deployed(&mut l, "shop-orders", &[], None);
        let build = [("core", worker(&[("shop.orders", "Cart")]))];
        let m = moves(&l, &names(CORE), &build);
        let accepted = vec!["shop.orders".to_string()];
        assert!(refusals(&m, &accepted).is_empty());
        assert!(unused_accepts(&m, &accepted).is_empty());
        assert_eq!(
            unused_accepts(&m, &["shop.nope".to_string()]),
            ["shop.nope"]
        );
        assert!(fresh_warning(&m[0]).contains("`shop-orders`"));
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], None);
        record_fresh_moves(
            &mut l,
            &DeployOptions {
                environment: "default".into(),
                accept_fresh_agent: accepted.clone(),
                ..DeployOptions::default()
            },
            "core",
            &m,
        );
        let resources: BTreeMap<String, Resources> =
            BTreeMap::from([("core".to_string(), Resources::default())]);
        let orphans = find_orphans(&l, "default", &resources);
        assert_eq!(orphans.workers, ["shop-orders"]);
        let state = &orphans.worker_agent_state["shop-orders"];
        assert_eq!(state["shop.orders"], ["Cart"]);
        assert!(agent_state_note("shop-orders", state).contains("do not delete"));
    }

    /// After an accepted move the ledger agrees with the build: the same deploy again is no
    /// move, even though the old group's record sorts first. The old record keeps no member that
    /// is now elsewhere, and keeps the note of the data it holds.
    #[test]
    fn an_accepted_move_out_of_a_dissolved_group_is_not_refused_again() {
        let mut l = lock();
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], None);
        let groups = names("");
        let build = [("shop-orders", worker(&[("shop.orders", "Cart")]))];
        let m = moves(&l, &groups, &build);
        assert_eq!(m.len(), 1, "{m:?}");
        assert!(!m[0].old_script_built && !m[0].renamed_in_place);
        let opts = DeployOptions {
            environment: "default".into(),
            accept_fresh_agent: vec!["shop.orders".to_string()],
            ..DeployOptions::default()
        };
        assert!(refusals(&m, &opts.accept_fresh_agent).is_empty());
        // The push: the context's own Worker is recorded, then the move.
        deployed(&mut l, "shop-orders", &[], None);
        record_fresh_moves(&mut l, &opts, "shop-orders", &m);
        assert!(moves(&l, &groups, &build).is_empty(), "refused again");
        let resources: BTreeMap<String, Resources> =
            BTreeMap::from([("shop-orders".to_string(), Resources::default())]);
        let orphans = find_orphans(&l, "default", &resources);
        assert_eq!(orphans.worker_agent_state["core"]["shop.orders"], ["Cart"]);
        // `shop.pay`, still listed by the dissolved group, is unaffected.
        assert_eq!(orphans.worker_contexts["core"], ["shop.pay"]);
    }

    /// A move between groups, the old key sorting first, is not refused again either.
    #[test]
    fn an_accepted_move_between_groups_is_not_refused_again() {
        let mut l = lock();
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], None);
        let groups = names(
            "\n[deploy.workers.hub]\ncontexts = [\"shop.orders\"]\n\n[deploy.workers.core]\ncontexts = [\"shop.pay\"]\n",
        );
        let build = [
            ("hub", worker(&[("shop.orders", "Cart")])),
            ("core", worker(&[])),
        ];
        let m = moves(&l, &groups, &build);
        let opts = DeployOptions {
            environment: "default".into(),
            accept_fresh_agent: vec!["shop.orders".to_string()],
            ..DeployOptions::default()
        };
        // Both Workers are pushed: the group the context left is built, so this is the old-script-built case.
        assert!(m[0].old_script_built);
        deployed(&mut l, "hub", &["shop.orders"], None);
        deployed(&mut l, "core", &["shop.pay"], None);
        assert!(moves(&l, &groups, &build).is_empty());
        let _ = opts;
    }

    /// A group renamed in place (`name =` under the same key) is refused even with the flag: its
    /// old script would hold the data with no record, and the live record would replace it.
    #[test]
    fn renaming_a_group_in_place_is_never_accepted() {
        let mut l = lock();
        deployed(&mut l, "core", &["shop.orders", "shop.pay"], None);
        let renamed = names(
            "\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pay\"]\nname = \"core-v2\"\n",
        );
        let m = moves(
            &l,
            &renamed,
            &[("core", worker(&[("shop.orders", "Cart")]))],
        );
        assert!(m[0].renamed_in_place);
        let why = refusals(&m, &["shop.orders".to_string()]);
        assert!(
            why[0].contains("renamed in place") && why[0].contains("not supported"),
            "{why:?}"
        );
    }
}
