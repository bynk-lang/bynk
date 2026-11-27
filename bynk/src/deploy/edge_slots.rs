//! #4526: the two verification slots of an edge key, and what a rotation does to them.
//!
//! A callee verifies the calls and queue messages of one caller under `BYNK_CALL_FROM_<S>`. A
//! rotation mints a new key, and Cloudflare never returns a secret's value, so the old key cannot be
//! copied anywhere (ADR 0195 D1). What a rotation can do without reading it is put the new key in
//! the *other* place and leave the old one where it is. So the callee has two slots for the edge,
//! `BYNK_CALL_FROM_<S>` and `BYNK_CALL_FROM2_<S>`, and verifies under every slot that holds a key and
//! is not retiring. A slot is retiring when a marker secret, `BYNK_CALL_UNTIL_<S>` or
//! `BYNK_CALL_UNTIL2_<S>`, names the instant (whole seconds since the epoch) after which the runtime
//! stops verifying under it. The runtime enforces that instant itself (`edgeVerifyKeysOf`), so a
//! retired key stops working whether or not another deploy ever runs; the deploy's cleanup, which
//! deletes the slot and its marker once the ledger says the instant has passed, is tidiness.
//!
//! This module is the pure part: the state of one edge's slots, what a rotation does to it
//! ([`plan_write`]), and the secrets it puts and deletes ([`CalleeWrite::ops`]). `decide_edge_key`
//! calls it; the apply step runs the operations.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use bynk_driver::{colour, emsg};
use serde::{Deserialize, Serialize};

use super::config::{deploy_command, deployed_worker_name};
use super::ledger::DeployLock;
use super::plan::DeployOptions;
use super::plan::end_presence;
use super::secrets::{Edge, EdgeKey, Presence, Rotation, Secrets, decide_edge_key, mint_edge_key};
use crate::cloudflare::CloudflareApi;
use std::process::ExitCode;

/// How long a rotated-out edge key keeps verifying: 15 days. A signed queue message can wait out the
/// queue's retention, 14 days at most, and be retried after it, and it is verified when it is
/// delivered, not when it was signed (`bynk-emit/runtime/src/queue_admit.ts`), so a key must outlive
/// the longest wait by the retry margin. Decision H of #3899 gives `Foreign` senders the same window.
pub(crate) const EDGE_KEY_GRACE_SECONDS: u64 = 15 * 24 * 60 * 60;

/// One of an edge key's two places on the callee.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum Slot {
    One,
    Two,
}

impl Slot {
    fn index(self) -> usize {
        match self {
            Slot::One => 0,
            Slot::Two => 1,
        }
    }

    fn other(self) -> Slot {
        match self {
            Slot::One => Slot::Two,
            Slot::Two => Slot::One,
        }
    }

    /// `1` or `2`, for a message.
    pub(crate) fn number(self) -> u8 {
        self.index() as u8 + 1
    }
}

/// The four secret names of one edge from one caller on the callee: each slot's key and its
/// retirement marker, derived from the first slot's name (`BYNK_CALL_FROM_<S>`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SlotNames {
    suffix: String,
}

impl SlotNames {
    /// `None` for a name that is not an edge key's first slot.
    pub(crate) fn of(slot_one_key: &str) -> Option<SlotNames> {
        slot_one_key
            .strip_prefix(bynk_emit::EDGE_FROM_PREFIX)
            .map(|suffix| SlotNames {
                suffix: suffix.to_string(),
            })
    }

    pub(crate) fn key(&self, slot: Slot) -> String {
        let prefix = match slot {
            Slot::One => bynk_emit::EDGE_FROM_PREFIX,
            Slot::Two => bynk_emit::EDGE_FROM_SLOT2_PREFIX,
        };
        format!("{prefix}{}", self.suffix)
    }

    pub(crate) fn until(&self, slot: Slot) -> String {
        let prefix = match slot {
            Slot::One => bynk_emit::EDGE_UNTIL_PREFIX,
            Slot::Two => bynk_emit::EDGE_UNTIL_SLOT2_PREFIX,
        };
        format!("{prefix}{}", self.suffix)
    }
}

/// What the callee holds for one edge key: which slots carry a key and which a retirement marker.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SlotState {
    key: [bool; 2],
    until: [bool; 2],
}

impl SlotState {
    /// The state from the Worker's secret listing. `slot_one_held` is the decision's own answer for
    /// the first slot (an unread listing counts as holding it, as for any edge key); the other
    /// three are known only from a listing that was read.
    pub(crate) fn from_listed(
        names: &SlotNames,
        slot_one_held: bool,
        listed: Option<&BTreeSet<String>>,
    ) -> SlotState {
        let has = |name: String| listed.is_some_and(|set| set.contains(&name));
        SlotState {
            key: [slot_one_held, has(names.key(Slot::Two))],
            until: [has(names.until(Slot::One)), has(names.until(Slot::Two))],
        }
    }

    /// A state for a test or a first mint.
    #[cfg(test)]
    pub(crate) fn of(key: [bool; 2], until: [bool; 2]) -> SlotState {
        SlotState { key, until }
    }

    fn holds(&self, slot: Slot) -> bool {
        self.key[slot.index()]
    }

    fn retiring(&self, slot: Slot) -> bool {
        self.until[slot.index()]
    }

    fn any_key(&self) -> bool {
        self.key[0] || self.key[1]
    }
}

