//! #3565 (agent-context moves, slice 4b): the move engine ([`super::move_apply`]) in a real run.
//!
//! A run with move sets walks [`Placed::steps`]: an ordinary Worker is applied as any run applies
//! it ([`apply_one`]), and a set's two nodes run the engine. Before a set's receive node, each of
//! its Workers that the build has is **prepared** ([`prepare_worker`]): provisioned, its
//! configurations (`wrangler.toml` and each phase variant) materialised, its migrations applied
//! and its secrets set, so that each of those deployments lands before the move takes its
//! baselines. The phase pushes are then pushes only ([`LivePusher`]): a variant's config, and the
//! facts that say which move and phase pushed it (§4.2).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::process::ExitCode;

use bynk_driver::{colour, emsg};

use super::edge_slots::record_edge_slots;
use bynk_emit::emitter::workers_moves::{CANCEL_CONFIG, COMMIT_CONFIG, RECEIVE_CONFIG};
use bynk_project::worker_moves::{Incoming, MoveVariants, Outgoing};

use super::agent_move::{AGENT_MOVE_REFUSED, MOVE_OPEN, Placed};
use super::facts::{self, Facts, Transfer};
use super::ledger::{AgentMoveEntry, AgentMoveRecord, DeployLock};
use super::move_apply::{
    MovePusher, MoveRun, MoveStop, Phase, PhasePush, fresh_move_id, now_rfc3339,
};
use super::move_sets::{MoveSet, Step};
use super::*;

/// What wrangler says when a commit finds no pending transfer on the receiver (S7).
const TRANSFER_LOST: &str = "transferred_pending_not_found";

/// The build's phase variants for the run's move sets (§3.2): for each move not yet committed, its
/// receiver's receive (and cancel) config and its source's commit config, or a stub for a source
/// the build no longer has.
pub(crate) fn variants(
    placed: &Placed,
    open: &[AgentMoveRecord],
    resources: &BTreeMap<String, Resources>,
    environment: &str,
) -> MoveVariants {
    let mut out = MoveVariants::default();
    let deployed = |script: &str| deployed_worker_name(environment, script);
    for (set, resumes) in placed.sets.iter().zip(&placed.resumes) {
        let committed: Vec<String> = resumes
            .as_ref()
            .and_then(|r| open.iter().find(|o| o.move_id == r.move_id))
            .map(|o| o.committed.clone())
            .unwrap_or_default();
        for m in &set.moves {
            let classes: Vec<String> = m
                .classes
                .iter()
                .filter(|c| !committed.contains(c))
                .cloned()
                .collect();
            if !classes.is_empty() {
                out.receive
                    .entry(m.to_worker.clone())
                    .or_default()
                    .push(Incoming {
                        context: m.context.clone(),
                        classes,
                        from_script: deployed(&m.from_script),
                    });
            }
            let outgoing: Vec<Outgoing> = m
                .classes
                .iter()
                .map(|c| Outgoing {
                    class: c.clone(),
                    to_script: deployed(&m.to_script),
                })
                .collect();
            if resources.contains_key(&m.from_worker) && m.old_script_built {
                out.commit
                    .entry(m.from_worker.clone())
                    .or_default()
                    .extend(outgoing);
            } else {
                out.stubs
                    .entry(m.from_worker.clone())
                    .or_insert_with(|| (m.from_script.clone(), Vec::new()))
                    .1
                    .extend(outgoing);
            }
        }
    }
    out
}

/// The pushes of a run's move set: each Worker's prepared push, with a phase's config and facts.
pub(crate) struct LivePusher<'a> {
    pub(crate) api: &'a crate::cloudflare::Account<'a>,
    pub(crate) environment: &'a str,
    pub(crate) wrangler_args: &'a [String],
    pub(crate) workers_dir: &'a Path,
    /// Each Worker of the set the build has, prepared.
    pub(crate) prepared: BTreeMap<String, PreparedPush>,
    /// Each receiver → the classes moving into it (not live in its phase 1 or cancel version).
    pub(crate) incoming: BTreeMap<String, BTreeSet<String>>,
    /// Each source → the classes it gives up, with their receiving script.
    pub(crate) outgoing: BTreeMap<String, BTreeMap<String, String>>,
    pub(crate) groups: &'a GroupNames,
    pub(crate) resources: &'a BTreeMap<String, Resources>,
}

