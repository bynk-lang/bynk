//! #3562 (G5): a context that hosts agents must not change scripts unannounced.
//!
//! A Durable Object class's state belongs to its Worker's script. A context that moves to
//! another script (into or out of a `[deploy.workers.<key>]` group, between groups, or with
//! its group renamed or re-keyed) redeclares its classes in the new script, where Cloudflare
//! creates empty namespaces and says nothing: the data stays behind in the old script. So
//! `bynk deploy` refuses the move before it pushes anything, unless the context is named in
//! `--accept-fresh-agent`, which starts it fresh and records that the old script still holds
//! the data. (A supported, state-preserving move is future work, ADR 0874.)

use bynk_driver::colour;
use bynk_driver::emsg;
use std::collections::BTreeMap;

use super::config::Resources;
use super::ledger::DeployLock;
use super::move_sets::{PlannedSet, move_sets, place};
use super::plan::{DeployOptions, Entry, GroupNames};

/// The refusal of an agent move (#3562) and the codes the JSON plan gives its siblings (#3797).
pub(crate) const AGENT_MOVE_REFUSED: &str = "bynk.deploy.agent_move_refused";
const AGENT_STARTS_FRESH: &str = "bynk.deploy.agent_starts_fresh";
const ACCEPT_FRESH_AGENT_UNUSED: &str = "bynk.deploy.accept_fresh_agent_unused";
/// #3565: a `--move-agent` name that matches no move of the run, and the window warning of a
/// planned move set.
const MOVE_AGENT_UNUSED: &str = "bynk.deploy.move_agent_unused";
const AGENT_MOVE_WINDOW: &str = "bynk.deploy.agent_move_window";

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
    /// #3565: the old classes of the class tombstones the context carries (a `replaces`, a
    /// `removed agent`). A supported move of such a context is refused in v1 (`tombstones`).
    pub(crate) tombstones: Vec<String>,
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
                tombstones: resources[worker]
                    .agent_tombstones
                    .get(context)
                    .cloned()
                    .unwrap_or_default(),
                from_worker,
                from_script,
                to_worker: worker.clone(),
                to_script: to_script.clone(),
            });
        }
    }
    moves
}

/// Whether `--accept-fresh-agent` names this move's context (a `-` stands for a `.`).
fn is_named(context: &str, accepted: &[String]) -> bool {
    accepted
        .iter()
        .any(|a| a == context || a.replace('-', ".") == context)
}

/// Why a move is refused: it is not named in `accepted`, or it is but cannot be accepted.
///
/// #3797: each as an entry of the JSON plan's `refusals` (`reason` says which way it is refused;
/// only a move not yet named carries the flag that would let it through).
#[cfg(test)]
pub(crate) fn refusals(moves: &[AgentMove], accepted: &[String]) -> Vec<Entry> {
    move_refusals(moves, accepted, &[])
}

/// [`refusals`], with the contexts `--move-agent` names (#3565): a move named there is refused by
/// its set ([`set_refusals`]), unless `--accept-fresh-agent` names it too (`both`); a move named
/// in neither is refused as before, with `--move-agent` as the flag that lets it through.
fn move_refusals(moves: &[AgentMove], accepted: &[String], moving: &[String]) -> Vec<Entry> {
    let named = |m: &AgentMove| is_named(&m.context, accepted);
    let moved = |m: &AgentMove| is_named(&m.context, moving);
    moves
        .iter()
        .filter_map(|m| {
            if moved(m) && !named(m) {
                return None;
            }
            let what = format!(
                "context `{}` hosts {} and would move from Worker script `{}` to `{}`; an agent's state belongs to its script, so it would start empty there and the old data would stay behind",
                m.context,
                classes_phrase(&m.classes),
                m.from_script,
                m.to_script
            );
            let (reason, message) = if moved(m) {
                (
                    "both",
                    format!(
                        "{what}, and both `--move-agent {c}` and `--accept-fresh-agent {c}` name it: a move keeps the state and a fresh start discards it, so name one.",
                        c = m.context
                    ),
                )
            } else if !named(m) {
                (
                    "not_accepted",
                    format!(
                        "{what}.\n  Keep `{}` on `{}`, move it with its state with `--move-agent {}`, or, if its state may be discarded, start it fresh with `--accept-fresh-agent {}`.",
                        m.context, m.from_script, m.context, m.context
                    ),
                )
            } else if m.renamed_in_place {
                (
                    "renamed_in_place",
                    format!(
                        "{what}, and the Worker is renamed in place: its old script would keep the data with nothing in the ledger to say so, so renaming a group that hosts agents is not supported (not even with `--accept-fresh-agent`); change its key as well, or keep its `name`."
                    ),
                )
            } else if m.old_script_built {
                (
                    "old_script_stays",
                    format!(
                        "{what}, and `{}` keeps running without it: Cloudflare cannot drop a class from a Worker that stays, so this move is not supported (not even with `--accept-fresh-agent`).",
                        m.from_script
                    ),
                )
            } else {
                return None;
            };
            let entry = Entry::new(AGENT_MOVE_REFUSED, message)
                .with("reason", reason)
                .with("context", m.context.as_str())
                .with("agents", m.classes.clone())
                .with("from", m.from_script.as_str())
                .with("to", m.to_script.as_str());
            Some(if reason == "not_accepted" {
                entry.overridden_by(format!("--move-agent {}", m.context))
            } else {
                entry
            })
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
                .any(|m| is_named(&m.context, std::slice::from_ref(*a)))
        })
        .cloned()
        .collect()
}

