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

/// What the callee holds for one edge key: which slots carry a key and which a retirement marker,
/// and whether the listing it comes from was read.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SlotState {
    key: [bool; 2],
    until: [bool; 2],
    /// The Worker's secret listing was read. Without it only the first slot is known (from the
    /// decision's own answer), and a rotation cannot tell which slot holds the live key.
    known: bool,
}

impl SlotState {
    /// The state from the Worker's secret listing. `slot_one_unread` is the decision's answer for
    /// the first slot, used only when the listing could not be read (an unread listing counts as
    /// holding it, as for any edge key).
    pub(crate) fn from_listed(
        names: &SlotNames,
        slot_one_unread: bool,
        listed: Option<&BTreeSet<String>>,
    ) -> SlotState {
        let has = |name: String| listed.is_some_and(|set| set.contains(&name));
        SlotState {
            key: [
                listed.map_or(slot_one_unread, |set| set.contains(&names.key(Slot::One))),
                has(names.key(Slot::Two)),
            ],
            until: [has(names.until(Slot::One)), has(names.until(Slot::Two))],
            known: listed.is_some(),
        }
    }

    /// A state for a test, from a read listing.
    #[cfg(test)]
    pub(crate) fn of(key: [bool; 2], until: [bool; 2]) -> SlotState {
        SlotState {
            key,
            until,
            known: true,
        }
    }

    fn holds(&self, slot: Slot) -> bool {
        self.key[slot.index()]
    }

    fn retiring(&self, slot: Slot) -> bool {
        self.until[slot.index()]
    }

    /// The slots that hold a key and no marker: the ones the runtime verifies without a deadline.
    fn live(&self) -> Vec<Slot> {
        [Slot::One, Slot::Two]
            .into_iter()
            .filter(|s| self.holds(*s) && !self.retiring(*s))
            .collect()
    }

    /// The slots that hold a key and a marker.
    fn retiring_keys(&self) -> Vec<Slot> {
        [Slot::One, Slot::Two]
            .into_iter()
            .filter(|s| self.holds(*s) && self.retiring(*s))
            .collect()
    }

    fn any_key(&self) -> bool {
        self.key[0] || self.key[1]
    }

    /// A key is held but none is live: every key there is retiring, so the edge stops working when
    /// the last one expires. A rotation that stopped between marking the old key retiring and putting
    /// the new one leaves exactly this, and it is repaired by the next deploy, not left to expire.
    pub(crate) fn needs_repair(&self) -> bool {
        self.live().is_empty() && !self.retiring_keys().is_empty()
    }
}

/// Why a rotation was not planned. The edge is left as it is, because replacing a key here could
/// overwrite the one a producer is still signing with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SlotRefusal {
    /// The callee's secret listing could not be read, so which slot holds the live key is not known.
    Unread,
    /// The previous rotation is still inside its window: both slots hold a key, one of them retiring.
    WindowOpen,
    /// The slots are in a state no rotation leaves (two live keys, or two retiring ones).
    Ambiguous,
}

