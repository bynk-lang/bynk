//! #3565 (agent-context moves, slice 4a): applying and resuming a move set
//! (`design/tracks/agent-context-moves.md` §3.2–§3.4, §7.1, §7.2).
//!
//! A set is opened ([`MoveRun::open`]) by reading the account and writing its record, then moved
//! in its run's order as two nodes: [`MoveRun::receive`] (phase 1) and [`MoveRun::commit`]
//! (phase 2, the commit confirmation, then phase 3). Each node reads the record and carries on
//! from the step it holds, so a re-run resumes the set where it stopped. Within a phase every
//! push starts at once, and each is recorded `done`, through the run's one [`LedgerWriter`], as
//! it lands. A Worker not recorded `done` is always pushed again, never skipped: a push cut off
//! part-way may have landed with its triggers unset (S3).
//!
//! **Phase 3 runs only after a confirmed commit** (S6): Cloudflare accepts a phase 3 pushed
//! before the commit, creates an empty namespace and drops the pending transfer. So before the
//! record says `committed`, and again immediately before the phase 3 pushes, the namespace listing
//! must show each moved class's namespace with the id recorded before phase 1 **and** on its
//! receiving script ([`MoveRun::unconfirmed`]); otherwise the run stops with
//! [`MOVE_COMMIT_UNCONFIRMED`] and the next run repeats the unconfirmed sources' commit.
//!
//! The ledger only says where the run had got to; the listing says where the data is (ADR 0194
//! D1). The pushes themselves are the caller's ([`MovePusher`]), and so is what the ledger records
//! of each pushed Worker beside `done`.

// 4a: the engine and its tests. Slice 4b wires it into the run with the wrangler-backed pusher,
// when the `apply_not_yet` refusal is lifted; until then only the tests reach it.
#![cfg_attr(not(test), allow(dead_code))]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::mpsc;

use super::agent_move::AGENT_MOVE_REFUSED;
use super::apply::LedgerWriter;
use super::facts::{Facts, Transfer};
use super::ledger::{AgentMoveEntry, AgentMoveRecord, DeployLock, MoveStep};
use super::plan::Entry;
use super::provisioning::DeployFailure;
use super::revert::Deployment;
use crate::cloudflare::CloudflareError;
use crate::migrate::sweep::Namespace;

/// Phase 3 would run, but the listing does not show the commit (S6).
pub(crate) const MOVE_COMMIT_UNCONFIRMED: &str = "bynk.deploy.move_commit_unconfirmed";
/// A commit was refused because the receiver's pending transfer is gone.
pub(crate) const MOVE_TRANSFER_LOST: &str = "bynk.deploy.move_transfer_lost";
/// A Worker of the set was deployed by something other than this move (§7.2).
pub(crate) const MOVE_INTERLEAVED: &str = "bynk.deploy.move_interleaved";

/// The record's word for a receiver that had no deployment before the move.
pub(crate) const NO_DEPLOYMENT: &str = "none";

/// What a move reads of the account (D7): the Durable Object namespaces, and each Worker's
/// deployments with the facts of a version.
pub(crate) trait MoveAccount {
    /// Every Durable Object namespace on the account.
    fn namespaces(&self) -> Result<Vec<Namespace>, CloudflareError>;
    /// The Worker's deployments, newest first. A Worker that is not on the account has none.
    fn deployments(&self, script: &str) -> Result<Vec<Deployment>, CloudflareError>;
    /// The text of a version's deploy facts, `None` when it has none.
    fn version_facts(&self, script: &str, version: &str)
    -> Result<Option<String>, CloudflareError>;
}