/// How a rotation treats the key it replaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// The old key keeps verifying for [`EDGE_KEY_GRACE_SECONDS`].
    Grace,
    /// The old key stops verifying at once (`--revoke-previous-edge-key`): a key that leaked.
    Revoke,
}

/// One step on the callee's secrets for an edge key. The new key's value is the decision's, not
/// the step's, so a step is only *where*.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotOp {
    /// Put the new key in this slot.
    PutKey(Slot),
    /// Mark this slot retiring: put the instant, in seconds since the epoch.
    PutUntil(Slot, u64),
    DeleteKey(Slot),
    DeleteUntil(Slot),
}

/// What a rotation (or a cleanup) does to the callee's slots, in the order to do it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CalleeWrite {
    pub(crate) ops: Vec<SlotOp>,
}

impl CalleeWrite {
    /// The slot this write marks retiring, with the instant.
    pub(crate) fn retiring(&self) -> Option<(Slot, u64)> {
        self.ops.iter().find_map(|op| match op {
            SlotOp::PutUntil(slot, at) => Some((*slot, *at)),
            _ => None,
        })
    }

    /// The puts, in order: the new key first, so the callee never runs without it, then the marker.
    pub(crate) fn puts(&self, names: &SlotNames, value: &str) -> Vec<(String, String)> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                SlotOp::PutKey(slot) => Some((names.key(*slot), value.to_string())),
                SlotOp::PutUntil(slot, at) => Some((names.until(*slot), at.to_string())),
                _ => None,
            })
            .collect()
    }

    /// The deletes, in order: a slot's key before its marker, so a key is never left live by
    /// losing its marker first.
    pub(crate) fn deletes(&self, names: &SlotNames) -> Vec<String> {
        self.ops
            .iter()
            .filter_map(|op| match op {
                SlotOp::DeleteKey(slot) => Some(names.key(*slot)),
                SlotOp::DeleteUntil(slot) => Some(names.until(*slot)),
                _ => None,
            })
            .collect()
    }
}

/// What a rotation does on the callee, given the state of its slots.
///
/// The new key goes in a slot that holds no key, else in one that is retiring (replacing a key whose
/// window was already running out), else in the second slot. The other slot, if it holds a key, is
/// marked retiring `now + grace` (a slot already retiring is left to its own instant: the marker is
/// a secret and cannot be read, so it is never extended). A marker left on the slot the new key now
/// holds is deleted after the put. The put comes first so the callee never runs without the key,
/// and a failure after it leaves both keys accepted, which only delays the old one's retirement.
///
/// [`Mode::Revoke`] puts the key in the live slot, or the first when there is none, and deletes every
/// other key and every marker.
pub(crate) fn plan_write(state: SlotState, mode: Mode, now: u64) -> CalleeWrite {
    let mut ops = Vec::new();
    if mode == Mode::Revoke {
        let target = [Slot::One, Slot::Two]
            .into_iter()
            .find(|s| state.holds(*s) && !state.retiring(*s))
            .unwrap_or(Slot::One);
        ops.push(SlotOp::PutKey(target));
        for slot in [Slot::One, Slot::Two] {
            if slot != target && state.holds(slot) {
                ops.push(SlotOp::DeleteKey(slot));
            }
        }
        for slot in [Slot::One, Slot::Two] {
            if state.retiring(slot) {
                ops.push(SlotOp::DeleteUntil(slot));
            }
        }
        return CalleeWrite { ops };
    }
    let target = [Slot::One, Slot::Two]
        .into_iter()
        .find(|s| !state.holds(*s))
        .or_else(|| {
            [Slot::One, Slot::Two]
                .into_iter()
                .find(|s| state.retiring(*s))
        })
        .unwrap_or(Slot::Two);
    ops.push(SlotOp::PutKey(target));
    let other = target.other();
    if state.any_key() && state.holds(other) && !state.retiring(other) {
        ops.push(SlotOp::PutUntil(
            other,
            now.saturating_add(EDGE_KEY_GRACE_SECONDS),
        ));
    }
    if state.retiring(target) {
        ops.push(SlotOp::DeleteUntil(target));
    }
    CalleeWrite { ops }
}

/// The cleanup of a slot whose window has passed: its key, then its marker, each if present.
pub(crate) fn retire_write(state: SlotState, slot: Slot) -> CalleeWrite {
    let mut ops = Vec::new();
    if state.holds(slot) {
        ops.push(SlotOp::DeleteKey(slot));
    }
    if state.retiring(slot) {
        ops.push(SlotOp::DeleteUntil(slot));
    }
    CalleeWrite { ops }
}

/// A slot a rotation is retiring, as the ledger records it: which slot, and the instant after which
/// the runtime refuses it (the same instant as its marker). Committed to the project, so it holds no
/// key and no secret: only a slot number and a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Retiring {
    pub(crate) slot: Slot,
    pub(crate) until: u64,
}

/// Whole seconds since the Unix epoch.
pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The UTC date of `secs` since the epoch, `YYYY-MM-DD` (the civil-from-days conversion).
pub(crate) fn utc_date(secs: u64) -> String {
    let days = (secs / 86_400) as i64 + 719_468;
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

impl Edge {
    /// Whether `context` is one of the contexts at either end of this edge.
    pub(crate) fn involves(&self, context: &str) -> bool {
        let serves = |worker: &str, members: &[String]| {
            if members.is_empty() {
                super::secrets::context_of(worker) == context
            } else {
                members.iter().any(|m| m == context)
            }
        };
        serves(&self.caller, &self.caller_contexts) || serves(&self.callee, &self.callee_contexts)
    }
}

/// A retiring slot whose window has passed: the edge's first-slot name, and the secrets to delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Expired {
    pub(crate) slot_one_key: String,
    pub(crate) deletes: Vec<String>,
}