impl SlotRefusal {
    /// What to tell the user, after the edge's name.
    pub(crate) fn reason(self) -> &'static str {
        match self {
            SlotRefusal::Unread => {
                "the consumer's secrets could not be listed, so which of its two slots holds the edge key is not known, and a rotation could overwrite the key the producer signs with"
            }
            SlotRefusal::WindowOpen => {
                "its previous rotation is still inside its 15-day window (both of the consumer's slots hold a key), and rotating again would replace a key a producer may still sign with"
            }
            SlotRefusal::Ambiguous => {
                "the consumer's slots are in a state a rotation does not leave (two live keys, or two retiring ones), so which one the producer signs with is not known"
            }
        }
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
/// A grace rotation is planned only from a state it can reason about, and otherwise leaves the edge
/// alone ([`SlotRefusal`]): replacing the wrong slot would overwrite the key a producer still signs
/// with, which is the loss this exists to prevent.
///
/// - Nothing held: the key goes in the first slot.
/// - One live key: the other slot is free (a marker left on it is deleted first, since it holds no
///   key), the live one is marked retiring `now + grace`, **then** the new key is put. In that order a
///   failure between the two leaves one retiring key and no live one, which `needs_repair` finds and
///   the next deploy finishes, rather than two live keys, which no one could tell apart.
/// - Only retiring keys (a rotation that stopped, or one whose live key was lost): the new key goes in
///   the free slot, and the retiring one keeps its own instant, so the producer's key still verifies.
/// - A live key and a retiring one, or two of either: [`SlotRefusal::WindowOpen`] and
///   [`SlotRefusal::Ambiguous`].
///
/// [`Mode::Revoke`] needs a read listing, puts the key in the first slot, and deletes the second
/// slot's key and every marker, leaving one live key. It is idempotent, so a rerun after a failed
/// delete finishes it.
pub(crate) fn plan_write(
    state: SlotState,
    mode: Mode,
    now: u64,
) -> Result<CalleeWrite, SlotRefusal> {
    let mut ops = Vec::new();
    if !state.known && state.any_key() {
        return Err(SlotRefusal::Unread);
    }
    if mode == Mode::Revoke {
        ops.push(SlotOp::PutKey(Slot::One));
        if state.holds(Slot::Two) {
            ops.push(SlotOp::DeleteKey(Slot::Two));
        }
        for slot in [Slot::Two, Slot::One] {
            if state.retiring(slot) {
                ops.push(SlotOp::DeleteUntil(slot));
            }
        }
        return Ok(CalleeWrite { ops });
    }
    let (live, retiring) = (state.live(), state.retiring_keys());
    let free = |slot: Slot| !state.holds(slot);
    match (live.as_slice(), retiring.as_slice()) {
        ([], []) => {
            // Nothing held: a marker left on either slot would retire the key put there.
            for slot in [Slot::One, Slot::Two] {
                if state.retiring(slot) {
                    ops.push(SlotOp::DeleteUntil(slot));
                }
            }
            ops.push(SlotOp::PutKey(Slot::One));
        }
        ([old], []) => {
            let target = old.other();
            if state.retiring(target) {
                ops.push(SlotOp::DeleteUntil(target));
            }
            ops.push(SlotOp::PutUntil(
                *old,
                now.saturating_add(EDGE_KEY_GRACE_SECONDS),
            ));
            ops.push(SlotOp::PutKey(target));
        }
        ([], [kept]) if free(kept.other()) => {
            let target = kept.other();
            if state.retiring(target) {
                ops.push(SlotOp::DeleteUntil(target));
            }
            ops.push(SlotOp::PutKey(target));
        }
        ([_], [_]) => return Err(SlotRefusal::WindowOpen),
        _ => return Err(SlotRefusal::Ambiguous),
    }
    Ok(CalleeWrite { ops })
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

/// The callee's key names, each as the slot that holds it: a name whose first slot is absent but
/// whose second holds a key is held in the second. An edge's key is held when each of its names is,
/// in either slot, so a rotation that moved it to the second slot (and a cleanup that deleted the
/// first) does not read as an absent key.
pub(crate) fn held_key_names(
    on_callee: &[String],
    listed: Option<&BTreeSet<String>>,
) -> Vec<String> {
    on_callee
        .iter()
        .map(|name| {
            let in_second = SlotNames::of(name)
                .map(|n| n.key(Slot::Two))
                .filter(|second| {
                    listed.is_some_and(|set| set.contains(second) && !set.contains(name))
                });
            in_second.unwrap_or_else(|| name.clone())
        })
        .collect()
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

/// Printed when a rotation was not planned and the edge is left as it is.
pub(crate) fn refusal_notice(environment: &str, edge: &Edge, refusal: SlotRefusal) -> String {
    format!(
        "bynk: warning: the edge key {} was not rotated: {}.\n  Rotate it after the window ends, or drop the previous key and re-key at once with `{} --revoke-previous-edge-key <context>`.\n",
        edge.describe(environment),
        refusal.reason(),
        deploy_command(environment)
    )
}

/// Decide each edge's key over both its ends, and the retiring slots whose window has passed
/// (#4526). Each edge in `revoked` (`--revoke-previous-edge-key`) is re-keyed with no grace; one
/// that cannot be (a `--context` run that does not hold both ends, or a callee whose slots cannot
/// be read) is refused, since a revoke that quietly did nothing would leave a leaked key working.
/// An edge whose keys are all retiring is re-keyed without being asked: it would stop working when
/// the last expired.
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
        let callee_listing = listed.get(&edge.callee).and_then(Option::as_ref);
        let held_names = held_key_names(&names.on_callee, callee_listing);
        let held = |worker: &str, keys: &[String]| {
            end_presence(edge, worker, keys, listed, lock, &opts.environment)
        };
        let callee = held(&edge.callee, &held_names);
        let caller = held(&edge.caller, &names.on_caller);
        let both_in_run =
            in_run.contains(edge.caller.as_str()) && in_run.contains(edge.callee.as_str());
        let revoke = opts.revoke_edge_keys.iter().any(|c| edge.involves(c));
        let states: Vec<(String, SlotState)> = names
            .on_callee
            .iter()
            .filter_map(|name| {
                SlotNames::of(name).map(|n| {
                    (
                        name.clone(),
                        SlotState::from_listed(&n, callee != Presence::Absent, callee_listing),
                    )
                })
            })
            .collect();
        let repair = states.iter().any(|(_, state)| state.needs_repair());
        let mode = if revoke { Mode::Revoke } else { Mode::Grace };
        let rotation = Rotation {
            callee_slots: &states,
            mode,
            now,
            environment: &opts.environment,
        };
        let decision = decide_edge_key(
            edge,
            caller,
            callee,
            both_in_run,
            opts.force || revoke || repair,
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
                callee: writes,
                ..
            } => {
                let retiring = writes.iter().find_map(|(_, w)| w.retiring());
                if let Some(notice) = rotation_notice(&opts.environment, edge, mode, retiring) {
                    colour::eprint(&notice);
                }
            }
            EdgeKey::Refused(refusal) => {
                colour::eprint(&refusal_notice(&opts.environment, edge, *refusal));
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
                callee_listing,
                lock,
                (opts, now),
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
    callee_listing: Option<&BTreeSet<String>>,
    lock: &DeployLock,
    (opts, now): (&DeployOptions, u64),
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
        let state = SlotState::from_listed(&names, false, callee_listing);
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

/// The secrets a rotation deletes on `worker`: a marker left on a slot a key goes in, and everything
/// a revoke drops. These are not tidiness: a marker left on the live key would retire it, and a
/// revoke that left the old key would leave a leaked key working, so a failure stops the deploy.
pub(crate) fn rotation_deletes(secrets: &Secrets<'_>, worker: &str) -> Vec<String> {
    let mut names = Vec::new();
    for (edge, decision) in secrets.edge_keys {
        let EdgeKey::Set { callee, .. } = decision else {
            continue;
        };
        if edge.callee != worker {
            continue;
        }
        for (key, write) in callee {
            if let Some(slot_names) = SlotNames::of(key) {
                names.extend(write.deletes(&slot_names));
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
/// the old key would leave a leaked key working); a cleanup's only warn, and the slot is kept in the
/// ledger (`secrets.cleaned` records only what was deleted) so a later deploy tries it again.
pub(crate) fn apply_edge_deletes(
    api: &dyn CloudflareApi,
    secrets: &Secrets<'_>,
    worker: &str,
    environment: &str,
) -> Result<(), super::DeployFailure> {
    let deployed = deployed_worker_name(environment, worker);
    let required = rotation_deletes(secrets, worker);
    delete_secrets(api, environment, worker, &required).map_err(|(name, why)| {
        super::DeployFailure::driver(format!(
            "could not delete the secret `{name}` on `{deployed}`: {why}.\n  The edge key's slots on `{deployed}` are part-way through a rotation. Nothing has been lost: the producer still signs with the key it had, which the consumer still accepts. Run `{} --force` again to finish it (a revoke: run the same `--revoke-previous-edge-key` again).",
            deploy_command(environment)
        ))
    })?;
    for expired in secrets.expired.get(worker).into_iter().flatten() {
        match delete_secrets(api, environment, worker, &expired.deletes) {
            Ok(()) => {
                secrets
                    .cleaned
                    .borrow_mut()
                    .insert((worker.to_string(), expired.slot_one_key.clone()));
            }
            Err((name, why)) => eprintln!(
                "bynk: warning: could not delete the retired edge-key secret `{name}` on `{deployed}`: {why}. The runtime already refuses it; the next deploy tries again."
            ),
        }
    }
    Ok(())
}

/// After `worker` was pushed: record what its rotations left retiring, and forget what was cleaned
/// up (`cleaned`: only the slots whose deletes succeeded). The ledger is the cleanup's source of
/// truth, and the runtime does not read it.
pub(crate) fn record_edge_slots(
    lock: &mut DeployLock,
    environment: &str,
    worker: &str,
    decisions: (&BTreeMap<Edge, EdgeKey>, &BTreeSet<(String, String)>),
) {
    for (edge, decision) in decisions.0 {
        let EdgeKey::Set { callee, .. } = decision else {
            continue;
        };
        if edge.callee != worker {
            continue;
        }
        for (key, write) in callee {
            let retiring = write
                .retiring()
                .map(|(slot, until)| Retiring { slot, until });
            lock.record_retiring_edge_key(environment, worker, key, retiring);
        }
    }
    for (cleaned_worker, key) in decisions.1 {
        if cleaned_worker == worker {
            lock.record_retiring_edge_key(environment, worker, key, None);
        }
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
    use crate::cloudflare::fake::{Recording, failed};
    use crate::deploy::config::Resources;
    use crate::deploy::secrets::{SecretSource, apply_secrets, prepare_bindings};

    const NOW: u64 = 1_800_000_000;
    const UNTIL: u64 = NOW + EDGE_KEY_GRACE_SECONDS;

    fn state(key: [bool; 2], until: [bool; 2]) -> SlotState {
        SlotState::of(key, until)
    }

    fn grace(state: SlotState) -> Result<Vec<SlotOp>, SlotRefusal> {
        plan_write(state, Mode::Grace, NOW).map(|w| w.ops)
    }

    fn revoke(state: SlotState) -> Result<Vec<SlotOp>, SlotRefusal> {
        plan_write(state, Mode::Revoke, NOW).map(|w| w.ops)
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
        assert_eq!(
            grace(state([false, false], [false, false])),
            Ok(vec![SlotOp::PutKey(Slot::One)])
        );
        // A marker left on a slot that holds nothing would retire the key put there: deleted first.
        assert_eq!(
            grace(state([false, false], [true, false])),
            Ok(vec![
                SlotOp::DeleteUntil(Slot::One),
                SlotOp::PutKey(Slot::One)
            ])
        );
    }

    #[test]
    fn a_rotation_marks_the_live_key_retiring_before_the_new_key_is_put_in_the_free_slot() {
        assert_eq!(
            grace(state([true, false], [false, false])),
            Ok(vec![
                SlotOp::PutUntil(Slot::One, UNTIL),
                SlotOp::PutKey(Slot::Two)
            ]),
            "the marker first: a failure between leaves one retiring key and no live one, which is repaired, not two live keys no one can tell apart"
        );
        // The next rotation, once the first window has been cleaned up, goes back the other way.
        assert_eq!(
            grace(state([false, true], [false, false])),
            Ok(vec![
                SlotOp::PutUntil(Slot::Two, UNTIL),
                SlotOp::PutKey(Slot::One)
            ])
        );
        // A stray marker on the free slot, which holds no key, is deleted before anything is put.
        assert_eq!(
            grace(state([true, false], [false, true])),
            Ok(vec![
                SlotOp::DeleteUntil(Slot::Two),
                SlotOp::PutUntil(Slot::One, UNTIL),
                SlotOp::PutKey(Slot::Two)
            ])
        );
    }

    #[test]
    fn an_edge_whose_keys_are_all_retiring_gets_a_new_key_in_the_free_slot_and_keeps_the_old_one() {
        // A rotation that stopped after marking the live key retiring: the producer still signs
        // under it, so it keeps its own instant, and the new key goes in the free slot.
        let stopped = state([true, false], [true, false]);
        assert!(stopped.needs_repair());
        assert_eq!(grace(stopped), Ok(vec![SlotOp::PutKey(Slot::Two)]));
        let stopped = state([false, true], [false, true]);
        assert_eq!(grace(stopped), Ok(vec![SlotOp::PutKey(Slot::One)]));
        // A live key is not a repair.
        assert!(!state([true, false], [false, false]).needs_repair());
        assert!(!state([false, false], [false, false]).needs_repair());
    }

    #[test]
    fn a_rotation_inside_a_window_or_from_a_state_no_rotation_leaves_changes_nothing() {
        // Live in one slot, retiring in the other: replacing either could overwrite the key a
        // producer signs with.
        assert_eq!(
            grace(state([true, true], [false, true])),
            Err(SlotRefusal::WindowOpen)
        );
        assert_eq!(
            grace(state([true, true], [true, false])),
            Err(SlotRefusal::WindowOpen)
        );
        // Two live, or two retiring: which the producer signs with is not known.
        assert_eq!(
            grace(state([true, true], [false, false])),
            Err(SlotRefusal::Ambiguous)
        );
        assert_eq!(
            grace(state([true, true], [true, true])),
            Err(SlotRefusal::Ambiguous)
        );
    }

    #[test]
    fn a_listing_that_was_not_read_refuses_a_rotation_of_a_key_that_is_held() {
        let names = SlotNames::of("BYNK_CALL_FROM_A__B").unwrap();
        let unread = SlotState::from_listed(&names, true, None);
        assert_eq!(
            plan_write(unread, Mode::Grace, NOW),
            Err(SlotRefusal::Unread)
        );
        assert_eq!(
            plan_write(unread, Mode::Revoke, NOW),
            Err(SlotRefusal::Unread)
        );
        // Nothing held (a first deploy): there is nothing to overwrite.
        let nothing = SlotState::from_listed(&names, false, None);
        assert_eq!(
            plan_write(nothing, Mode::Grace, NOW).map(|w| w.ops),
            Ok(vec![SlotOp::PutKey(Slot::One)])
        );
    }

    #[test]
    fn a_revoke_leaves_one_live_key_and_nothing_else() {
        assert_eq!(
            revoke(state([true, true], [false, true])),
            Ok(vec![
                SlotOp::PutKey(Slot::One),
                SlotOp::DeleteKey(Slot::Two),
                SlotOp::DeleteUntil(Slot::Two)
            ])
        );
        // Whatever the state, even one a grace rotation refuses.
        assert_eq!(
            revoke(state([true, true], [true, true])),
            Ok(vec![
                SlotOp::PutKey(Slot::One),
                SlotOp::DeleteKey(Slot::Two),
                SlotOp::DeleteUntil(Slot::Two),
                SlotOp::DeleteUntil(Slot::One)
            ])
        );
        assert_eq!(
            revoke(state([false, true], [false, false])),
            Ok(vec![
                SlotOp::PutKey(Slot::One),
                SlotOp::DeleteKey(Slot::Two)
            ])
        );
        assert_eq!(
            revoke(state([false, false], [false, false])),
            Ok(vec![SlotOp::PutKey(Slot::One)])
        );
        let write = plan_write(state([true, false], [false, false]), Mode::Revoke, NOW).unwrap();
        assert!(
            write.retiring().is_none(),
            "a revoke marks nothing retiring"
        );
    }

    #[test]
    fn the_puts_and_deletes_are_named_and_ordered() {
        let names = SlotNames::of("BYNK_CALL_FROM_A__B").unwrap();
        let write = plan_write(state([true, false], [false, true]), Mode::Grace, NOW).unwrap();
        assert_eq!(
            write.puts(&names, "k"),
            [
                ("BYNK_CALL_UNTIL_A__B".to_string(), UNTIL.to_string()),
                ("BYNK_CALL_FROM2_A__B".to_string(), "k".to_string()),
            ],
            "the marker before the key"
        );
        assert_eq!(write.deletes(&names), ["BYNK_CALL_UNTIL2_A__B"]);
        let revoke = plan_write(state([true, true], [true, true]), Mode::Revoke, NOW).unwrap();
        assert_eq!(
            revoke.deletes(&names),
            [
                "BYNK_CALL_FROM2_A__B",
                "BYNK_CALL_UNTIL2_A__B",
                "BYNK_CALL_UNTIL_A__B"
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
        // The first slot is read from the listing, not taken from the decision's answer.
        let s = SlotState::from_listed(&names, true, Some(&listed));
        assert_eq!(s, SlotState::of([false, true], [true, false]));
        // An unread listing knows nothing of the other three names.
        let unread = SlotState::from_listed(&names, true, None);
        assert!(!unread.known);
        assert_eq!(unread.key, [true, false]);
    }

    #[test]
    fn an_edge_key_is_held_in_either_slot() {
        let names = vec![
            "BYNK_CALL_FROM_A__B".to_string(),
            "BYNK_CALL_FROM_C__D".to_string(),
        ];
        let listed: BTreeSet<String> = ["BYNK_CALL_FROM2_A__B", "BYNK_CALL_FROM_C__D"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            held_key_names(&names, Some(&listed)),
            ["BYNK_CALL_FROM2_A__B", "BYNK_CALL_FROM_C__D"],
            "the first slot's name unless only the second holds it"
        );
        let neither: BTreeSet<String> = BTreeSet::new();
        assert_eq!(held_key_names(&names, Some(&neither)), names);
        assert_eq!(held_key_names(&names, None), names);
    }

    // ---- the deploy: the decisions, the order they are applied in, and the ledger ----

    const CALLEE: &str = "shop-payment";
    const CALLER: &str = "shop-orders";
    const FROM: &str = "BYNK_CALL_FROM_SHOP__ORDERS";
    const FROM2: &str = "BYNK_CALL_FROM2_SHOP__ORDERS";
    const UNTIL1: &str = "BYNK_CALL_UNTIL_SHOP__ORDERS";
    const TO: &str = "BYNK_CALL_TO_SHOP__PAYMENT";

    type Listing = BTreeMap<String, Option<BTreeSet<String>>>;

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

    fn listing(callee: Option<&[&str]>, caller: &[&str]) -> Listing {
        BTreeMap::from([
            (CALLEE.to_string(), callee.and_then(names)),
            (CALLER.to_string(), names(caller)),
        ])
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

    fn decide_at(
        listed: &Listing,
        lock: &DeployLock,
        opts: &DeployOptions,
        in_run: &[&str],
        now: u64,
    ) -> Result<EdgeDecisions, ExitCode> {
        let in_run: BTreeSet<&str> = in_run.iter().copied().collect();
        decide_edge_keys(&[edge()], &in_run, listed, lock, opts, now)
    }

    fn decide(
        listed: &Listing,
        lock: &DeployLock,
        opts: &DeployOptions,
        in_run: &[&str],
    ) -> Result<EdgeDecisions, ExitCode> {
        decide_at(listed, lock, opts, in_run, NOW)
    }

    struct Applied {
        calls: Vec<String>,
        stdin: Vec<String>,
        cleaned: std::cell::RefCell<BTreeSet<(String, String)>>,
        failure: Option<String>,
    }

    /// Apply the decisions Worker by Worker, in the order given (a callee before its caller, as the
    /// upload order has it), through a recording fake: the calls, what each put was sent, and the
    /// slots a cleanup deleted. `fail_deletes` makes every delete fail.
    fn apply_in_order(
        order: &[&str],
        decisions: &EdgeDecisions,
        listed: &Listing,
        lock: &DeployLock,
        fail_deletes: bool,
    ) -> Applied {
        let api = Recording::new(move |op| {
            if fail_deletes
                && matches!(
                    op,
                    crate::cloudflare::api::Op::NamedSecret { verb: "delete", .. }
                )
            {
                return Err(failed(op, "", "boom"));
            }
            Ok(String::new())
        });
        let source = SecretSource::default();
        let (foreign, foreign_secrets) = (BTreeMap::new(), BTreeSet::new());
        let mut resolved = BTreeMap::new();
        let cleaned = std::cell::RefCell::new(BTreeSet::new());
        let mut failure = None;
        for worker in order {
            let mut secrets = Secrets {
                source: &source,
                force: false,
                edge_keys: &decisions.keys,
                expired: &decisions.expired,
                cleaned: &cleaned,
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
            if let Err(e) = apply_edge_deletes(&api, &secrets, worker, "default") {
                failure = Some(e.message);
                break;
            }
        }
        Applied {
            calls: api.calls(),
            stdin: api.stdin(),
            cleaned,
            failure,
        }
    }

    fn set_of(decisions: &EdgeDecisions) -> &Vec<(String, CalleeWrite)> {
        let EdgeKey::Set { callee, .. } = &decisions.keys[&edge()] else {
            panic!("the edge is re-keyed: {:?}", decisions.keys[&edge()]);
        };
        callee
    }

    #[test]
    fn a_forced_rotation_marks_the_old_key_then_writes_the_consumers_slot_before_the_producer_signs()
     {
        let listed = listing(Some(&[FROM]), &[TO]);
        let lock = deployed_both();
        let decisions = decide(&listed, &lock, &options(true, &[]), &[CALLEE, CALLER]).unwrap();
        let applied = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock, false);
        assert_eq!(
            applied.calls,
            [
                "secret put BYNK_CALL_UNTIL_SHOP__ORDERS",
                "secret put BYNK_CALL_FROM2_SHOP__ORDERS",
                "secret put BYNK_CALL_TO_SHOP__PAYMENT",
            ],
            "the old key retiring, the consumer's new slot, then the producer's key"
        );
        // The marker is the instant the old key stops verifying, fifteen days on; the key goes to the
        // consumer's slot and the producer as one value.
        assert_eq!(applied.stdin.len(), 3);
        assert_eq!(applied.stdin[0], UNTIL.to_string());
        assert_eq!(applied.stdin[1], applied.stdin[2]);
        assert_eq!(
            applied.stdin[1].len(),
            64,
            "a minted edge key: 32 bytes of hex"
        );
    }

    #[test]
    fn a_rotation_records_the_retiring_slot_and_the_cleanup_leaves_a_deploy_that_keeps_the_edge() {
        let listed = listing(Some(&[FROM]), &[TO]);
        let mut lock = deployed_both();
        let decisions = decide(&listed, &lock, &options(true, &[]), &[CALLEE, CALLER]).unwrap();
        record_edge_slots(
            &mut lock,
            "default",
            CALLEE,
            (&decisions.keys, &BTreeSet::new()),
        );
        assert_eq!(
            lock.retiring_edge_keys("default", CALLEE).get(FROM),
            Some(&Retiring {
                slot: Slot::One,
                until: UNTIL
            })
        );
        // The ledger holds a slot and a time, never a key.
        let text = toml::to_string(&lock).unwrap();
        assert!(text.contains("retiring_edge_keys"), "{text}");

        let mid = listing(Some(&[FROM, FROM2, UNTIL1]), &[TO]);
        let in_run = [CALLEE, CALLER];
        // Before the instant a plain deploy keeps the edge and cleans nothing.
        let before = decide_at(&mid, &lock, &options(false, &[]), &in_run, UNTIL - 1).unwrap();
        assert!(before.expired.is_empty());
        assert_eq!(before.keys[&edge()], EdgeKey::Keep);
        // At it, the old key and its marker are deleted, the key first, and the ledger forgets it.
        let past = decide_at(&mid, &lock, &options(false, &[]), &in_run, UNTIL).unwrap();
        let applied = apply_in_order(&in_run, &past, &mid, &lock, false);
        assert_eq!(
            applied.calls,
            [
                "secret delete BYNK_CALL_FROM_SHOP__ORDERS --name shop-payment",
                "secret delete BYNK_CALL_UNTIL_SHOP__ORDERS --name shop-payment",
            ]
        );
        record_edge_slots(
            &mut lock,
            "default",
            CALLEE,
            (&past.keys, &applied.cleaned.borrow()),
        );
        assert!(lock.retiring_edge_keys("default", CALLEE).is_empty());

        // The key now lives in the second slot alone. A plain deploy keeps the edge, and a
        // `--context` deploy of either end is not refused: the callee still holds its key.
        let after = listing(Some(&[FROM2]), &[TO]);
        let plain = decide(&after, &lock, &options(false, &[]), &in_run).unwrap();
        assert_eq!(
            plain.keys[&edge()],
            EdgeKey::Keep,
            "no rotation nobody asked for"
        );
        for only in [CALLEE, CALLER] {
            let one_end = decide(&after, &lock, &options(false, &[]), &[only]).unwrap();
            assert_eq!(one_end.keys[&edge()], EdgeKey::Keep, "--context {only}");
        }
        // And the next rotation goes back to the first slot.
        let next = decide(&after, &lock, &options(true, &[]), &in_run).unwrap();
        assert_eq!(
            set_of(&next)[0].1.ops,
            [
                SlotOp::PutUntil(Slot::Two, UNTIL),
                SlotOp::PutKey(Slot::One)
            ]
        );
    }

    #[test]
    fn a_cleanup_whose_delete_failed_stays_in_the_ledger_and_is_tried_again() {
        let mut lock = deployed_both();
        lock.record_retiring_edge_key(
            "default",
            CALLEE,
            FROM,
            Some(Retiring {
                slot: Slot::One,
                until: NOW,
            }),
        );
        let mid = listing(Some(&[FROM, FROM2, UNTIL1]), &[TO]);
        let in_run = [CALLEE, CALLER];
        let past = decide(&mid, &lock, &options(false, &[]), &in_run).unwrap();
        let failed_run = apply_in_order(&in_run, &past, &mid, &lock, true);
        assert!(
            failed_run.cleaned.borrow().is_empty(),
            "nothing was deleted"
        );
        record_edge_slots(
            &mut lock,
            "default",
            CALLEE,
            (&past.keys, &failed_run.cleaned.borrow()),
        );
        assert!(
            lock.retiring_edge_keys("default", CALLEE)
                .contains_key(FROM),
            "the slot is kept, so the next deploy tries it again"
        );
        // The next deploy finds it again.
        let again = decide(&mid, &lock, &options(false, &[]), &in_run).unwrap();
        assert_eq!(again.expired[CALLEE].len(), 1);
    }

    #[test]
    fn a_plain_deploy_that_keeps_the_edge_touches_no_slot() {
        let listed = listing(Some(&[FROM]), &[TO]);
        let lock = deployed_both();
        let decisions = decide(&listed, &lock, &options(false, &[]), &[CALLEE, CALLER]).unwrap();
        assert_eq!(decisions.keys[&edge()], EdgeKey::Keep);
        let applied = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock, false);
        assert!(applied.calls.is_empty(), "{:?}", applied.calls);
    }

    #[test]
    fn an_edge_whose_only_key_is_retiring_is_repaired_without_being_asked() {
        // A rotation stopped after marking the old key retiring: no live key, and the producer still
        // signs with the old one, which expires. A plain deploy finishes it.
        let listed = listing(Some(&[FROM, UNTIL1]), &[TO]);
        let lock = deployed_both();
        let decisions = decide(&listed, &lock, &options(false, &[]), &[CALLEE, CALLER]).unwrap();
        assert_eq!(set_of(&decisions)[0].1.ops, [SlotOp::PutKey(Slot::Two)]);
        let applied = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock, false);
        assert_eq!(
            applied.calls,
            [
                "secret put BYNK_CALL_FROM2_SHOP__ORDERS",
                "secret put BYNK_CALL_TO_SHOP__PAYMENT"
            ],
            "the old key keeps its instant; the producer takes the new one"
        );
    }

    #[test]
    fn a_rotation_inside_its_window_is_not_planned_and_replaces_no_key() {
        let listed = listing(Some(&[FROM, FROM2, UNTIL1]), &[TO]);
        let lock = deployed_both();
        let decisions = decide(&listed, &lock, &options(true, &[]), &[CALLEE, CALLER]).unwrap();
        assert_eq!(
            decisions.keys[&edge()],
            EdgeKey::Refused(SlotRefusal::WindowOpen)
        );
        let applied = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock, false);
        assert!(applied.calls.is_empty(), "{:?}", applied.calls);
    }

    #[test]
    fn a_rotation_whose_consumer_listing_could_not_be_read_is_not_planned_and_a_revoke_is_refused()
    {
        let listed = listing(None, &[TO]);
        let lock = deployed_both();
        let forced = decide(&listed, &lock, &options(true, &[]), &[CALLEE, CALLER]).unwrap();
        assert_eq!(forced.keys[&edge()], EdgeKey::Refused(SlotRefusal::Unread));
        let applied = apply_in_order(&[CALLEE, CALLER], &forced, &listed, &lock, false);
        assert!(applied.calls.is_empty(), "no slot is written blind");
        let revoked = decide(
            &listed,
            &lock,
            &options(false, &["shop.orders"]),
            &[CALLEE, CALLER],
        );
        assert!(
            revoked.is_err(),
            "a revoke that cannot read the slots is an error, not a no-op"
        );
    }

    #[test]
    fn a_producer_redeployed_after_it_was_deleted_rotates_with_the_old_key_kept_on_the_consumer() {
        // The producer's Worker was deleted: the ledger does not have it as deployed, so it holds
        // nothing, while the consumer holds the old key.
        let listed = listing(Some(&[FROM]), &[]);
        let mut lock = DeployLock::default();
        lock.record_deployed("default", CALLEE, None);
        let decisions = decide(&listed, &lock, &options(false, &[]), &[CALLEE, CALLER]).unwrap();
        let EdgeKey::Set { rotating, .. } = &decisions.keys[&edge()] else {
            panic!("a producer with no key is re-keyed");
        };
        assert!(*rotating, "the consumer held a key");
        assert_eq!(
            set_of(&decisions)[0].1.ops,
            [
                SlotOp::PutUntil(Slot::One, UNTIL),
                SlotOp::PutKey(Slot::Two)
            ],
            "messages the old producer signed and left in the queue are still admitted"
        );
    }

    #[test]
    fn a_consumer_redeployed_after_it_was_deleted_starts_with_one_live_key() {
        let listed = listing(Some(&[]), &[TO]);
        let mut lock = DeployLock::default();
        lock.record_deployed("default", CALLER, None);
        let decisions = decide(&listed, &lock, &options(false, &[]), &[CALLEE, CALLER]).unwrap();
        assert_eq!(
            set_of(&decisions)[0].1.ops,
            [SlotOp::PutKey(Slot::One)],
            "there is no old key on it to keep"
        );
    }

    #[test]
    fn a_revoke_drops_the_previous_key_at_once_and_says_so() {
        // The consumer is mid-rotation: the old key retiring in the first slot, the new one live.
        let listed = listing(Some(&[FROM, FROM2, UNTIL1]), &[TO]);
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
            assert!(
                set_of(&decisions)[0].1.retiring().is_none(),
                "a revoke retires nothing"
            );
            let applied = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock, false);
            assert_eq!(
                applied.calls,
                [
                    "secret put BYNK_CALL_FROM_SHOP__ORDERS",
                    "secret delete BYNK_CALL_FROM2_SHOP__ORDERS --name shop-payment",
                    "secret delete BYNK_CALL_UNTIL_SHOP__ORDERS --name shop-payment",
                    "secret put BYNK_CALL_TO_SHOP__PAYMENT",
                ],
                "{context}: the new key in the first slot, the other key and the marker gone, then the producer"
            );
        }
    }

    #[test]
    fn a_failed_delete_stops_the_deploy_naming_what_to_run() {
        let listed = listing(Some(&[FROM, FROM2, UNTIL1]), &[TO]);
        let lock = deployed_both();
        let decisions = decide(
            &listed,
            &lock,
            &options(false, &["shop.orders"]),
            &[CALLEE, CALLER],
        )
        .unwrap();
        let applied = apply_in_order(&[CALLEE, CALLER], &decisions, &listed, &lock, true);
        let message = applied.failure.expect("the deploy stops");
        assert!(
            message.contains("BYNK_CALL_FROM2_SHOP__ORDERS"),
            "{message}"
        );
        assert!(message.contains("--force"), "{message}");
        assert!(message.contains("--revoke-previous-edge-key"), "{message}");
        assert!(
            !applied.calls.iter().any(|c| c.contains("BYNK_CALL_TO_")),
            "the producer is not touched: {:?}",
            applied.calls
        );
    }

    #[test]
    fn a_revoke_that_cannot_re_key_the_edge_is_refused_not_ignored() {
        let listed = listing(Some(&[FROM]), &[TO]);
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
        let refused = refusal_notice("default", &edge(), SlotRefusal::WindowOpen);
        assert!(
            refused.contains("not rotated") && refused.contains("window"),
            "{refused}"
        );
    }
}