impl MoveAccount for super::revert::LiveAccount<'_> {
    fn namespaces(&self) -> Result<Vec<Namespace>, CloudflareError> {
        use crate::migrate::sweep::Cloudflare as _;
        self.rest.namespaces()
    }

    fn deployments(&self, script: &str) -> Result<Vec<Deployment>, CloudflareError> {
        match <Self as super::revert::Account>::deployments(self, script) {
            // A receiver that is a first deploy is not on the account yet.
            Err(e) if e.status() == Some(404) => Ok(Vec::new()),
            other => other,
        }
    }

    fn version_facts(
        &self,
        script: &str,
        version: &str,
    ) -> Result<Option<String>, CloudflareError> {
        <Self as super::revert::Account>::version_facts(self, script, version)
    }
}

/// One phase of a move (§3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Phase {
    /// Each receiver, expecting the classes and forwarding the moving contexts.
    Receive,
    /// Each source, giving the classes up.
    Commit,
    /// Each receiver, serving the classes.
    GoLive,
}

impl Phase {
    /// The number the deploy facts carry (`transfer.phase`).
    pub(crate) fn number(self) -> u8 {
        match self {
            Phase::Receive => 1,
            Phase::Commit => 2,
            Phase::GoLive => 3,
        }
    }
}

/// How one phase push ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PhasePush {
    Landed,
    /// Cut short by a shared Ctrl-C: whether it landed is unknown, so it is not recorded.
    Interrupted,
    Failed(DeployFailure),
    /// A commit refused with `transferred_pending_not_found`: the receiver's pending transfer is
    /// gone, so nothing committed and the data is still the source's.
    TransferLost(DeployFailure),
}

/// The pushes of a move: one phase variant of one Worker at a time, from several threads at once.
pub(crate) trait MovePusher: Sync {
    /// Push `worker`'s variant for `phase`, its deploy facts carrying `transfer`.
    fn push(&self, worker: &str, phase: Phase, transfer: &Transfer) -> PhasePush;
    /// What the ledger records of `worker` once its `phase` push has landed (its Worker record,
    /// as for any push), in the same write as its `done`.
    fn record(&self, lock: &mut DeployLock, worker: &str, phase: Phase);
}

/// Why a move stopped: what it says, the pushes that failed, and whether it was interrupted.
#[derive(Debug, Default)]
pub(crate) struct MoveStop {
    pub(crate) refusals: Vec<Entry>,
    pub(crate) failures: Vec<DeployFailure>,
    pub(crate) interrupted: bool,
}

impl MoveStop {
    fn refused(entry: Entry) -> Self {
        MoveStop {
            refusals: vec![entry],
            ..MoveStop::default()
        }
    }

    fn failed(failure: DeployFailure) -> Self {
        MoveStop {
            failures: vec![failure],
            ..MoveStop::default()
        }
    }

    fn account(what: &str, e: &CloudflareError) -> Self {
        Self::failed(DeployFailure::driver(format!(
            "could not read {what} for the agent move: {e}"
        )))
    }

    fn is_empty(&self) -> bool {
        self.refusals.is_empty() && self.failures.is_empty() && !self.interrupted
    }
}

/// A fresh move id: 8 random hex digits.
pub(crate) fn fresh_move_id() -> Result<String, String> {
    let mut bytes = [0u8; 4];
    getrandom::fill(&mut bytes).map_err(|e| format!("could not generate a move id: {e}"))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// Now, in RFC 3339 (UTC, to the second), for a record's `started`.
pub(crate) fn now_rfc3339() -> String {
    let t = time::OffsetDateTime::now_utc();
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// One run's moves in one environment, against one account, with one way of pushing.
pub(crate) struct MoveRun<'a> {
    pub(crate) environment: &'a str,
    pub(crate) account: &'a dyn MoveAccount,
    pub(crate) pusher: &'a dyn MovePusher,
}

/// A class whose commit the listing does not show.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Unconfirmed {
    pub(crate) class: String,
    pub(crate) source: String,
    /// The receiving script the namespace should be on.
    pub(crate) expected: String,
    /// The namespace recorded before phase 1.
    pub(crate) id: String,
    /// The script the listing shows that namespace on, `None` when it lists no such namespace.
    pub(crate) found: Option<String>,
}

