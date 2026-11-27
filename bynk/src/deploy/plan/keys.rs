use super::messages::foreign_binding_refusal;
use super::run::DeployOptions;
use crate::deploy::config::Resources;
use crate::deploy::config::SessionKeyKind;
use crate::deploy::config::deploy_command;
use crate::deploy::config::deployed_worker_name;
use crate::deploy::ledger::DeployLock;
use crate::deploy::provisioning::WorkerPresence;
use crate::deploy::secrets::Edge;
use crate::deploy::secrets::EdgeKey;
use crate::deploy::secrets::ForeignKey;
use crate::deploy::secrets::Presence;
use crate::deploy::secrets::SecretSource;
use crate::deploy::secrets::decide_foreign_key;
use crate::deploy::secrets::decide_pepper;
use crate::deploy::secrets::decide_session_key;
use crate::deploy::secrets::mint_edge_key;
use crate::deploy::secrets::mint_session_key;
use crate::deploy::secrets::presence_of;
use crate::deploy::secrets::value_from_source;
use bynk_driver::colour;
use std::collections::BTreeMap;
use std::collections::BTreeSet;

/// The secrets `deploy` decides itself rather than asking the user for: a foreign caller's
/// (I3, #1971) and a `SignedCookie` actor's signing key (#3386), over the Workers in a run.
pub(crate) struct ManagedKeys<'a> {
    pub(crate) order: &'a [String],
    pub(crate) resources: &'a BTreeMap<String, Resources>,
    pub(crate) listed: &'a BTreeMap<String, Option<BTreeSet<String>>>,
}