impl LivePusher<'_> {
    fn config(phase: Phase) -> &'static str {
        match phase {
            Phase::Receive => RECEIVE_CONFIG,
            Phase::Commit => COMMIT_CONFIG,
            Phase::GoLive => "wrangler.toml",
            Phase::Cancel => CANCEL_CONFIG,
        }
    }
}

impl MovePusher for LivePusher<'_> {
    fn push(&self, worker: &str, phase: Phase, transfer: &Transfer) -> PhasePush {
        let api: &dyn CloudflareApi = self.api;
        let (dir, mut facts, first, secrets) = match self.prepared.get(worker) {
            Some(p) => (
                p.worker_dir.clone(),
                p.facts.clone(),
                p.first_deploy && phase == Phase::Receive,
                p.secrets.clone(),
            ),
            // A dissolved source: its stub, which declares nothing but its tombstones.
            None => (
                self.workers_dir.join(worker),
                Facts {
                    v: 1,
                    ..Facts::default()
                },
                false,
                Vec::new(),
            ),
        };
        if matches!(phase, Phase::Receive | Phase::Cancel)
            && let Some(incoming) = self.incoming.get(worker)
        {
            // §4.2: a class is in `classes` only while it is live on this script.
            facts.classes.retain(|c| !incoming.contains(c));
        }
        facts.transfer = Some(transfer.clone());
        let mut args: Vec<String> = self.wrangler_args.to_vec();
        args.extend(["--config".to_string(), Self::config(phase).to_string()]);
        args.extend(facts::var_args(&facts));
        let environment = self.environment;
        if phase == Phase::Commit {
            return commit_push(api, &dir, worker, &args, environment);
        }
        let file = if first && !secrets.is_empty() {
            match SecretsFile::write(&secrets) {
                Ok(file) => Some(file),
                Err(e) => {
                    return PhasePush::Failed(DeployFailure::driver(format!(
                        "could not write the secrets for `{}`'s first version to a temporary file: {e}",
                        deployed_worker_name(environment, worker)
                    )));
                }
            }
        } else {
            None
        };
        let pushed = push(
            api,
            &dir,
            worker,
            &args,
            environment,
            (file.as_ref().map(|f| f.path()), first),
        );
        drop(file);
        if first && pushed == Ok(Pushed::Ok) {
            report_first_version_bindings(&secrets, &deployed_worker_name(environment, worker));
        }
        match pushed {
            Ok(Pushed::Ok) => PhasePush::Landed,
            Ok(Pushed::Interrupted) => PhasePush::Interrupted,
            Err(f) => PhasePush::Failed(f),
        }
    }

    fn record(&self, lock: &mut DeployLock, worker: &str, phase: Phase) {
        let environment = self.environment;
        match phase {
            Phase::Commit => {
                if let Some(given) = self.outgoing.get(worker) {
                    lock.record_transferred(environment, worker, given.clone());
                }
                match self.resources.get(worker) {
                    Some(declared) if self.prepared.contains_key(worker) => {
                        record_worker(lock, environment, self, worker, declared);
                    }
                    // A dissolved source keeps no member: its contexts are the receivers' now.
                    _ => lock.record_members(environment, worker, &[]),
                }
            }
            Phase::GoLive => {
                if let Some(declared) = self.resources.get(worker) {
                    record_worker(lock, environment, self, worker, declared);
                }
            }
            // A new receiver is on the account from its phase 1 on: a resume must not take it for
            // a first deploy, which would mint edge keys its callers get and it never does.
            Phase::Receive => {
                if self.prepared.get(worker).is_some_and(|p| p.first_deploy)
                    && !lock.is_deployed(environment, worker)
                {
                    lock.record_deployed(environment, worker, None);
                }
            }
            Phase::Cancel => {}
        }
    }
}