impl MoveRun<'_> {
    /// A script's name on the account in this run's environment.
    fn deployed(&self, script: &str) -> String {
        super::deployed_worker_name(self.environment, script)
    }

    /// The record of the move `move_id`, as the ledger holds it now.
    fn record(writer: &LedgerWriter<'_>, environment: &str, move_id: &str) -> AgentMoveRecord {
        writer
            .lock()
            .open_moves(environment)
            .iter()
            .find(|r| r.move_id == move_id)
            .cloned()
            .expect("a move's record is open while it is moved")
    }

    /// Change the record of `move_id` with `change`, then write the ledger.
    fn write(
        &self,
        writer: &mut LedgerWriter<'_>,
        move_id: &str,
        change: impl FnOnce(&mut AgentMoveRecord),
    ) -> Result<(), MoveStop> {
        let environment = self.environment;
        writer
            .change(|lock| {
                if let Some(record) = lock.open_move_mut(environment, move_id) {
                    change(record);
                }
            })
            .map_err(|e| ledger_failure(writer, &e))
    }

    /// Open the move set `moves` (before its first push): read the account, refuse what cannot
    /// be moved (§7.1 item 4, §7.2), and write its record at `receiving` with the baselines the
    /// rest of the move compares against: each Worker's live deployment, and each moved class's
    /// namespace.
    pub(crate) fn open(
        &self,
        writer: &mut LedgerWriter<'_>,
        moves: Vec<AgentMoveEntry>,
        move_id: &str,
        started: &str,
    ) -> Result<(), MoveStop> {
        let listing = self
            .account
            .namespaces()
            .map_err(|e| MoveStop::account("the Durable Object namespaces", &e))?;
        let mut namespaces = BTreeMap::new();
        let mut stop = MoveStop::default();
        for m in &moves {
            let (from, to) = (self.deployed(&m.from_script), self.deployed(&m.to_script));
            for class in &m.classes {
                let on = |script: &str| {
                    listing
                        .iter()
                        .find(|n| n.script == script && &n.class == class)
                };
                match (on(&from), on(&to)) {
                    (Some(_), Some(theirs)) => {
                        stop.refusals.push(receiver_has_class(m, class, &to, theirs))
                    }
                    (None, Some(_)) => stop.refusals.push(already_under_way(
                        &format!(
                            "agent `{class}` of context `{}` is already on `{to}`, and `{from}` has none",
                            m.context
                        ),
                        &m.context,
                    )),
                    (None, None) => stop.refusals.push(
                        Entry::new(
                            AGENT_MOVE_REFUSED,
                            format!(
                                "context `{}` would move agent `{class}` from `{from}` to `{to}`, but the account lists no namespace for it on `{from}`; deploy it where it is first, so that the move has the namespace to keep.",
                                m.context
                            ),
                        )
                        .with("reason", "no_namespace")
                        .with("context", m.context.as_str())
                        .with("agents", vec![class.clone()])
                        .with("from", from.as_str())
                        .with("to", to.as_str()),
                    ),
                    (Some(ours), None) => {
                        namespaces.insert(class.clone(), ours.id.clone());
                    }
                }
            }
        }
        let mut deployments = BTreeMap::new();
        for (worker, script) in workers_of(&moves) {
            let script = self.deployed(&script);
            let newest = self.newest(&script)?;
            if let Some((_, Some(transfer))) = &newest.1
                && transfer.phase != Phase::GoLive.number()
            {
                stop.refusals.push(already_under_way(
                    &format!(
                        "`{script}`'s live version was pushed by phase {} of another move (`{}`)",
                        transfer.phase, transfer.move_id
                    ),
                    &moves[0].context,
                ));
            }
            deployments.insert(worker, newest.0);
        }
        if !stop.is_empty() {
            return Err(stop);
        }
        let record = AgentMoveRecord {
            move_id: move_id.to_string(),
            step: MoveStep::Receiving,
            started: started.to_string(),
            bynk: env!("CARGO_PKG_VERSION").to_string(),
            done: Vec::new(),
            moves,
            deployments,
            namespaces,
        };
        let environment = self.environment;
        writer
            .change(|lock| lock.open_move(environment, record))
            .map_err(|e| ledger_failure(writer, &e))
    }

    /// The set's receive node: phase 1, unless the record is already past it.
    pub(crate) fn receive(
        &self,
        writer: &mut LedgerWriter<'_>,
        move_id: &str,
    ) -> Result<(), MoveStop> {
        let record = Self::record(writer, self.environment, move_id);
        if record.step != MoveStep::Receiving {
            return Ok(());
        }
        self.check_interleaved(&record)?;
        let outcome = self.push_all(writer, &record, Phase::Receive, &receivers(&record));
        if !outcome.stop.is_empty() {
            return Err(outcome.stop);
        }
        self.write(writer, move_id, |r| {
            r.step = MoveStep::Received;
            r.done.clear();
        })
    }

    /// The set's commit node: phase 2, the confirmation, then phase 3 and the record's close, from
    /// whichever step the record holds.
    pub(crate) fn commit(
        &self,
        writer: &mut LedgerWriter<'_>,
        move_id: &str,
    ) -> Result<(), MoveStop> {
        loop {
            let record = Self::record(writer, self.environment, move_id);
            match record.step {
                MoveStep::Receiving => {
                    return Err(MoveStop::failed(DeployFailure::driver(format!(
                        "the agent move `{move_id}` cannot commit before its phase 1 has landed"
                    ))));
                }
                MoveStep::Received => self.write(writer, move_id, |r| {
                    r.step = MoveStep::Committing;
                    r.done.clear();
                })?,
                MoveStep::Committing => {
                    let sources = sources(&record);
                    if !sources.iter().all(|s| record.done.contains(s)) {
                        self.check_interleaved(&record)?;
                        let outcome = self.push_all(writer, &record, Phase::Commit, &sources);
                        if !outcome.lost.is_empty() {
                            return Err(self.transfer_lost(writer, &record, outcome));
                        }
                        if !outcome.stop.is_empty() {
                            return Err(outcome.stop);
                        }
                    }
                    let record = Self::record(writer, self.environment, move_id);
                    let unconfirmed = self.unconfirmed(&record)?;
                    if !unconfirmed.is_empty() {
                        return Err(self.commit_unconfirmed(writer, &record, &unconfirmed));
                    }
                    self.write(writer, move_id, |r| {
                        r.step = MoveStep::Committed;
                        r.done.clear();
                    })?;
                }
                MoveStep::Committed => self.write(writer, move_id, |r| {
                    r.step = MoveStep::GoingLive;
                    r.done.clear();
                })?,
                MoveStep::GoingLive => {
                    self.check_interleaved(&record)?;
                    // S6: the hard gate, immediately before the phase 3 pushes.
                    let unconfirmed = self.unconfirmed(&record)?;
                    if !unconfirmed.is_empty() {
                        return Err(self.commit_unconfirmed(writer, &record, &unconfirmed));
                    }
                    let outcome =
                        self.push_all(writer, &record, Phase::GoLive, &receivers(&record));
                    if !outcome.stop.is_empty() {
                        return Err(outcome.stop);
                    }
                    let environment = self.environment;
                    return writer
                        .change(|lock| lock.close_move(environment, move_id))
                        .map_err(|e| ledger_failure(writer, &e));
                }
            }
        }
    }

    /// The moved classes whose commit the listing does not show: each must have the namespace
    /// recorded before phase 1, on its receiving script. A namespace with that id on another
    /// script, or the receiving script with a namespace of another id, is not a commit.
    pub(crate) fn unconfirmed(
        &self,
        record: &AgentMoveRecord,
    ) -> Result<Vec<Unconfirmed>, MoveStop> {
        let listing = self
            .account
            .namespaces()
            .map_err(|e| MoveStop::account("the Durable Object namespaces", &e))?;
        let mut out = Vec::new();
        for m in &record.moves {
            let expected = self.deployed(&m.to_script);
            for class in &m.classes {
                let id = record.namespaces.get(class).cloned().unwrap_or_default();
                let found = listing.iter().find(|n| n.id == id && &n.class == class);
                if found.is_some_and(|n| n.script == expected) {
                    continue;
                }
                out.push(Unconfirmed {
                    class: class.clone(),
                    source: m.from_worker.clone(),
                    expected: expected.clone(),
                    id,
                    found: found.map(|n| n.script.clone()),
                });
            }
        }
        Ok(out)
    }

    /// Stop with [`MOVE_COMMIT_UNCONFIRMED`]: the record goes (or stays) at `committing`, with
    /// `done` cleared for each source whose classes are not confirmed, so the next run pushes
    /// their commit again; a source whose classes are confirmed stays `done`.
    fn commit_unconfirmed(
        &self,
        writer: &mut LedgerWriter<'_>,
        record: &AgentMoveRecord,
        unconfirmed: &[Unconfirmed],
    ) -> MoveStop {
        let again: BTreeSet<String> = unconfirmed.iter().map(|u| u.source.clone()).collect();
        let confirmed: Vec<String> = sources(record)
            .into_iter()
            .filter(|s| !again.contains(s))
            .collect();
        let detail = unconfirmed
            .iter()
            .map(|u| match &u.found {
                Some(script) => format!(
                    "agent `{}`'s namespace `{}` is on `{script}`, not `{}`",
                    u.class, u.id, u.expected
                ),
                None => format!(
                    "the account lists no namespace `{}` for agent `{}` (expected on `{}`)",
                    u.id, u.class, u.expected
                ),
            })
            .collect::<Vec<_>>()
            .join("; ");
        let mut stop = MoveStop::refused(
            Entry::new(
                MOVE_COMMIT_UNCONFIRMED,
                format!(
                    "the agent move `{}` is not confirmed as committed: {detail}. Nothing is pushed live, because a receiver serving a class before its commit starts it empty and drops the transfer. The next `bynk deploy` pushes the commit of {} again.",
                    record.move_id,
                    names_phrase(&again)
                ),
            )
            .with("move", record.move_id.as_str())
            .with(
                "agents",
                unconfirmed.iter().map(|u| u.class.clone()).collect::<Vec<_>>(),
            )
            .with("sources", again.iter().cloned().collect::<Vec<_>>()),
        );
        if let Err(e) = self.write(writer, &record.move_id, |r| {
            r.step = MoveStep::Committing;
            r.done = confirmed;
        }) {
            stop.failures.extend(e.failures);
        }
        stop
    }

    /// Stop with [`MOVE_TRANSFER_LOST`]: the record goes back to `receiving`, with the `done` of
    /// each receiver of a lost transfer cleared, so the next run pushes its phase 1 again and then
    /// the commit.
    fn transfer_lost(
        &self,
        writer: &mut LedgerWriter<'_>,
        record: &AgentMoveRecord,
        outcome: Pushes,
    ) -> MoveStop {
        let mut stop = outcome.stop;
        let lost_receivers: BTreeSet<String> = record
            .moves
            .iter()
            .filter(|m| outcome.lost.contains(&m.from_worker))
            .map(|m| m.to_worker.clone())
            .collect();
        for m in record
            .moves
            .iter()
            .filter(|m| outcome.lost.contains(&m.from_worker))
        {
            let (from, to) = (self.deployed(&m.from_script), self.deployed(&m.to_script));
            stop.refusals.push(
                Entry::new(
                    MOVE_TRANSFER_LOST,
                    format!(
                        "the commit of context `{}` from `{from}` was refused: `{to}` no longer expects {} (`transferred_pending_not_found`). Nothing has committed, so the data is safe on `{from}`. The next `bynk deploy` pushes `{to}`'s phase 1 again, then the commit. If `{to}` now has a namespace of its own for the class, Cloudflare refuses that phase 1 (`phase_one_transfer_target_class_provisioned`): that namespace was made outside the move and holds none of `{from}`'s data, so remove the class from `{to}` (`removed agent`, once it is empty) or cancel the move and start it again.",
                        m.context,
                        agents_phrase(&m.classes)
                    ),
                )
                .with("move", record.move_id.as_str())
                .with("context", m.context.as_str())
                .with("agents", m.classes.clone())
                .with("from", from.as_str())
                .with("to", to.as_str()),
            );
        }
        let done: Vec<String> = receivers(record)
            .into_iter()
            .filter(|r| !lost_receivers.contains(r))
            .collect();
        if let Err(e) = self.write(writer, &record.move_id, |r| {
            r.step = MoveStep::Receiving;
            r.done = done;
        }) {
            stop.failures.extend(e.failures);
        }
        stop
    }

    /// A Worker's newest deployment id (or [`NO_DEPLOYMENT`]), with its version and that
    /// version's `transfer`, when it serves one version.
    #[allow(clippy::type_complexity)]
    fn newest(
        &self,
        script: &str,
    ) -> Result<(String, Option<(String, Option<Transfer>)>), MoveStop> {
        let deployments = self
            .account
            .deployments(script)
            .map_err(|e| MoveStop::account(&format!("`{script}`'s deployments"), &e))?;
        let Some(newest) = deployments.first() else {
            return Ok((NO_DEPLOYMENT.to_string(), None));
        };
        let version = match newest.versions.as_slice() {
            [(version, 100)] => version.clone(),
            _ => return Ok((newest.id.clone(), None)),
        };
        let transfer = self
            .account
            .version_facts(script, &version)
            .map_err(|e| MoveStop::account(&format!("`{script}`'s deploy facts"), &e))?
            .and_then(|text| Facts::decode(&text))
            .and_then(|facts| facts.transfer);
        Ok((newest.id.clone(), Some((version, transfer))))
    }

    /// §7.2: before the pushes of a step, every Worker of the set must still be at the
    /// deployment the record holds for it, or at one of this move's own pushes it may be at
    /// (a push of the phase under way that landed without being recorded, which is pushed again).
    /// Anything else was deployed by something else, and the run stops with
    /// [`MOVE_INTERLEAVED`].
    fn check_interleaved(&self, record: &AgentMoveRecord) -> Result<(), MoveStop> {
        let mut stop = MoveStop::default();
        for (worker, script) in workers_of(&record.moves) {
            let script = self.deployed(&script);
            let recorded = record
                .deployments
                .get(&worker)
                .map_or(NO_DEPLOYMENT, String::as_str);
            let (id, version) = self.newest(&script)?;
            if id == recorded {
                continue;
            }
            let own = version
                .as_ref()
                .and_then(|(_, transfer)| transfer.as_ref())
                .is_some_and(|t| {
                    t.move_id == record.move_id && own_phases(record, &worker).contains(&t.phase)
                });
            if own {
                continue;
            }
            stop.refusals.push(
                Entry::new(
                    MOVE_INTERLEAVED,
                    format!(
                        "`{script}` was deployed by something other than the agent move `{}` since the move recorded it: its live deployment is `{id}`, and the move recorded `{recorded}`. The move stops rather than push over that deployment. Find what deployed it; `bynk.deploy.lock` holds the move at step `{}`.",
                        record.move_id,
                        step_name(record.step)
                    ),
                )
                .with("move", record.move_id.as_str())
                .with("worker", worker.as_str())
                .with("script", script.as_str())
                .with("recorded", recorded)
                .with("deployment", id.as_str()),
            );
        }
        if stop.is_empty() { Ok(()) } else { Err(stop) }
    }

    /// Push `workers`' `phase` variant, every one not yet `done` at once, and record each that
    /// lands as it lands. Every push runs to its end, whatever the others do: one that fails
    /// leaves the others recorded `done`, and the run stops once they have all settled.
    fn push_all(
        &self,
        writer: &mut LedgerWriter<'_>,
        record: &AgentMoveRecord,
        phase: Phase,
        workers: &[String],
    ) -> Pushes {
        let pending: Vec<&String> = workers
            .iter()
            .filter(|w| !record.done.contains(w))
            .collect();
        let transfers: Vec<(String, Transfer)> = pending
            .iter()
            .map(|w| ((*w).clone(), self.transfer(record, w, phase)))
            .collect();
        let mut outcome = Pushes::default();
        let pusher = self.pusher;
        std::thread::scope(|scope| {
            let (tx, rx) = mpsc::channel();
            for (worker, transfer) in &transfers {
                let tx = tx.clone();
                scope.spawn(move || {
                    let _ = tx.send((worker.clone(), pusher.push(worker, phase, transfer)));
                });
            }
            drop(tx);
            for (worker, pushed) in rx {
                match pushed {
                    PhasePush::Landed => self.landed(writer, record, &worker, phase, &mut outcome),
                    PhasePush::Interrupted => outcome.stop.interrupted = true,
                    PhasePush::Failed(f) => outcome.stop.failures.push(f),
                    PhasePush::TransferLost(f) => {
                        outcome.stop.failures.push(f);
                        outcome.lost.insert(worker);
                    }
                }
            }
        });
        outcome
    }

    /// Record `worker`'s landed `phase` push: in `done`, at its new deployment, with its Worker
    /// record, in one write.
    fn landed(
        &self,
        writer: &mut LedgerWriter<'_>,
        record: &AgentMoveRecord,
        worker: &str,
        phase: Phase,
        outcome: &mut Pushes,
    ) {
        let script = record_script(record, worker);
        // The new deployment, so the next check sees it as recorded. Unread, the record keeps the
        // old id and the next check knows the push by its facts.
        let deployment = self.newest(&self.deployed(&script)).ok().map(|(id, _)| id);
        let (environment, pusher) = (self.environment, self.pusher);
        let move_id = record.move_id.as_str();
        let written = writer.change(|lock| {
            if let Some(r) = lock.open_move_mut(environment, move_id) {
                if !r.done.iter().any(|d| d == worker) {
                    r.done.push(worker.to_string());
                    r.done.sort();
                }
                if let Some(id) = deployment {
                    r.deployments.insert(worker.to_string(), id);
                }
            }
            pusher.record(lock, worker, phase);
        });
        if let Err(e) = written {
            outcome.stop.failures.push(DeployFailure::driver(format!(
                "`{worker}`'s phase {} push landed but could not be recorded in {}: {e}",
                phase.number(),
                writer.path().display()
            )));
        }
    }

    /// What `worker`'s `phase` version says of the move (§4.2).
    fn transfer(&self, record: &AgentMoveRecord, worker: &str, phase: Phase) -> Transfer {
        let mut transfer = Transfer {
            move_id: record.move_id.clone(),
            phase: phase.number(),
            ..Transfer::default()
        };
        for m in &record.moves {
            match phase {
                Phase::Receive if m.to_worker == worker => {
                    for class in &m.classes {
                        transfer
                            .expecting
                            .insert(class.clone(), self.deployed(&m.from_script));
                    }
                }
                Phase::Commit if m.from_worker == worker => {
                    for class in &m.classes {
                        transfer
                            .transferred
                            .insert(class.clone(), self.deployed(&m.to_script));
                    }
                }
                _ => {}
            }
        }
        transfer
    }
}