/// The warning for an accepted move: the class and the old script that keeps the data.
pub(crate) fn fresh_warning(m: &AgentMove) -> Entry {
    Entry::new(
        AGENT_STARTS_FRESH,
        format!(
            "`{}` starts with fresh {} on `{}`; the old state stays in script `{}`, which is no longer a Worker of this project: do not delete it if the data matters.",
            m.context,
            classes_phrase(&m.classes),
            m.to_script,
            m.from_script
        ),
    )
    .with("context", m.context.as_str())
    .with("agents", m.classes.clone())
    .with("from", m.from_script.as_str())
    .with("to", m.to_script.as_str())
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
    for m in moves
        .iter()
        .filter(|m| m.to_worker == worker && is_named(&m.context, accepted))
    {
        lock.record_fresh_agents(&opts.environment, &m.from_worker, &m.context, &m.classes);
    }
}

/// What the agent-move check of a run found (#3562, #3797): the moves, and the refusals and
/// warnings it has to say. #3565: the move sets `--move-agent` names, as the plan lists them,
/// when the run plans them.
#[derive(Debug)]
pub(crate) struct AgentGate {
    pub(crate) moves: Vec<AgentMove>,
    pub(crate) refusals: Vec<Entry>,
    pub(crate) warnings: Vec<Entry>,
    pub(crate) planned: Vec<PlannedSet>,
}

impl AgentGate {
    /// Every refusal and every `--accept-fresh-agent` name that matches no move stops the run;
    /// an accepted move warns.
    pub(crate) fn may_go_on(&self) -> bool {
        self.refusals.is_empty()
    }

    /// The stderr lines, as a run has always printed them: each refusal, and the warnings only
    /// when the run goes on.
    pub(crate) fn eprint(&self) {
        for refusal in &self.refusals {
            emsg!("bynk: error: {}", refusal.line());
        }
        if self.may_go_on() {
            for warning in &self.warnings {
                colour::eprint(&format!("bynk: warning: {}\n", warning.message));
            }
        }
    }
}

/// What a run knows that the move sets are checked against (#3565).
pub(crate) struct MoveCx<'a> {
    pub(crate) resources: &'a BTreeMap<String, Resources>,
    pub(crate) order: &'a [String],
    /// Each Worker of the build that declares `routes` or `custom_domains`, with the key it
    /// declares (D12: a move of such a Worker's set is refused in v1).
    pub(crate) attached: &'a BTreeMap<String, &'static str>,
    /// A `--worker` or `--context` run, which deploys only part of the project.
    pub(crate) partial: bool,
    pub(crate) dry_run: bool,
    /// Each Worker's script, for the plan's names.
    pub(crate) script: &'a dyn Fn(&str) -> String,
}

/// The moves of this run, and what they need said: [`gate`] over [`agent_moves`].
pub(crate) fn checked_agent_moves(
    lock: &DeployLock,
    opts: &DeployOptions,
    resources: &BTreeMap<String, Resources>,
    order: &[String],
    groups: &GroupNames,
    attached: &BTreeMap<String, &'static str>,
) -> AgentGate {
    let moves = agent_moves(lock, &opts.environment, resources, order, groups);
    let script = |w: &str| groups.script(w);
    let cx = MoveCx {
        resources,
        order,
        attached,
        partial: opts.context.is_some(),
        dry_run: opts.dry_run,
        script: &script,
    };
    gate(moves, &opts.accept_fresh_agent, &opts.move_agent, &cx)
}