/// What a run decided of its edge keys: each edge's, and the retiring slots to clean up, by the
/// callee Worker that holds them.
pub(crate) struct EdgeDecisions {
    pub(crate) keys: BTreeMap<Edge, EdgeKey>,
    pub(crate) expired: BTreeMap<String, Vec<Expired>>,
}

/// What `--revoke-previous-edge-key` and `--force` are told about a rotation, printed once.
pub(crate) fn rotation_notice(
    environment: &str,
    edge: &Edge,
    mode: Mode,
    retiring: Option<(Slot, u64)>,
) -> Option<String> {
    let edge_text = edge.describe(environment);
    let callee = deployed_worker_name(environment, &edge.callee);
    match (mode, retiring) {
        (Mode::Revoke, _) => Some(format!(
            "bynk: note: revoking the edge key {edge_text}: the previous key stops verifying on `{callee}` at once, so a message still in a queue that was signed under it is refused.\n"
        )),
        (Mode::Grace, Some((slot, until))) => Some(format!(
            "bynk: note: rotating the edge key {edge_text}: the previous key stays in slot {} on `{callee}` and keeps verifying until {} ({} days from now), so messages signed under it that are still in a queue are admitted; the new key is in slot {}.\n  \
             If the previous key leaked, rotate with `{} --revoke-previous-edge-key <context>` instead, which drops it at once.\n",
            slot.number(),
            utc_date(until),
            EDGE_KEY_GRACE_SECONDS / 86_400,
            slot.other().number(),
            deploy_command(environment),
        )),
        (Mode::Grace, None) => None,
    }
}

/// Decide each edge's key over both its ends, and the retiring slots whose window has passed
/// (#4526). Each edge in `revoked` (`--revoke-previous-edge-key`) is re-keyed with no grace; one
/// that cannot be (a `--context` run that does not hold both ends) is refused, since a revoke that
/// quietly did nothing would leave a leaked key working.
pub(crate) fn decide_edge_keys(
    edges: &[Edge],
    in_run: &BTreeSet<&str>,
    listed: &BTreeMap<String, Option<BTreeSet<String>>>,
    lock: &DeployLock,
    opts: &DeployOptions,
    now: u64,
) -> Result<EdgeDecisions, ExitCode> {
    for context in &opts.revoke_edge_keys {
        if !edges.iter().any(|e| e.involves(context)) {
            emsg!(
                "bynk: error: `--revoke-previous-edge-key {context}`: no edge key in this run is on a context named `{context}`.\n  Name a context at either end of a calling edge, and deploy both ends (a whole-project run, or `--context` for one end with the other already live holds the old key)."
            );
            return Err(ExitCode::FAILURE);
        }
    }
    let mut decisions = EdgeDecisions {
        keys: BTreeMap::new(),
        expired: BTreeMap::new(),
    };
    for edge in edges {
        let names = match edge.keys() {
            Ok(names) => names,
            Err(e) => {
                emsg!("bynk: error: {e}");
                return Err(ExitCode::FAILURE);
            }
        };
        let held = |worker: &str, keys: &[String]| {
            end_presence(edge, worker, keys, listed, lock, &opts.environment)
        };
        let callee = held(&edge.callee, &names.on_callee);
        let caller = held(&edge.caller, &names.on_caller);
        let both_in_run =
            in_run.contains(edge.caller.as_str()) && in_run.contains(edge.callee.as_str());
        let revoke = opts.revoke_edge_keys.iter().any(|c| edge.involves(c));
        let slot_names = names.on_callee.first().and_then(|n| SlotNames::of(n));
        let callee_slots = slot_names.as_ref().map_or_else(SlotState::default, |n| {
            SlotState::from_listed(
                n,
                callee != Presence::Absent,
                listed.get(&edge.callee).and_then(Option::as_ref),
            )
        });
        let mode = if revoke { Mode::Revoke } else { Mode::Grace };
        let rotation = Rotation {
            callee_slots,
            mode,
            now,
            environment: &opts.environment,
        };
        let decision = decide_edge_key(
            edge,
            caller,
            callee,
            both_in_run,
            opts.force || revoke,
            &mut mint_edge_key,
            &rotation,
        )
        .map_err(|e| {
            emsg!("bynk: error: {e}");
            ExitCode::FAILURE
        })?;
        match &decision {
            EdgeKey::Set {
                rotating: true,
                callee: write,
                ..
            } => {
                if let Some(notice) =
                    rotation_notice(&opts.environment, edge, mode, write.retiring())
                {
                    colour::eprint(&notice);
                }
            }
            EdgeKey::Keep if revoke => {
                emsg!(
                    "bynk: error: the edge key {} was not revoked: a revoke needs both of its Workers in the run, and Cloudflare never returns a key's value, so a `--context` run cannot re-key it.\n  Deploy the whole project (`{} --revoke-previous-edge-key ...`).",
                    edge.describe(&opts.environment),
                    deploy_command(&opts.environment)
                );
                return Err(ExitCode::FAILURE);
            }
            _ => {}
        }
        if matches!(decision, EdgeKey::Keep) {
            collect_expired(
                edge,
                &names.on_callee,
                listed,
                lock,
                opts,
                now,
                &mut decisions,
            );
        }
        decisions.keys.insert(edge.clone(), decision);
    }
    Ok(decisions)
}