/// What a phase's pushes came to: why it stops, and the sources whose transfer was lost.
#[derive(Default)]
struct Pushes {
    stop: MoveStop,
    lost: BTreeSet<String>,
}

/// The phases of this move a Worker of the set may be live at, past its recorded deployment,
/// given the step: a receiver at its phase 1 (and, going live, its phase 3); a source, from
/// the commit on, at its phase 2.
fn own_phases(record: &AgentMoveRecord, worker: &str) -> Vec<u8> {
    let receiver = record.moves.iter().any(|m| m.to_worker == worker);
    match (receiver, record.step) {
        (true, MoveStep::GoingLive) => vec![1, 3],
        (true, _) => vec![1],
        (false, MoveStep::Receiving | MoveStep::Received) => Vec::new(),
        (false, _) => vec![2],
    }
}

/// Every Worker of the moves, with its script: the sources, then the receivers.
fn workers_of(moves: &[AgentMoveEntry]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    for m in moves {
        out.insert(m.from_worker.clone(), m.from_script.clone());
        out.insert(m.to_worker.clone(), m.to_script.clone());
    }
    out
}

fn record_script(record: &AgentMoveRecord, worker: &str) -> String {
    workers_of(&record.moves)
        .remove(worker)
        .unwrap_or_else(|| worker.to_string())
}