impl ManagedKeys<'_> {
    /// Every such secret's name in the project, which is never spread as an ordinary secret.
    pub(crate) fn names(&self) -> BTreeSet<String> {
        self.resources
            .values()
            .flat_map(|r| {
                let session = r.session_keys.iter().map(|k| k.binding.clone());
                r.foreign_callers
                    .iter()
                    .map(|f| f.binding.clone())
                    .chain(session)
            })
            .collect()
    }

    /// What this run does with each, by (worker, secret): a value the operator supplies, the one
    /// the Worker holds (kept), or a fresh one minted. A refusal is the text to print. A foreign
    /// caller's mint is hex and handed off; a session key's is base64url and stays on its Worker,
    /// where a supplied value must be one the runtime accepts, so a weak key is caught here.
    pub(crate) fn decide(
        &self,
        lock: &DeployLock,
        opts: &DeployOptions,
        source: &SecretSource,
    ) -> Result<BTreeMap<(String, String), ForeignKey>, String> {
        let mut keys = BTreeMap::new();
        for worker in self.order {
            let presence = |binding: &str| {
                presence_of(
                    binding,
                    self.listed[worker].as_ref(),
                    !lock.is_deployed(&opts.environment, worker),
                )
            };
            for caller in &self.resources[worker].foreign_callers {
                let supplied = value_from_source(&caller.binding, source);
                let decision = decide_foreign_key(
                    supplied,
                    presence(&caller.binding),
                    opts.force,
                    &mut mint_edge_key,
                )
                .map_err(|e| foreign_binding_refusal(&opts.environment, worker, caller, &e))?;
                keys.insert((worker.clone(), caller.binding.clone()), decision);
            }
        }
        self.decide_session_keys(lock, opts, source, &mut keys)?;
        Ok(keys)
    }

    /// A `SignedCookie` actor's signing key, by binding. Contexts that list the same binding
    /// (a login service that issues the cookie and an API that is guarded by it) must hold the
    /// **same** value, or every cookie one issues fails on the other. So a minted key is minted
    /// once and set on each; and since Cloudflare never returns a secret's value, a run that would
    /// leave the contexts with different keys (a sibling already holds one and another is new, or
    /// a `--context` run reaches only some of them) is refused, asking for the value to be supplied.
    fn decide_session_keys(
        &self,
        lock: &DeployLock,
        opts: &DeployOptions,
        source: &SecretSource,
        keys: &mut BTreeMap<(String, String), ForeignKey>,
    ) -> Result<(), String> {
        for (binding, all) in self.session_key_holders() {
            // #3939: a pepper is kept under `--force` (`decide_pepper`); a signing key is not.
            let pepper = all
                .iter()
                .any(|(_, _, kind)| *kind == SessionKeyKind::Pepper);
            let in_run: Vec<_> = all
                .iter()
                .filter(|(w, _, _)| self.order.contains(*w))
                .collect();
            let supplied = value_from_source(binding, source);
            let mut minted: Option<String> = None;
            let mut mint = || match &minted {
                Some(v) => Ok(v.clone()),
                None => {
                    let v = mint_session_key()?;
                    minted = Some(v.clone());
                    Ok(v)
                }
            };
            let mut decisions = Vec::new();
            for (worker, actor, _) in &in_run {
                let presence = presence_of(
                    binding,
                    self.listed[*worker].as_ref(),
                    !lock.is_deployed(&opts.environment, worker),
                );
                let d = if pepper {
                    decide_pepper(supplied.clone(), presence, &mut mint)
                } else {
                    decide_session_key(supplied.clone(), presence, opts.force, &mut mint)
                };
                let what = if pepper {
                    "pepper of the `ApiKey`"
                } else {
                    "session key of the `SignedCookie`"
                };
                let d = d.map_err(|e| {
                    format!(
                        "bynk: error: the {what} actor `{actor}` (`{binding}`) on `{worker}`: {e}\n"
                    )
                })?;
                decisions.push(((*worker).clone(), d));
            }
            let fresh = decisions
                .iter()
                .any(|(_, d)| matches!(d, ForeignKey::Set { minted: true, .. }));
            let kept = decisions.iter().any(|(_, d)| *d == ForeignKey::Keep);
            if all.len() > 1 && fresh && (kept || in_run.len() < all.len()) {
                let names: Vec<&str> = all.iter().map(|(w, _, _)| w.as_str()).collect();
                return Err(format!(
                    "bynk: error: the session key `{binding}` is shared by {}, but this run would leave them with different values (one already holds a key, or not all of them are in this run) and Cloudflare never returns a secret's value.\n  Supply the same value for every one of them: set `{binding}` in the environment or `--secrets-file` (a base64url value of at least 32 bytes), then deploy again.\n",
                    names.join(", ")
                ));
            }
            for (worker, d) in decisions {
                keys.insert((worker, binding.to_string()), d);
            }
        }
        Ok(())
    }

    /// Each session-key binding with the Workers, actors and kinds that list it.
    fn session_key_holders(&self) -> BTreeMap<&str, Vec<(&String, &str, SessionKeyKind)>> {
        let mut holders: BTreeMap<&str, Vec<(&String, &str, SessionKeyKind)>> = BTreeMap::new();
        for (worker, r) in self.resources {
            for k in &r.session_keys {
                holders.entry(k.binding.as_str()).or_default().push((
                    worker,
                    k.actor.as_str(),
                    k.kind,
                ));
            }
        }
        holders
    }

    /// #3939 (the maintainer's ruling on #4595): under `--force`, a note for each `ApiKey` pepper
    /// a Worker in the run holds and this run keeps, since `--force` never replaces a set pepper.
    /// A supplied value is set instead, so it has no note.
    pub(crate) fn pepper_kept_under_force(
        &self,
        lock: &DeployLock,
        opts: &DeployOptions,
        source: &SecretSource,
    ) -> Vec<String> {
        if !opts.force {
            return Vec::new();
        }
        let mut out = Vec::new();
        for (binding, all) in self.session_key_holders() {
            if value_from_source(binding, source).is_some() {
                continue;
            }
            for (worker, actor, kind) in all {
                let held = presence_of(
                    binding,
                    self.listed.get(worker.as_str()).and_then(Option::as_ref),
                    !lock.is_deployed(&opts.environment, worker),
                ) != Presence::Absent;
                if kind == SessionKeyKind::Pepper && held && self.order.contains(worker) {
                    out.push(format!(
                        "bynk: note: kept the pepper `{binding}` of the `ApiKey` actor `{actor}` on `{worker}`: `--force` never replaces a set pepper, because a new one ends every key issued under it. To replace it, supply the new value (`{binding}` in the environment or `--secrets-file`).\n"
                    ));
                }
            }
        }
        out
    }
}

/// Interop I5 (#1972): a warning for each foreign Worker that the pushed
/// contexts bind to and `presence` does not find, each Worker asked once,
/// in name order. Cloudflare resolves a Service Binding at upload, so a push
/// binding a missing Worker is rejected there: the warning says so ahead of
/// it, and the driver itself refuses nothing (a Worker created between now
/// and the push is fine).
pub(crate) fn foreign_worker_warnings(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    mut presence: impl FnMut(&str) -> WorkerPresence,
) -> Vec<String> {
    let mut bound: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for worker in order {
        for f in &resources[worker].foreign_services {
            bound.entry(f.worker.as_str()).or_default().push(worker);
        }
    }
    let mut out = Vec::new();
    for (name, by) in bound {
        let by = by
            .iter()
            .map(|w| format!("`{w}`"))
            .collect::<Vec<_>>()
            .join(", ");
        match presence(name) {
            WorkerPresence::Present => {}
            WorkerPresence::Missing => out.push(format!(
                "bynk: warning: {by} binds to Worker `{name}`, which this account does not have — \
                 Cloudflare rejects a Service Binding to a missing Worker at upload, so deploy `{name}` first."
            )),
            WorkerPresence::Unknown(said) => out.push(format!(
                "bynk: warning: could not check that Worker `{name}` (bound by {by}) exists: {said}"
            )),
        }
    }
    out
}