/// The retiring slots of `edge` on its callee whose window has passed, to be cleaned up when the
/// callee is deployed in this run.
fn collect_expired(
    edge: &Edge,
    on_callee: &[String],
    listed: &BTreeMap<String, Option<BTreeSet<String>>>,
    lock: &DeployLock,
    opts: &DeployOptions,
    now: u64,
    decisions: &mut EdgeDecisions,
) {
    let recorded = lock.retiring_edge_keys(&opts.environment, &edge.callee);
    for slot_one_key in on_callee {
        let Some(retiring) = recorded.get(slot_one_key).filter(|r| r.until <= now) else {
            continue;
        };
        let Some(names) = SlotNames::of(slot_one_key) else {
            continue;
        };
        let state = SlotState::from_listed(
            &names,
            // The first slot's presence is read like the others, from the listing.
            listed
                .get(&edge.callee)
                .and_then(Option::as_ref)
                .is_some_and(|set| set.contains(slot_one_key)),
            listed.get(&edge.callee).and_then(Option::as_ref),
        );
        decisions
            .expired
            .entry(edge.callee.clone())
            .or_default()
            .push(Expired {
                slot_one_key: slot_one_key.clone(),
                deletes: retire_write(state, retiring.slot).deletes(&names),
            });
    }
}

/// The secrets a rotation deletes on `worker`: a marker left on the slot a new key went in, and
/// everything a revoke drops. These are not tidiness: a marker left on the live key would retire
/// it, and a revoke that left the old key would leave a leaked key working, so a failure stops
/// the deploy.
pub(crate) fn rotation_deletes(secrets: &Secrets<'_>, worker: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (edge, decision) in secrets.edge_keys {
        let EdgeKey::Set { callee, .. } = decision else {
            continue;
        };
        if edge.callee != worker {
            continue;
        }
        let Ok(keys) = edge.keys() else { continue };
        for key in keys.on_callee {
            if let Some(slot_names) = SlotNames::of(&key) {
                names.extend(callee.deletes(&slot_names));
            }
        }
    }
    names
}

/// Delete `names` (secrets) on `worker`, by the Worker's account name, from an empty directory so
/// no project config is read. `Err` is the first that failed.
pub(crate) fn delete_secrets(
    api: &dyn CloudflareApi,
    environment: &str,
    worker: &str,
    names: &[String],
) -> Result<(), (String, String)> {
    let script = deployed_worker_name(environment, worker);
    let scratch = std::env::temp_dir().join("bynk-secret-delete");
    for name in names {
        api.named_secret(Path::new(&scratch), "delete", &script, name, "")
            .map_err(|e| (name.clone(), e.to_string()))?;
        eprintln!("bynk: deleted secret `{name}` on `{script}`");
    }
    Ok(())
}

/// The deletes of a rotation and of a cleanup, for `worker`, before its push: a rotation's stop the
/// deploy when one fails (a marker left on the live key would retire it, and a revoke that left
/// the old key would leave a leaked key working); a cleanup's only warn, since the runtime has
/// already stopped trusting the slot.
pub(crate) fn apply_edge_deletes(
    api: &dyn CloudflareApi,
    secrets: &Secrets<'_>,
    worker: &str,
    environment: &str,
) -> Result<(), super::DeployFailure> {
    let required = rotation_deletes(secrets, worker);
    delete_secrets(api, environment, worker, &required).map_err(|(name, why)| {
        super::DeployFailure::driver(format!(
            "could not delete the secret `{name}` on `{}`: {why}.\n  A rotation's marker or a revoked key is left in place, so run the deploy again.",
            deployed_worker_name(environment, worker)
        ))
    })?;
    for expired in secrets.expired.get(worker).into_iter().flatten() {
        if let Err((name, why)) = delete_secrets(api, environment, worker, &expired.deletes) {
            eprintln!(
                "bynk: warning: could not delete the retired edge-key secret `{name}` on `{}`: {why}. The runtime already refuses it; it is deleted on a later deploy.",
                deployed_worker_name(environment, worker)
            );
        }
    }
    Ok(())
}

/// After `worker` was pushed: record what its rotations left retiring, and forget what was cleaned
/// up. The ledger is the cleanup's source of truth, and the runtime does not read it.
pub(crate) fn record_edge_slots(
    lock: &mut DeployLock,
    environment: &str,
    worker: &str,
    decisions: (&BTreeMap<Edge, EdgeKey>, &BTreeMap<String, Vec<Expired>>),
) {
    for (edge, decision) in decisions.0 {
        let EdgeKey::Set { callee: write, .. } = decision else {
            continue;
        };
        if edge.callee != worker {
            continue;
        }
        let Ok(keys) = edge.keys() else { continue };
        for key in keys.on_callee {
            let retiring = write
                .retiring()
                .map(|(slot, until)| Retiring { slot, until });
            lock.record_retiring_edge_key(environment, worker, &key, retiring);
        }
    }
    for expired in decisions.1.get(worker).into_iter().flatten() {
        lock.record_retiring_edge_key(environment, worker, &expired.slot_one_key, None);
    }
}