/// The set's receiving Workers, in order.
pub(crate) fn receivers(record: &AgentMoveRecord) -> Vec<String> {
    let set: BTreeSet<String> = record.moves.iter().map(|m| m.to_worker.clone()).collect();
    set.into_iter().collect()
}

/// The set's source Workers, in order.
pub(crate) fn sources(record: &AgentMoveRecord) -> Vec<String> {
    let set: BTreeSet<String> = record.moves.iter().map(|m| m.from_worker.clone()).collect();
    set.into_iter().collect()
}

pub(crate) fn step_name(step: MoveStep) -> &'static str {
    match step {
        MoveStep::Receiving => "receiving",
        MoveStep::Received => "received",
        MoveStep::Committing => "committing",
        MoveStep::Committed => "committed",
        MoveStep::GoingLive => "going_live",
    }
}

fn ledger_failure(writer: &LedgerWriter<'_>, e: &str) -> MoveStop {
    MoveStop::failed(DeployFailure::driver(format!(
        "could not record the agent move in {}: {e}",
        writer.path().display()
    )))
}

/// §7.1 item 4: the receiver already owns a namespace for the class, so Cloudflare would refuse
/// phase 1 (`phase_one_transfer_target_class_provisioned`).
fn receiver_has_class(m: &AgentMoveEntry, class: &str, to: &str, theirs: &Namespace) -> Entry {
    Entry::new(
        AGENT_MOVE_REFUSED,
        format!(
            "context `{}` would move agent `{class}` to `{to}`, but `{to}` already has a namespace of its own for it (`{}`), which Cloudflare will not replace with a transfer; it was made outside the move, for instance by an earlier `--accept-fresh-agent`. Remove the class from `{to}` (`removed agent`, once it is empty) and deploy again.",
            m.context, theirs.id
        ),
    )
    .with("reason", "receiver_has_class")
    .with("context", m.context.as_str())
    .with("agents", vec![class.to_string()])
    .with("to", to)
}

/// §7.2: a second checkout, without the open record, planning a move that is under way or done.
fn already_under_way(what: &str, context: &str) -> Entry {
    Entry::new(
        MOVE_INTERLEAVED,
        format!(
            "{what}: the move is already under way or done. Pull `bynk.deploy.lock` and deploy from the checkout that holds the move's record."
        ),
    )
    .with("context", context)
}

fn names_phrase(names: &BTreeSet<String>) -> String {
    names
        .iter()
        .map(|n| format!("`{n}`"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn agents_phrase(classes: &[String]) -> String {
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

#[cfg(test)]
#[path = "move_apply_tests.rs"]
mod tests;