/// #2004 review, I2 (#1971): what to tell the user when a run that re-keyed
/// edges stopped before every end of them was deployed. `completed` is the
/// Workers that finished (not the one that failed, which may or may not hold
/// its new keys). An edge whose ends had no key before cannot be split: the
/// next run finds an end without one and re-keys it. One that was rotating
/// can: each end may hold a different key, and a plain rerun keeps both.
/// `None` when no such edge was left incomplete.
pub(crate) fn interrupted_rotation_hint(
    environment: &str,
    edge_keys: &BTreeMap<Edge, EdgeKey>,
    completed: &[String],
) -> Option<String> {
    let done = |w: &String| completed.contains(w);
    let split: Vec<String> = edge_keys
        .iter()
        .filter(|(edge, key)| {
            matches!(key, EdgeKey::Set { rotating: true, .. })
                && !(done(&edge.caller) && done(&edge.callee))
        })
        .map(|(edge, _)| edge.describe(environment))
        .collect();
    if split.is_empty() {
        return None;
    }
    Some(format!(
        "bynk: note: this run was setting new keys on {} and stopped before both ends of each were deployed, so each end may hold a different key. The consumer still accepts the previous key for 15 days after a rotation (calls along it are refused at once after a `--revoke-previous-edge-key`), and the producer keeps signing under the old one.\n  \
         Rerun `{} --force` (the whole project) to set one key on both ends; a plain rerun would keep both.\n",
        split.join(", "),
        deploy_command(environment)
    ))
}

/// Whether one end of `edge` holds its key: it holds each name the edge puts on it (a
/// group's Worker holds one per member), and an end holding only some is keyed afresh.
pub(crate) fn end_presence(
    edge: &Edge,
    worker: &str,
    keys: &[String],
    listed: &BTreeMap<String, Option<BTreeSet<String>>>,
    lock: &DeployLock,
    environment: &str,
) -> Presence {
    let held: Vec<(&String, Presence)> = keys
        .iter()
        .map(|key| {
            let p = presence_of(
                key,
                listed[worker].as_ref(),
                !lock.is_deployed(environment, worker),
            );
            (key, p)
        })
        .collect();
    // #2004 review: name the Workers whose answer could not be read, since the
    // decision counts them as holding the key. One notice per end, naming its keys.
    let unknown: Vec<&str> = held
        .iter()
        .filter(|(_, p)| *p == Presence::Unknown)
        .map(|(key, _)| key.as_str())
        .collect();
    if !unknown.is_empty() {
        colour::eprint(&unknown_edge_key_notice(
            environment,
            worker,
            &unknown.join(", "),
            edge,
        ));
    }
    combine_presence(held.iter().map(|(_, p)| *p))
}

/// An end holds its key when it holds every name: one name absent makes it absent (it is
/// keyed afresh), else one unread makes it unknown, else it holds it.
pub(super) fn combine_presence(held: impl IntoIterator<Item = Presence>) -> Presence {
    let held: Vec<Presence> = held.into_iter().collect();
    if held.contains(&Presence::Absent) {
        Presence::Absent
    } else if held.contains(&Presence::Unknown) {
        Presence::Unknown
    } else {
        Presence::Present
    }
}

/// #2004 review: an end of `edge` whose secrets could not be listed, counted
/// as holding `key`. The Worker and the edge by their deployed names, the
/// remedy repeating the run's `--env` (#2047).
pub(crate) fn unknown_edge_key_notice(
    environment: &str,
    worker: &str,
    key: &str,
    edge: &Edge,
) -> String {
    format!(
        "bynk: warning: could not ask Cloudflare whether `{}` holds {key}; assuming it does. If it does not, the edge {} refuses every call — `{} --force` sets a fresh key on every edge.\n",
        deployed_worker_name(environment, worker),
        edge.describe(environment),
        deploy_command(environment)
    )
}