impl super::ledger::DeployLock {
    /// The slots a rotation is retiring on `worker` in `environment`, by the name of each edge's
    /// first slot.
    pub(crate) fn retiring_edge_keys(
        &self,
        environment: &str,
        worker: &str,
    ) -> std::collections::BTreeMap<String, Retiring> {
        self.environments
            .get(environment)
            .and_then(|env| env.retiring_edge_keys.get(worker))
            .cloned()
            .unwrap_or_default()
    }

    /// Record that `worker`'s edge key `slot_one_key` is retiring `slot` until `until`, or (`None`)
    /// that nothing is.
    pub(crate) fn record_retiring_edge_key(
        &mut self,
        environment: &str,
        worker: &str,
        slot_one_key: &str,
        retiring: Option<Retiring>,
    ) {
        let env = self
            .environments
            .entry(environment.to_string())
            .or_default();
        match retiring {
            Some(retiring) => {
                env.retiring_edge_keys
                    .entry(worker.to_string())
                    .or_default()
                    .insert(slot_one_key.to_string(), retiring);
            }
            None => {
                if let Some(keys) = env.retiring_edge_keys.get_mut(worker) {
                    keys.remove(slot_one_key);
                    if keys.is_empty() {
                        env.retiring_edge_keys.remove(worker);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000;
    const UNTIL: u64 = NOW + EDGE_KEY_GRACE_SECONDS;

    fn state(key: [bool; 2], until: [bool; 2]) -> SlotState {
        SlotState::of(key, until)
    }

    #[test]
    fn the_grace_window_outlasts_the_queue_retention() {
        // Queue retention is at most 14 days; the window is that and a day of retries.
        assert_eq!(EDGE_KEY_GRACE_SECONDS, 15 * 24 * 3600);
        const { assert!(EDGE_KEY_GRACE_SECONDS > 14 * 24 * 3600) };
    }

    #[test]
    fn names_are_derived_from_the_first_slot() {
        let names = SlotNames::of("BYNK_CALL_FROM_SHOP__ORDERS").unwrap();
        assert_eq!(names.key(Slot::One), "BYNK_CALL_FROM_SHOP__ORDERS");
        assert_eq!(names.key(Slot::Two), "BYNK_CALL_FROM2_SHOP__ORDERS");
        assert_eq!(names.until(Slot::One), "BYNK_CALL_UNTIL_SHOP__ORDERS");
        assert_eq!(names.until(Slot::Two), "BYNK_CALL_UNTIL2_SHOP__ORDERS");
        assert_eq!(SlotNames::of("BYNK_CALL_TO_SHOP__ORDERS"), None);
    }

    #[test]
    fn a_first_mint_puts_the_key_in_the_first_slot_and_retires_nothing() {
        let write = plan_write(state([false, false], [false, false]), Mode::Grace, NOW);
        assert_eq!(write.ops, [SlotOp::PutKey(Slot::One)]);
    }

    #[test]
    fn a_rotation_puts_the_new_key_in_the_free_slot_and_marks_the_old_one_retiring() {
        let from_one = plan_write(state([true, false], [false, false]), Mode::Grace, NOW);
        assert_eq!(
            from_one.ops,
            [
                SlotOp::PutKey(Slot::Two),
                SlotOp::PutUntil(Slot::One, UNTIL)
            ]
        );
        assert_eq!(from_one.retiring(), Some((Slot::One, UNTIL)));
        // The next rotation, once the first window has been cleaned up, goes back the other way.
        let from_two = plan_write(state([false, true], [false, false]), Mode::Grace, NOW);
        assert_eq!(
            from_two.ops,
            [
                SlotOp::PutKey(Slot::One),
                SlotOp::PutUntil(Slot::Two, UNTIL)
            ]
        );
    }

    #[test]
    fn a_rotation_inside_a_window_replaces_the_retiring_key_and_never_extends_it() {
        // Live in the first slot, the second retiring: the retiring key is replaced, its marker
        // deleted, and the live key is marked.
        let write = plan_write(state([true, true], [false, true]), Mode::Grace, NOW);
        assert_eq!(
            write.ops,
            [
                SlotOp::PutKey(Slot::Two),
                SlotOp::PutUntil(Slot::One, UNTIL),
                SlotOp::DeleteUntil(Slot::Two)
            ]
        );
        // Live in the second, the first retiring.
        let write = plan_write(state([true, true], [true, false]), Mode::Grace, NOW);
        assert_eq!(
            write.ops,
            [
                SlotOp::PutKey(Slot::One),
                SlotOp::PutUntil(Slot::Two, UNTIL),
                SlotOp::DeleteUntil(Slot::One)
            ]
        );
    }

    #[test]
    fn two_live_keys_from_an_interrupted_rotation_keep_the_first_and_replace_the_second() {
        let write = plan_write(state([true, true], [false, false]), Mode::Grace, NOW);
        assert_eq!(
            write.ops,
            [
                SlotOp::PutKey(Slot::Two),
                SlotOp::PutUntil(Slot::One, UNTIL)
            ]
        );
    }

    #[test]
    fn a_stray_marker_on_the_slot_a_key_goes_in_is_deleted_after_the_put() {
        let write = plan_write(state([true, false], [false, true]), Mode::Grace, NOW);
        assert_eq!(
            write.ops,
            [
                SlotOp::PutKey(Slot::Two),
                SlotOp::PutUntil(Slot::One, UNTIL),
                SlotOp::DeleteUntil(Slot::Two)
            ]
        );
    }

    #[test]
    fn a_revoke_leaves_one_live_key_and_nothing_else() {
        let write = plan_write(state([true, true], [false, true]), Mode::Revoke, NOW);
        assert_eq!(
            write.ops,
            [
                SlotOp::PutKey(Slot::One),
                SlotOp::DeleteKey(Slot::Two),
                SlotOp::DeleteUntil(Slot::Two)
            ]
        );
        // With a key in the second slot only, that slot is the live one and takes the new key.
        let write = plan_write(state([false, true], [false, false]), Mode::Revoke, NOW);
        assert_eq!(write.ops, [SlotOp::PutKey(Slot::Two)]);
        // Nothing held: a first mint.
        let write = plan_write(state([false, false], [false, false]), Mode::Revoke, NOW);
        assert_eq!(write.ops, [SlotOp::PutKey(Slot::One)]);
        assert!(
            write.retiring().is_none(),
            "a revoke marks nothing retiring"
        );
    }

    #[test]
    fn the_puts_and_deletes_are_named_and_ordered() {
        let names = SlotNames::of("BYNK_CALL_FROM_A__B").unwrap();
        let write = plan_write(state([true, true], [false, true]), Mode::Grace, NOW);
        assert_eq!(
            write.puts(&names, "k"),
            [
                ("BYNK_CALL_FROM2_A__B".to_string(), "k".to_string()),
                ("BYNK_CALL_UNTIL_A__B".to_string(), UNTIL.to_string())
            ],
            "the key before the marker"
        );
        assert_eq!(write.deletes(&names), ["BYNK_CALL_UNTIL2_A__B"]);
        let revoke = plan_write(state([true, true], [true, true]), Mode::Revoke, NOW);
        assert_eq!(
            revoke.deletes(&names),
            [
                "BYNK_CALL_FROM2_A__B",
                "BYNK_CALL_UNTIL_A__B",
                "BYNK_CALL_UNTIL2_A__B"
            ],
            "a slot's key before its marker"
        );
    }

    #[test]
    fn a_cleanup_deletes_the_key_then_the_marker_of_what_is_present() {
        let both = retire_write(state([true, true], [true, false]), Slot::One);
        assert_eq!(
            both.ops,
            [SlotOp::DeleteKey(Slot::One), SlotOp::DeleteUntil(Slot::One)]
        );
        // Already gone: nothing to delete, so nothing is asked of Cloudflare.
        let gone = retire_write(state([false, true], [false, false]), Slot::One);
        assert!(gone.ops.is_empty());
    }

    #[test]
    fn a_date_is_written_in_utc() {
        assert_eq!(utc_date(0), "1970-01-01");
        assert_eq!(utc_date(951_782_400), "2000-02-29");
        assert_eq!(utc_date(1_800_000_000), "2027-01-15");
        assert_eq!(
            utc_date(1_800_000_000 + EDGE_KEY_GRACE_SECONDS),
            "2027-01-30"
        );
    }

    #[test]
    fn the_state_comes_from_the_listing() {
        let names = SlotNames::of("BYNK_CALL_FROM_A__B").unwrap();
        let listed: BTreeSet<String> = ["BYNK_CALL_FROM2_A__B", "BYNK_CALL_UNTIL_A__B"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let s = SlotState::from_listed(&names, true, Some(&listed));
        assert_eq!(s, SlotState::of([true, true], [true, false]));
        // An unread listing knows nothing of the other three names.
        let unread = SlotState::from_listed(&names, true, None);
        assert_eq!(unread, SlotState::of([true, false], [false, false]));
    }

    // ---- the deploy: the decisions, the order they are applied in, and the ledger ----

    use crate::cloudflare::fake::Recording;
    use crate::deploy::config::Resources;
    use crate::deploy::secrets::{SecretSource, apply_secrets, prepare_bindings};

    const CALLEE: &str = "shop-payment";
    const CALLER: &str = "shop-orders";
    const FROM: &str = "BYNK_CALL_FROM_SHOP__ORDERS";
    const TO: &str = "BYNK_CALL_TO_SHOP__PAYMENT";

    fn edge() -> Edge {
        Edge {
            caller: CALLER.into(),
            callee: CALLEE.into(),
            caller_contexts: Vec::new(),
            callee_contexts: Vec::new(),
        }
    }

    fn names(of: &[&str]) -> Option<BTreeSet<String>> {
        Some(of.iter().map(|s| s.to_string()).collect())
    }

    /// A ledger that has pushed both Workers, so neither is a first deploy.
    fn deployed_both() -> DeployLock {
        let mut lock = DeployLock::default();
        lock.record_deployed("default", CALLEE, None);
        lock.record_deployed("default", CALLER, None);
        lock
    }

    fn options(force: bool, revoke: &[&str]) -> DeployOptions {
        DeployOptions {
            environment: "default".into(),
            force,
            revoke_edge_keys: revoke.iter().map(|s| s.to_string()).collect(),
            ..DeployOptions::default()
        }
    }

    fn decide(
        listed: &BTreeMap<String, Option<BTreeSet<String>>>,
        lock: &DeployLock,
        opts: &DeployOptions,
        in_run: &[&str],
    ) -> Result<EdgeDecisions, ExitCode> {
        let in_run: BTreeSet<&str> = in_run.iter().copied().collect();
        decide_edge_keys(&[edge()], &in_run, listed, lock, opts, NOW)
    }

    /// Apply the decisions Worker by Worker, in the order given (a callee before its caller, as the
    /// upload order has it), through a recording fake: the calls, and what each put was sent.
    fn apply_in_order(
        order: &[&str],
        decisions: &EdgeDecisions,
        listed: &BTreeMap<String, Option<BTreeSet<String>>>,
        lock: &DeployLock,
    ) -> (Vec<String>, Vec<String>) {
        let api = Recording::new(|_op| Ok::<_, crate::cloudflare::WranglerError>(String::new()));
        let source = SecretSource::default();
        let (foreign, foreign_secrets) = (BTreeMap::new(), BTreeSet::new());
        let mut resolved = BTreeMap::new();
        for worker in order {
            let mut secrets = Secrets {
                source: &source,
                force: false,
                edge_keys: &decisions.keys,
                expired: &decisions.expired,
                foreign_keys: &foreign,
                foreign_secrets: &foreign_secrets,
                listed,
                resolved: &mut resolved,
                with_upload: true,
            };
            let first = !lock.is_deployed("default", worker);
            let prepared = prepare_bindings(
                &api,
                Path::new("."),
                worker,
                &Resources::default(),
                &mut secrets,
                first,
                "default",
            )
            .unwrap();
            apply_secrets(&api, Path::new("."), worker, &prepared, "default").unwrap();
            apply_edge_deletes(&api, &secrets, worker, "default").unwrap();
        }
        (api.calls(), api.stdin())
    }

    fn held_both() -> BTreeMap<String, Option<BTreeSet<String>>> {
        BTreeMap::from([
            (CALLEE.to_string(), names(&[FROM])),
            (CALLER.to_string(), names(&[TO])),
        ])
    }

    #[test]
    fn a_forced_rotation_writes_the_consumers_slot_before_the_producer_signs_with_the_new_key() {
        let listed = held_both();
        let lock = deployed_both();
        let decisions = decide(&listed, &lock, &options(true, &[]), &[CALLEE, CALLER]).unwrap();
        let (calls, stdin) = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock);
        assert_eq!(
            calls,
            [
                "secret put BYNK_CALL_FROM2_SHOP__ORDERS",
                "secret put BYNK_CALL_UNTIL_SHOP__ORDERS",
                "secret put BYNK_CALL_TO_SHOP__PAYMENT",
            ],
            "the consumer's new slot, then the marker retiring the old one, then the producer's key"
        );
        // The key goes to the consumer's slot and the producer's key as one value; the marker is the
        // instant the old key stops verifying, fifteen days on.
        assert_eq!(stdin.len(), 3);
        assert_eq!(stdin[0], stdin[2]);
        assert_eq!(stdin[0].len(), 64, "a minted edge key: 32 bytes of hex");
        assert_eq!(stdin[1], (NOW + EDGE_KEY_GRACE_SECONDS).to_string());
    }

    #[test]
    fn a_rotation_records_the_retiring_slot_and_a_later_deploy_past_it_deletes_the_old_key() {
        let listed = held_both();
        let mut lock = deployed_both();
        let decisions = decide(&listed, &lock, &options(true, &[]), &[CALLEE, CALLER]).unwrap();
        record_edge_slots(
            &mut lock,
            "default",
            CALLEE,
            (&decisions.keys, &decisions.expired),
        );
        let recorded = lock.retiring_edge_keys("default", CALLEE);
        assert_eq!(
            recorded.get(FROM),
            Some(&Retiring {
                slot: Slot::One,
                until: NOW + EDGE_KEY_GRACE_SECONDS
            })
        );
        // The ledger holds a slot and a time, never a key.
        let text = toml::to_string(&lock).unwrap();
        assert!(text.contains("retiring_edge_keys"), "{text}");
        assert!(!text.contains(&"0".repeat(32)), "{text}");

        // Before the instant a plain deploy keeps the edge and cleans nothing.
        let listed_after = BTreeMap::from([
            (
                CALLEE.to_string(),
                names(&[
                    FROM,
                    "BYNK_CALL_FROM2_SHOP__ORDERS",
                    "BYNK_CALL_UNTIL_SHOP__ORDERS",
                ]),
            ),
            (CALLER.to_string(), names(&[TO])),
        ]);
        let in_run: BTreeSet<&str> = [CALLEE, CALLER].into_iter().collect();
        let before = decide_edge_keys(
            &[edge()],
            &in_run,
            &listed_after,
            &lock,
            &options(false, &[]),
            NOW + EDGE_KEY_GRACE_SECONDS - 1,
        )
        .unwrap();
        assert!(before.expired.is_empty());
        // At it, the old key and its marker are deleted, the key first, and the ledger forgets it.
        let past = decide_edge_keys(
            &[edge()],
            &in_run,
            &listed_after,
            &lock,
            &options(false, &[]),
            NOW + EDGE_KEY_GRACE_SECONDS,
        )
        .unwrap();
        let (calls, _) = apply_in_order(&[CALLEE, CALLER], &past, &listed_after, &lock);
        assert_eq!(
            calls,
            [
                "secret delete BYNK_CALL_FROM_SHOP__ORDERS --name shop-payment",
                "secret delete BYNK_CALL_UNTIL_SHOP__ORDERS --name shop-payment",
            ]
        );
        record_edge_slots(&mut lock, "default", CALLEE, (&past.keys, &past.expired));
        assert!(lock.retiring_edge_keys("default", CALLEE).is_empty());
    }

    #[test]
    fn a_plain_deploy_that_keeps_the_edge_touches_no_slot() {
        let listed = held_both();
        let lock = deployed_both();
        let decisions = decide(&listed, &lock, &options(false, &[]), &[CALLEE, CALLER]).unwrap();
        assert_eq!(decisions.keys[&edge()], EdgeKey::Keep);
        let (calls, _) = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock);
        assert!(calls.is_empty(), "{calls:?}");
    }

    #[test]
    fn a_producer_redeployed_after_it_was_deleted_rotates_with_the_old_key_kept_on_the_consumer() {
        // The producer's Worker was deleted: the ledger does not have it as deployed, so it holds
        // nothing, while the consumer holds the old key.
        let listed = BTreeMap::from([
            (CALLEE.to_string(), names(&[FROM])),
            (CALLER.to_string(), names(&[])),
        ]);
        let mut lock = DeployLock::default();
        lock.record_deployed("default", CALLEE, None);
        let decisions = decide(&listed, &lock, &options(false, &[]), &[CALLEE, CALLER]).unwrap();
        let EdgeKey::Set {
            callee, rotating, ..
        } = &decisions.keys[&edge()]
        else {
            panic!("a producer with no key is re-keyed");
        };
        assert!(*rotating, "the consumer held a key");
        assert_eq!(
            callee.ops,
            [
                SlotOp::PutKey(Slot::Two),
                SlotOp::PutUntil(Slot::One, NOW + EDGE_KEY_GRACE_SECONDS)
            ],
            "messages the old producer signed and left in the queue are still admitted"
        );
    }

    #[test]
    fn a_consumer_redeployed_after_it_was_deleted_starts_with_one_live_key() {
        let listed = BTreeMap::from([
            (CALLEE.to_string(), names(&[])),
            (CALLER.to_string(), names(&[TO])),
        ]);
        let mut lock = DeployLock::default();
        lock.record_deployed("default", CALLER, None);
        let decisions = decide(&listed, &lock, &options(false, &[]), &[CALLEE, CALLER]).unwrap();
        let EdgeKey::Set { callee, .. } = &decisions.keys[&edge()] else {
            panic!("a consumer with no key is re-keyed");
        };
        assert_eq!(
            callee.ops,
            [SlotOp::PutKey(Slot::One)],
            "there is no old key on it to keep"
        );
    }

    #[test]
    fn a_revoke_drops_the_previous_key_at_once_and_says_so() {
        // The consumer is mid-rotation: the live key in the second slot, the first retiring.
        let listed = BTreeMap::from([
            (
                CALLEE.to_string(),
                names(&[
                    FROM,
                    "BYNK_CALL_FROM2_SHOP__ORDERS",
                    "BYNK_CALL_UNTIL_SHOP__ORDERS",
                ]),
            ),
            (CALLER.to_string(), names(&[TO])),
        ]);
        let lock = deployed_both();
        // No `--force`: the revoke alone re-keys the edge, naming either end's context.
        for context in ["shop.orders", "shop.payment"] {
            let decisions = decide(
                &listed,
                &lock,
                &options(false, &[context]),
                &[CALLEE, CALLER],
            )
            .unwrap();
            let EdgeKey::Set { callee, .. } = &decisions.keys[&edge()] else {
                panic!("a revoked edge is re-keyed");
            };
            assert!(callee.retiring().is_none(), "a revoke retires nothing");
            let (calls, _) = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock);
            assert_eq!(
                calls,
                [
                    "secret put BYNK_CALL_FROM2_SHOP__ORDERS",
                    "secret delete BYNK_CALL_FROM_SHOP__ORDERS --name shop-payment",
                    "secret delete BYNK_CALL_UNTIL_SHOP__ORDERS --name shop-payment",
                    "secret put BYNK_CALL_TO_SHOP__PAYMENT",
                ],
                "{context}: the new key in the live slot, the old one and its marker gone, then the producer"
            );
        }
    }

    #[test]
    fn a_revoke_that_cannot_re_key_the_edge_is_refused_not_ignored() {
        let listed = held_both();
        let lock = deployed_both();
        // A `--context` run that holds only the consumer cannot copy a key to the producer.
        let only_callee = decide(&listed, &lock, &options(false, &["shop.orders"]), &[CALLEE]);
        assert!(only_callee.is_err());
        // A context with no edge in the run is refused too.
        let unknown = decide(
            &listed,
            &lock,
            &options(false, &["shop.nobody"]),
            &[CALLEE, CALLER],
        );
        assert!(unknown.is_err());
    }

    #[test]
    fn the_notice_names_the_rotation_and_its_window_or_the_revoke() {
        let grace =
            rotation_notice("default", &edge(), Mode::Grace, Some((Slot::One, UNTIL))).unwrap();
        assert!(grace.contains("15 days"), "{grace}");
        assert!(grace.contains("until 2027-01-30"), "{grace}");
        assert!(grace.contains("--revoke-previous-edge-key"), "{grace}");
        let revoke = rotation_notice("default", &edge(), Mode::Revoke, None).unwrap();
        assert!(revoke.contains("at once"), "{revoke}");
        assert_eq!(rotation_notice("default", &edge(), Mode::Grace, None), None);
    }
}