/// What the ledger records of a pushed Worker of the build, as [`record_pushed`] does.
fn record_worker(
    lock: &mut DeployLock,
    environment: &str,
    pusher: &LivePusher<'_>,
    worker: &str,
    declared: &Resources,
) {
    let pushed = PushedWorker::of(worker, pusher.workers_dir, pusher.groups, declared);
    record_pushed_contracts(
        lock,
        environment,
        (pushed.worker, &pushed.dir),
        (&pushed.members, &pushed.script),
    );
    record_pushed_tombstones(lock, environment, worker, declared);
    kinds_after_push(lock, environment, worker, declared);
}

/// A commit push, its stderr watched for a lost transfer (S7): wrangler names
/// `transferred_pending_not_found` when the receiver no longer expects the class.
fn commit_push(
    api: &dyn CloudflareApi,
    dir: &Path,
    worker: &str,
    args: &[String],
    environment: &str,
) -> PhasePush {
    let run = match api.push(crate::cloudflare::PushCall {
        worker_dir: dir,
        args,
        secrets_file: None,
        environment,
        tee: true,
    }) {
        Ok(run) => run,
        Err(e) => {
            return PhasePush::Failed(DeployFailure::driver(format!(
                "could not run wrangler deploy for `{}`: {e}",
                deployed_worker_name(environment, worker)
            )));
        }
    };
    match wrangler_outcome(environment, worker, run.exit) {
        Ok(Pushed::Ok) => PhasePush::Landed,
        Ok(Pushed::Interrupted) => PhasePush::Interrupted,
        Err(f) if run.said.contains(TRANSFER_LOST) => PhasePush::TransferLost(f),
        Err(f) => PhasePush::Failed(f),
    }
}

/// The moves of `set` as its record lists them.
fn entries(set: &MoveSet) -> Vec<AgentMoveEntry> {
    set.moves
        .iter()
        .map(|m| AgentMoveEntry {
            context: m.context.clone(),
            classes: m.classes.clone(),
            from_worker: m.from_worker.clone(),
            from_script: m.from_script.clone(),
            to_worker: m.to_worker.clone(),
            to_script: m.to_script.clone(),
        })
        .collect()
}

/// Say why the move stopped, and the code the run exits with.
pub(crate) fn report(stop: &MoveStop) -> ExitCode {
    for refusal in &stop.refusals {
        emsg!("bynk: error: {}", refusal.line());
    }
    for failure in &stop.failures {
        emsg!("bynk: error: {}", failure.message);
    }
    match stop.failures.first() {
        Some(f) => ExitCode::from(f.code),
        None if !stop.refusals.is_empty() => ExitCode::FAILURE,
        None => ExitCode::SUCCESS,
    }
}