/// What the moves of this run need said: every refusal, every `--accept-fresh-agent` or
/// `--move-agent` name that matches no move (an error, as neither flag is ever a blanket), a
/// warning for each accepted move, and, for the move sets `--move-agent` names, their refusals
/// or, when none, their plan and the window's warning.
pub(crate) fn gate(
    moves: Vec<AgentMove>,
    accepted: &[String],
    moving: &[String],
    cx: &MoveCx<'_>,
) -> AgentGate {
    let mut refusals = move_refusals(&moves, accepted, moving);
    for name in unused_accepts(&moves, accepted) {
        refusals.push(
            Entry::new(
                ACCEPT_FRESH_AGENT_UNUSED,
                format!(
                    "`--accept-fresh-agent {name}` names no context that would move with agent state in this run"
                ),
            )
            .with("name", name),
        );
    }
    for name in unused_accepts(&moves, moving) {
        refusals.push(
            Entry::new(
                MOVE_AGENT_UNUSED,
                format!(
                    "`--move-agent {name}` names no context that would move with agent state in this run"
                ),
            )
            .with("name", name),
        );
    }
    let moved: Vec<AgentMove> = moves
        .iter()
        .filter(|m| is_named(&m.context, moving) && !is_named(&m.context, accepted))
        .cloned()
        .collect();
    let (set_refused, planned) = plan_sets(&moved, cx);
    refusals.extend(set_refused);
    // Warnings say what a run goes ahead with, so a refused run has none: its refusals are the
    // whole story, as on stderr. (A move that is refused is said by its refusal, not as though it
    // were happening.)
    if !refusals.is_empty() {
        return AgentGate {
            moves,
            refusals,
            warnings: Vec::new(),
            planned: Vec::new(),
        };
    }
    let mut warnings: Vec<Entry> = moves
        .iter()
        .filter(|m| is_named(&m.context, accepted))
        .map(fresh_warning)
        .collect();
    warnings.extend(planned.iter().map(window_warning));
    AgentGate {
        moves,
        refusals,
        warnings,
        planned,
    }
}

/// The move sets of `moved` (the moves `--move-agent` names), checked: each set's refusals; or,
/// when it has none, the sets as the plan lists them, placed in the order (§3.2). Outside
/// `--dry-run` a supported set is still refused (`apply_not_yet`): this release plans moves, it
/// does not apply them.
fn plan_sets(moved: &[AgentMove], cx: &MoveCx<'_>) -> (Vec<Entry>, Vec<PlannedSet>) {
    if moved.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let sets = move_sets(moved);
    let mut refused = Vec::new();
    for set in &sets {
        refused.extend(set_refusals(set, cx));
    }
    if !refused.is_empty() {
        return (refused, Vec::new());
    }
    let placement = match place(&sets, cx.order, cx.resources) {
        Ok(placement) => placement,
        Err(cycle) => {
            let entry = Entry::new(
                AGENT_MOVE_REFUSED,
                format!(
                    "the agent moves of this run cannot be placed in the upload order: {}. A move set's receivers are pushed first, then the Workers that call them, then its sources and its receivers again, and this run's bindings need another order; split the move across two deploys.",
                    cycle.join(" → ")
                ),
            )
            .with("reason", "order")
            .with("cycle", cycle);
            return (vec![entry], Vec::new());
        }
    };
    let planned: Vec<PlannedSet> = sets
        .iter()
        .zip(&placement.callers)
        .map(|(set, callers)| PlannedSet::of(set, callers, cx.script))
        .collect();
    if !cx.dry_run {
        let refused = planned
            .iter()
            .map(|p| {
                let contexts: Vec<String> = p.moves.iter().map(|m| m.context.clone()).collect();
                Entry::new(
                    AGENT_MOVE_REFUSED,
                    format!(
                        "{} would move with {} state; this release plans such a move but does not apply it yet. See the plan with `--dry-run`; applying it comes with a later release.",
                        contexts_phrase(&contexts),
                        if contexts.len() == 1 { "its" } else { "their" }
                    ),
                )
                .with("reason", "apply_not_yet")
                .with("contexts", contexts)
            })
            .collect();
        return (refused, Vec::new());
    }
    (Vec::new(), planned)
}