/// Prepare each Worker of `set` the build has (§3.2), before the move takes its baselines. A
/// Worker of a move under way may not set a secret (D8 (d)): each would be a deployment the move
/// cannot tell from someone else's.
fn prepare_set(
    apply: &PlanApply<'_>,
    writer: &mut LedgerWriter<'_>,
    set: &MoveSet,
    under_way: bool,
    resolved: &mut BTreeMap<String, String>,
    (only, cancel): (Option<&[String]>, bool),
) -> Result<BTreeMap<String, PreparedPush>, MoveStop> {
    let opts = apply.opts;
    let keys = apply.keys;
    let mut prepared = BTreeMap::new();
    for worker in set
        .built_workers(apply.resources)
        .into_iter()
        .filter(|w| only.is_none_or(|only| only.contains(w)))
    {
        let Some(i) = apply.order.iter().position(|w| *w == worker) else {
            continue;
        };
        let run = &mut RunCx {
            tools: apply.tools,
            workers_dir: apply.workers_dir,
            project: apply.resources,
            writer,
            secrets: &mut Secrets {
                source: apply.secret_source,
                force: opts.force,
                edge_keys: &keys.edge_keys,
                expired: &keys.expired,
                cleaned: &keys.cleaned,
                foreign_keys: &keys.foreign_keys,
                foreign_secrets: &keys.foreign_secrets,
                listed: &keys.listed,
                resolved,
                with_upload: apply.with_upload,
            },
            wrangler_args: &opts.wrangler_args,
            environment: &opts.environment,
            lives: apply.lives,
        };
        let p = prepare_worker(
            run,
            &apply.plan.contexts[i],
            &[RECEIVE_CONFIG, COMMIT_CONFIG, CANCEL_CONFIG],
            !cancel,
        )
        .map_err(MoveStop::from_failure)?;
        if p.first_deploy && !p.secrets.is_empty() && !apply.with_upload {
            // A new receiver's secrets must ride its phase 1 (`--secrets-file`): set after it,
            // each would be a deployment the move could not tell from someone else's.
            return Err(MoveStop::from_refusal(
                Entry::new(
                    AGENT_MOVE_REFUSED,
                    format!(
                        "`{}` is a new Worker that receives an agent move and needs secrets, which this wrangler cannot upload with its first version (`--secrets-file` needs wrangler {SECRETS_FILE_MIN} or later); upgrade wrangler and deploy again.",
                        deployed_worker_name(&opts.environment, &worker)
                    ),
                )
                .with("reason", "secrets_file")
                .with("worker", worker.as_str()),
            ));
        }
        if under_way && !p.first_deploy && !p.secrets.is_empty() {
            let names: Vec<String> = p.secrets.iter().map(|(n, _)| n.clone()).collect();
            return Err(MoveStop::from_refusal(
                Entry::new(
                    MOVE_OPEN,
                    format!(
                        "`{}` would set {} while an agent move is under way; each set is a deployment the move could not tell from someone else's. Finish the move with the secrets as they are, then set {} with the next `bynk deploy`.",
                        deployed_worker_name(&opts.environment, &worker),
                        names_phrase(&names),
                        if names.len() == 1 { "it" } else { "them" }
                    ),
                )
                .with("worker", worker.as_str())
                .with("secrets", names),
            ));
        }
        // A retired edge key's delete is a deployment too: while a move is under way it waits
        // for the run after the move closes, which still finds it retired.
        apply_prepared_secrets(run, &p, !under_way).map_err(MoveStop::from_failure)?;
        hand_off_and_record_slots(apply, writer, &worker)?;
        prepared.insert(worker, p);
    }
    Ok(prepared)
}

/// What [`apply_one`] does once a Worker's secrets are set: hand each foreign caller's newly
/// minted secret to its operator (the only time the value is shown), and record the edge-key
/// slots its rotations left retiring (#4526).
fn hand_off_and_record_slots(
    apply: &PlanApply<'_>,
    writer: &mut LedgerWriter<'_>,
    worker: &str,
) -> Result<(), MoveStop> {
    let (env, keys) = (apply.opts.environment.as_str(), apply.keys);
    for caller in &apply.resources[worker].foreign_callers {
        if let Some(ForeignKey::Set {
            value,
            minted: true,
        }) = keys
            .foreign_keys
            .get(&(worker.to_string(), caller.binding.clone()))
        {
            colour::eprint(&foreign_hand_off(env, worker, caller, value, apply.runner));
        }
    }
    writer
        .change(|lock| {
            record_edge_slots(lock, env, worker, (&keys.edge_keys, &keys.cleaned.borrow()));
        })
        .map_err(|e| {
            MoveStop::from_failure(DeployFailure::driver(format!(
                "could not record `{worker}`'s edge keys in {}: {e}",
                writer.path().display()
            )))
        })
}

fn names_phrase(names: &[String]) -> String {
    let quoted: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
    match quoted.as_slice() {
        [one] => format!("secret {one}"),
        many => format!("secrets {}", many.join(", ")),
    }
}

/// The pushes of `set`, with its Workers' prepared pushes.
fn live_pusher<'a>(
    apply: &PlanApply<'a>,
    set: &MoveSet,
    prepared: BTreeMap<String, PreparedPush>,
    committed: &[String],
) -> LivePusher<'a> {
    let environment = apply.opts.environment.as_str();
    let mut incoming: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut outgoing: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    for m in &set.moves {
        // A class already committed is live on its receiver (§4.2), even in a second phase 1.
        incoming
            .entry(m.to_worker.clone())
            .or_default()
            .extend(m.classes.iter().filter(|c| !committed.contains(c)).cloned());
        for c in &m.classes {
            outgoing
                .entry(m.from_worker.clone())
                .or_default()
                .insert(c.clone(), deployed_worker_name(environment, &m.to_script));
        }
    }
    LivePusher {
        api: apply
            .move_api
            .expect("a run that moves agents pushes through the account"),
        environment,
        wrangler_args: &apply.opts.wrangler_args,
        workers_dir: apply.workers_dir,
        prepared,
        incoming,
        outgoing,
        groups: apply.groups,
        resources: apply.resources,
    }
}

/// `--cancel-move` (D8): cancel set `n` of `placed`, a move under way, by pushing each receiver
/// with its cancel config, then closing the move's record.
pub(crate) fn cancel_set(
    apply: &PlanApply<'_>,
    writer: &mut LedgerWriter<'_>,
    placed: &Placed,
    n: usize,
) -> Result<(), ExitCode> {
    let set = &placed.sets[n];
    let move_id = placed.resumes[n]
        .as_ref()
        .expect("a cancelled set is under way")
        .move_id
        .clone();
    let receivers = set.receivers();
    let mut resolved = BTreeMap::new();
    let prepared = prepare_set(
        apply,
        writer,
        set,
        true,
        &mut resolved,
        (Some(&receivers), true),
    )
    .map_err(|stop| report(&stop))?;
    let pusher = live_pusher(apply, set, prepared, &[]);
    let run = MoveRun {
        environment: apply.opts.environment.as_str(),
        account: apply
            .move_account
            .expect("a run that moves agents reads the account"),
        pusher: &pusher,
    };
    run.cancel(writer, &move_id).map_err(|stop| report(&stop))
}