/// Why one move set `--move-agent` names cannot be moved (D9, D12, the tombstones ruling).
fn set_refusals(set: &super::move_sets::MoveSet, cx: &MoveCx<'_>) -> Vec<Entry> {
    let mut refused = Vec::new();
    let entry = |reason: &str, message: String, m: &AgentMove| {
        Entry::new(AGENT_MOVE_REFUSED, message)
            .with("reason", reason)
            .with("context", m.context.as_str())
            .with("agents", m.classes.clone())
            .with("from", m.from_script.as_str())
            .with("to", m.to_script.as_str())
    };
    for m in &set.moves {
        if m.renamed_in_place {
            refused.push(entry(
                "renamed_in_place",
                format!(
                    "context `{}` would move from Worker script `{}` to `{}` because its Worker is renamed in place: the old script would keep no ledger record once the move closed, so renaming a group that hosts agents is not supported; change its key as well, or keep its `name`.",
                    m.context, m.from_script, m.to_script
                ),
                m,
            ));
        }
        if !m.tombstones.is_empty() {
            refused.push(
                entry(
                    "tombstones",
                    format!(
                        "context `{}` carries class tombstones ({}): where they belong once its agents move from `{}` to `{}` is not settled, so moving a context that carries them is not supported yet. Deploy the rename or removal where the context is first, then move it once the tombstones are gone.",
                        m.context,
                        m.tombstones
                            .iter()
                            .map(|t| format!("`{t}`"))
                            .collect::<Vec<_>>()
                            .join(", "),
                        m.from_script,
                        m.to_script
                    ),
                    m,
                )
                .with("tombstones", m.tombstones.clone()),
            );
        }
    }
    let contexts: Vec<String> = set.moves.iter().map(|m| m.context.clone()).collect();
    let set_entry = |reason: &str, message: String| {
        Entry::new(AGENT_MOVE_REFUSED, message)
            .with("reason", reason)
            .with("contexts", contexts.clone())
    };
    if let Some(script) = set.chained() {
        refused.push(
            set_entry(
                "chained",
                format!(
                    "Worker script `{script}` would both give up and receive agent state in this run ({}); a Worker cannot be a source and a receiver of one move. Give one of the groups a new key, or make the change in two deploys.",
                    contexts_phrase(&contexts)
                ),
            )
            .with("script", script),
        );
    }
    let workers = set.built_workers(cx.resources);
    if cx.partial {
        // The set's own Workers, and every Worker that calls a receiver: each is rebound before the
        // commit, and one this run leaves out would stay bound to the source after it.
        let receivers = set.receivers();
        let callers = cx.resources.iter().filter_map(|(w, r)| {
            (!workers.contains(w) && r.binds_to.iter().any(|t| receivers.contains(t))).then_some(w)
        });
        let left_out: Vec<String> = workers
            .iter()
            .chain(callers)
            .filter(|w| !cx.order.contains(w))
            .map(|w| (cx.script)(w))
            .collect();
        if !left_out.is_empty() {
            refused.push(
                set_entry(
                    "partial_selection",
                    format!(
                        "the move of {} pushes {}, which this `--worker`/`--context` run leaves out; a move set is pushed as one, with every Worker that calls its receivers, so run the whole project.",
                        contexts_phrase(&contexts),
                        left_out
                            .iter()
                            .map(|w| format!("`{w}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .with("workers", left_out),
            );
        }
    }
    for w in &workers {
        if let Some(key) = cx.attached.get(w) {
            refused.push(
                set_entry(
                    "routes",
                    format!(
                        "Worker `{}` declares `{key}` in `[deploy.workers.{w}]`, and moving {} with its routes or custom domains is not supported yet: a domain attaches to one Worker at a time, and this release cannot hand it over behind the move.",
                        (cx.script)(w),
                        contexts_phrase(&contexts)
                    ),
                )
                .with("worker", (cx.script)(w))
                .with("key", *key),
            );
        }
    }
    refused
}

/// The window warning of one planned move set (D11).
fn window_warning(set: &PlannedSet) -> Entry {
    let contexts: Vec<String> = set.moves.iter().map(|m| m.context.clone()).collect();
    Entry::new(
        AGENT_MOVE_WINDOW,
        format!(
            "moving {} keeps its agents' state, but from the commit on {} until {} goes live again, calls to the moving contexts fail: a window of about one push (about 4 seconds measured).",
            contexts_phrase(&contexts),
            set.commit.join(", "),
            set.go_live.join(", ")
        ),
    )
    .with("contexts", contexts)
    .with("commit", set.commit.clone())
    .with("go_live", set.go_live.clone())
}

fn contexts_phrase(contexts: &[String]) -> String {
    match contexts {
        [one] => format!("context `{one}`"),
        many => format!(
            "contexts {}",
            many.iter()
                .map(|c| format!("`{c}`"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
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
            why[0].line().contains("bynk.deploy.agent_move_refused")
                && why[0].line().contains("`Cart`"),
            "{why:?}"
        );
        assert!(
            why[0].line().contains("--accept-fresh-agent shop.orders"),
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
        assert!(why[0].line().contains("not supported"), "{why:?}");
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
        assert!(fresh_warning(&m[0]).message.contains("`shop-orders`"));
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
            why[0].line().contains("renamed in place") && why[0].line().contains("not supported"),
            "{why:?}"
        );
    }
}