/// [`apply_plan`] for a run with move sets: each step of `placed` in turn.
pub(crate) fn apply_placed(
    apply: &PlanApply<'_>,
    writer: &mut LedgerWriter<'_>,
    placed: &Placed,
    resolved: &mut BTreeMap<String, String>,
) -> Result<(), ExitCode> {
    let environment = apply.opts.environment.as_str();
    let account = apply
        .move_account
        .expect("a run that moves agents reads the account");
    let mut pushers: BTreeMap<usize, (String, LivePusher<'_>)> = BTreeMap::new();
    for step in &placed.steps {
        match step {
            Step::Worker(w) => {
                let i = apply
                    .order
                    .iter()
                    .position(|o| o == w)
                    .expect("a placed Worker is in the order");
                apply_one(apply, writer, i, resolved)?;
            }
            Step::Receive(n) => {
                let set = &placed.sets[*n];
                let resumes = placed.resumes[*n].as_ref();
                let prepared = prepare_set(
                    apply,
                    writer,
                    set,
                    resumes.is_some(),
                    resolved,
                    (None, false),
                )
                .map_err(|stop| report(&stop))?;
                let committed: Vec<String> = resumes
                    .and_then(|r| {
                        writer
                            .lock()
                            .open_moves(environment)
                            .iter()
                            .find(|o| o.move_id == r.move_id)
                            .map(|o| o.committed.clone())
                    })
                    .unwrap_or_default();
                let pusher = live_pusher(apply, set, prepared, &committed);
                let move_id = match resumes {
                    Some(r) => r.move_id.clone(),
                    None => {
                        let id = fresh_move_id().map_err(|e| {
                            report(&MoveStop::from_failure(DeployFailure::driver(e)))
                        })?;
                        let run = MoveRun {
                            environment,
                            account,
                            pusher: &pusher,
                        };
                        run.open(writer, entries(set), &id, &now_rfc3339())
                            .map_err(|stop| report(&stop))?;
                        id
                    }
                };
                let run = MoveRun {
                    environment,
                    account,
                    pusher: &pusher,
                };
                run.receive(writer, &move_id)
                    .map_err(|stop| report(&stop))?;
                pushers.insert(*n, (move_id, pusher));
            }
            Step::Commit(n) => {
                let (move_id, pusher) = pushers
                    .get(n)
                    .expect("a set's receive node comes before its commit node");
                let run = MoveRun {
                    environment,
                    account,
                    pusher,
                };
                run.commit(writer, move_id).map_err(|stop| report(&stop))?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloudflare::{Account, Wrangler};
    use crate::deploy::agent_move::AgentMove;

    fn scratch(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bynk-move-run-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("core")).unwrap();
        dir
    }

    #[cfg(unix)]
    fn transfer(phase: Phase) -> Transfer {
        Transfer {
            move_id: "m1".into(),
            phase: phase.number(),
            expecting: BTreeMap::from([("Cart".into(), "shop-orders".into())]),
            ..Transfer::default()
        }
    }

    fn moving(context: &str, class: &str, from: &str, to: &str, built: bool) -> AgentMove {
        AgentMove {
            context: context.into(),
            classes: vec![class.into()],
            from_worker: from.into(),
            from_script: from.into(),
            to_worker: to.into(),
            to_script: to.into(),
            old_script_built: built,
            renamed_in_place: false,
            tombstones: Vec::new(),
        }
    }

    /// The build's variants: each receiver expects what is not yet committed; a source the build
    /// has gives its classes up in its commit config, one it lacks as a stub.
    #[test]
    fn the_variants_leave_out_what_is_committed_and_stub_a_dissolved_source() {
        let set = MoveSet {
            moves: vec![
                moving("shop.orders", "Cart", "shop-orders", "core", false),
                moving("shop.pay", "Wallet", "pay", "core", true),
            ],
        };
        let placed = Placed {
            sets: vec![set],
            resumes: vec![Some(super::super::move_sets::Resumes {
                move_id: "m1".into(),
                step: "receiving".into(),
            })],
            steps: Vec::new(),
        };
        let record = AgentMoveRecord {
            move_id: "m1".into(),
            step: super::super::ledger::MoveStep::Receiving,
            started: String::new(),
            bynk: String::new(),
            done: Vec::new(),
            moves: Vec::new(),
            deployments: BTreeMap::new(),
            namespaces: BTreeMap::new(),
            committed: vec!["Cart".into()],
        };
        let resources = BTreeMap::from([("pay".to_string(), Resources::default())]);
        let v = variants(&placed, &[record], &resources, "staging");
        let into_core: Vec<&str> = v.receive["core"]
            .iter()
            .map(|i| i.context.as_str())
            .collect();
        assert_eq!(
            into_core,
            ["shop.pay"],
            "Cart is committed: not received again"
        );
        assert_eq!(v.receive["core"][0].from_script, "pay-staging");
        assert_eq!(v.commit["pay"][0].to_script, "core-staging");
        assert_eq!(v.stubs["shop-orders"].0, "shop-orders");
        assert_eq!(v.stubs["shop-orders"].1[0].class, "Cart");
    }

    /// A new receiver is recorded as on the account when its phase 1 lands, so a resume decides
    /// its keys as for a deployed Worker and does not mint edge keys it would never get.
    #[test]
    fn a_new_receivers_phase_1_records_it_as_deployed() {
        let dir = scratch("deployed");
        let api = Account::new(Wrangler::scripted("exit 0", &dir));
        let groups = GroupNames::of(&dir);
        let resources = BTreeMap::new();
        let pusher = LivePusher {
            api: &api,
            environment: "default",
            wrangler_args: &[],
            workers_dir: &dir,
            prepared: BTreeMap::from([(
                "core".to_string(),
                PreparedPush {
                    worker: "core".into(),
                    worker_dir: dir.join("core"),
                    first_deploy: true,
                    secrets: Vec::new(),
                    facts: Facts::default(),
                },
            )]),
            incoming: BTreeMap::new(),
            outgoing: BTreeMap::new(),
            groups: &groups,
            resources: &resources,
        };
        let mut lock: DeployLock = toml::from_str("version = 1\n").unwrap();
        assert!(!lock.is_deployed("default", "core"));
        pusher.record(&mut lock, "core", Phase::Receive);
        assert!(lock.is_deployed("default", "core"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// S7: a commit that finds no pending transfer on the receiver is a lost transfer, which the
    /// engine sends back to phase 1; any other failed commit is a failure.
    #[test]
    fn a_commit_refused_for_no_pending_transfer_is_a_lost_transfer() {
        let dir = scratch("lost");
        for (said, lost) in [
            ("X [code: 10081] transferred_pending_not_found", true),
            ("something else went wrong", false),
        ] {
            let api = Account::new(Wrangler::scripted(
                &format!("echo '{said}' >&2; exit 1"),
                &dir,
            ));
            let pushed = commit_push(&api, &dir.join("core"), "core", &[], "default");
            assert_eq!(matches!(pushed, PhasePush::TransferLost(_)), lost, "{said}");
            assert!(!matches!(pushed, PhasePush::Landed));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Each phase push uses its phase's config, and its facts carry the move and phase, with the
    /// incoming class out of `classes` while it is not live (§4.2). Unix: the scripted wrangler is
    /// `sh`, which reads the facts' JSON back only where its arguments arrive as they were passed
    /// (as `cloudflare/process.rs`'s argument tests).
    #[cfg(unix)]
    #[test]
    fn a_phase_push_uses_its_config_and_says_its_move_in_its_facts() {
        let dir = scratch("args");
        let api = Account::new(Wrangler::scripted(r#"printf '%s\n' "$@" > args.txt"#, &dir));
        let groups = GroupNames::of(&dir);
        let resources = BTreeMap::new();
        let prepared = BTreeMap::from([(
            "core".to_string(),
            PreparedPush {
                worker: "core".into(),
                worker_dir: dir.join("core"),
                first_deploy: false,
                secrets: Vec::new(),
                facts: Facts {
                    v: 1,
                    classes: vec!["Cart".into(), "Other".into()],
                    ..Facts::default()
                },
            },
        )]);
        let pusher = LivePusher {
            api: &api,
            environment: "default",
            wrangler_args: &[],
            workers_dir: &dir,
            prepared,
            incoming: BTreeMap::from([("core".into(), BTreeSet::from(["Cart".to_string()]))]),
            outgoing: BTreeMap::new(),
            groups: &groups,
            resources: &resources,
        };
        for (phase, config, live) in [
            (Phase::Receive, "wrangler.receive.toml", false),
            (Phase::Cancel, "wrangler.cancel.toml", false),
            (Phase::GoLive, "wrangler.toml", true),
        ] {
            assert_eq!(
                pusher.push("core", phase, &transfer(phase)),
                PhasePush::Landed
            );
            let args = std::fs::read_to_string(dir.join("core/args.txt")).unwrap();
            assert!(args.contains(&format!("--config\n{config}\n")), "{args}");
            let facts = args
                .lines()
                .find_map(|l| l.strip_prefix("BYNK_DEPLOY_FACTS:"))
                .unwrap();
            let facts = Facts::decode(facts).unwrap();
            let t = facts.transfer.unwrap();
            assert_eq!((t.move_id.as_str(), t.phase), ("m1", phase.number()));
            assert_eq!(
                facts.classes.contains(&"Cart".to_string()),
                live,
                "{phase:?}"
            );
            assert!(facts.classes.contains(&"Other".to_string()));
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    /// Slice 4b lifted the refusal of a supported move outside `--dry-run`: no code path says
    /// a move is not applied yet.
    #[test]
    fn the_refusal_of_a_move_outside_a_dry_run_is_gone() {
        let gone = ["apply", "_not_", "yet"].concat();
        for source in [
            include_str!("agent_move.rs"),
            include_str!("move_apply.rs"),
            include_str!("move_run.rs"),
            include_str!("plan/run.rs"),
        ] {
            assert!(!source.contains(&gone));
        }
    }
}
