//! 1D.1c (#3929): Hyperdrive, each `Postgres` (or `MySql`) instance's config (`capability Main =
//! Postgres`), as one [`ResourceKind`].
//!
//! A config is made, synced or adopted through Cloudflare's REST API (`HyperdriveApi`, in
//! `deploy/hyperdrive.rs`), from what `bynk.toml` and the environment say each binding wants. Those
//! wants and the API are the run's [`Live`](ResourceKind::Live): read once, before the plan, and
//! seen by the plan line, provisioning and `--prune`. The ledger records the config's name and id
//! (#3319), never anything derived from the connection string. A replaced config is kept as a
//! superseded record and reported as an orphan, which `--prune` deletes unless it was adopted.
//!
//! #3970: the config's id is its [`ResourceKind::bound_id`], so the deploy facts record it and
//! `bynk revert` refuses a version bound to a config the ledger has since replaced (superseded,
//! or pruned), as it does for KV, D1 and rate limiters. An adopted config is checked the same
//! way: a different adopted id is a different config. Facts written before this have no
//! Hyperdrive id, so there is nothing to compare.

use super::super::hyperdrive::{HyperdriveRun, Want, action, config_name, need_api, scrub};
use super::*;

/// The Hyperdrive kind.
pub(crate) struct Hyperdrive;

/// [`Hyperdrive`]'s records: `Environment::hyperdrive_instances`, by Worker and binding.
pub(crate) struct HyperdriveTable;

/// What provisioning and pruning change in [`Hyperdrive`]'s records.
pub(crate) enum HyperdriveChange {
    /// A config made, synced or adopted.
    Record(TableChange<HyperdriveTable>),
    /// Config `id` of `worker`'s `binding`, deleted by `--prune` ([`HyperdriveTable::forget_config`]).
    ForgetConfig {
        worker: String,
        binding: String,
        id: String,
    },
}

impl LedgerChange for HyperdriveChange {
    fn apply(self, lock: &mut DeployLock, environment: &str) -> bool {
        match self {
            HyperdriveChange::Record(change) => change.apply(lock, environment),
            HyperdriveChange::ForgetConfig {
                worker,
                binding,
                id,
            } => {
                if let Some(env) = lock.environments.get_mut(environment) {
                    HyperdriveTable::forget_config(env, &worker, &binding, &id);
                }
                true
            }
        }
    }
}

impl HyperdriveTable {
    /// Drop config `id` from `worker`'s `binding` once `--prune` deleted it, whether it was the
    /// live one (its Worker or capability is gone) or a superseded one, and the Worker's map once
    /// it is empty.
    pub(crate) fn forget_config(env: &mut Environment, worker: &str, binding: &str, id: &str) {
        let Some(instances) = env.hyperdrive_instances.get_mut(worker) else {
            return;
        };
        if instances.get(binding).is_some_and(|r| r.id == id) {
            instances.remove(binding);
        } else if let Some(record) = instances.get_mut(binding) {
            record.superseded.retain(|past| past.id != id);
        }
        if instances.is_empty() {
            env.hyperdrive_instances.remove(worker);
        }
    }
}

impl LedgerTable for HyperdriveTable {
    type Record = HyperdriveRecord;

    fn get<'l>(env: &'l Environment, worker: &str, binding: &str) -> Option<&'l HyperdriveRecord> {
        env.hyperdrive_instances
            .get(worker)
            .and_then(|instances| instances.get(binding))
    }

    /// A config this replaces is not forgotten: it stays as a superseded one, reported as an
    /// orphan, unless the project had adopted it, which was never its own to delete.
    fn insert(env: &mut Environment, worker: &str, binding: &str, record: HyperdriveRecord) {
        let instances = env
            .hyperdrive_instances
            .entry(worker.to_string())
            .or_default();
        let mut record = record;
        if let Some(old) = instances.remove(binding) {
            record.superseded = old.superseded;
            if old.id != record.id && !old.adopted {
                record.superseded.push(HyperdrivePast {
                    name: old.name,
                    id: old.id,
                    adopted: old.adopted,
                });
            }
        }
        instances.insert(binding.to_string(), record);
    }

    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if let Some(instances) = env.hyperdrive_instances.get_mut(worker) {
            instances.remove(binding);
            if instances.is_empty() {
                env.hyperdrive_instances.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &HyperdriveRecord)> {
        env.hyperdrive_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances
                    .iter()
                    .map(move |(binding, config)| (worker.as_str(), binding.as_str(), config))
            })
            .collect()
    }
}

impl ResourceKind for Hyperdrive {
    const ID: KindId = KindId::Hyperdrive;

    type Decl<'r> = &'r HyperdriveDecl;
    type Record = HyperdriveRecord;
    type Table = HyperdriveTable;
    type Line<'r> = PlanHyperdrive<'r>;
    type Orphan = OrphanHyperdrive;
    /// What each binding wants, and the API to ask. A bare plan's wants are empty, and then a
    /// config is a `create` or a `reuse` by the ledger alone.
    type Live<'a> = HyperdriveRun<'a>;
    /// The config's id, which the config binds it by.
    type Bound = String;
    type Change = HyperdriveChange;
    type Step<'r> = HyperdriveStep<'r>;

    fn declared(r: &Resources) -> Vec<&HyperdriveDecl> {
        r.hyperdrive.iter().collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        &decl.binding
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanHyperdrive<'r>] {
        &context.hyperdrive
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanHyperdrive<'r>> {
        &mut context.hyperdrive
    }

    fn live_in<'a>(lives: Lives<'a>) -> HyperdriveRun<'a> {
        lives.hyperdrive
    }

    /// Never the connection string: an adopted config by its id, a made one by its name, and the
    /// variable a sync reads by name.
    fn plan_line<'r>(
        cx: &PlanCx<'_>,
        decl: Self::Decl<'r>,
        run: HyperdriveRun<'_>,
    ) -> PlanHyperdrive<'r> {
        let step = hyperdrive_step(cx, decl, run);
        let want = run
            .wants
            .get(&(cx.worker.to_string(), decl.binding.clone()));
        PlanHyperdrive {
            binding: &decl.binding,
            action: step.action,
            config: match want {
                Some(Want::Adopt { id }) => id.clone(),
                _ => step.physical,
            },
            connection_env: match want {
                Some(Want::Connection { variable, .. }) => Some(variable.clone()),
                _ => None,
            },
        }
    }

    fn steps<'r>(
        cx: &PlanCx<'_>,
        declared: &'r Resources,
        run: HyperdriveRun<'_>,
    ) -> Vec<HyperdriveStep<'r>> {
        declared
            .hyperdrive
            .iter()
            .map(|decl| hyperdrive_step(cx, decl, run))
            .collect()
    }

    fn steps_of<'p, 'r>(steps: &'p WorkerSteps<'r>) -> &'p [HyperdriveStep<'r>] {
        &steps.hyperdrive
    }

    fn steps_mut<'p, 'r>(steps: &'p mut WorkerSteps<'r>) -> &'p mut Vec<HyperdriveStep<'r>> {
        &mut steps.hyperdrive
    }

    fn short_line(h: &PlanHyperdrive<'_>, out: &mut String) {
        out.push_str(&format!(
            "hyperdrive {} {} ({}",
            h.action, h.config, h.binding
        ));
        if let Some(variable) = &h.connection_env {
            out.push_str(&format!("; from `{variable}`"));
        }
        out.push_str(")\n");
    }

    fn orphans_in(orphans: &Orphans) -> &[OrphanHyperdrive] {
        &orphans.hyperdrive
    }

    fn set_orphans(orphans: &mut Orphans, found: Vec<OrphanHyperdrive>) {
        orphans.hyperdrive = found;
    }

    /// A config with its Worker or on its own; one a replacement left behind is an orphan while
    /// its binding is live, too.
    fn orphans(cx: &OrphanCx<'_>) -> Vec<OrphanHyperdrive> {
        let resources = cx.resources;
        cx.env
            .hyperdrive_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances.iter().flat_map(move |(binding, record)| {
                    let live = resources
                        .get(worker)
                        .is_some_and(|r| r.hyperdrive.iter().any(|h| &h.binding == binding));
                    let own = (!live).then(|| OrphanHyperdrive {
                        worker: worker.clone(),
                        binding: binding.clone(),
                        config: record.name.clone(),
                        id: record.id.clone(),
                        adopted: record.adopted,
                    });
                    own.into_iter()
                        .chain(record.superseded.iter().map(move |past| OrphanHyperdrive {
                            worker: worker.clone(),
                            binding: binding.clone(),
                            config: past.name.clone(),
                            id: past.id.clone(),
                            adopted: past.adopted,
                        }))
                })
            })
            .collect()
    }

    fn shown_orphan(h: &OrphanHyperdrive, _environment: &str) -> OrphanHyperdrive {
        h.clone()
    }

    fn short_orphan(h: &OrphanHyperdrive, out: &mut String) {
        out.push_str(&format!(
            "orphan hyperdrive {} {} ({}{})\n",
            h.worker,
            h.binding,
            h.config,
            if h.adopted {
                "; adopted, never deleted"
            } else {
                "; deleted by --prune"
            }
        ));
    }

    fn orphan_worker(h: &OrphanHyperdrive) -> &str {
        &h.worker
    }

    /// A config holds no data, so one the deploy made is deleted; an adopted one never is.
    fn prunable(h: &OrphanHyperdrive) -> bool {
        !h.adopted
    }

    fn prune_notice(h: &OrphanHyperdrive, _environment: &str) -> Option<Notice> {
        Some(Notice::Plain(format!(
            "bynk: will delete Hyperdrive config `{}` of `{}`'s `{}`",
            h.config, h.worker, h.binding
        )))
    }

    /// Deleted by its id, if the run holds an API (it does whenever there is one to delete), and
    /// stripped from the ledger. Already gone is success, as for a queue (DECISION E).
    fn prune(
        cx: &StepCx<'_, '_, Hyperdrive>,
        orphan: &OrphanHyperdrive,
    ) -> Result<Option<Commit<HyperdriveChange>>, DeployFailure> {
        let Some(api) = cx.live.api else {
            return Ok(None);
        };
        api.delete(&orphan.id).map_err(|e| {
            DeployFailure::driver(format!(
                "could not delete the orphaned Hyperdrive config `{}` of `{}`'s `{}`: {e}",
                orphan.config, orphan.worker, orphan.binding
            ))
        })?;
        Ok(Some(Commit::recorded(
            HyperdriveChange::ForgetConfig {
                worker: orphan.worker.clone(),
                binding: orphan.binding.clone(),
                id: orphan.id.clone(),
            },
            &format!("deleted the orphaned Hyperdrive config `{}`", orphan.config),
        )))
    }

    /// Make, update or adopt the config, record what was done before the push (an interrupted
    /// run never makes a second config), and bind it by its id.
    fn apply(
        cx: &StepCx<'_, '_, Hyperdrive>,
        step: &HyperdriveStep<'_>,
    ) -> Result<Applied<Hyperdrive>, DeployFailure> {
        let (worker, run) = (cx.worker, cx.live);
        let binding = &step.decl.binding;
        let physical = step.physical.clone();
        let want = run.wants.get(&(worker.to_string(), binding.clone()));
        let recorded = step.recorded.clone();
        let todo = step.action;
        let record = |record: HyperdriveRecord, done: &str| {
            Commit::recorded(
                HyperdriveChange::Record(TableChange::insert(worker, binding, record)),
                done,
            )
        };
        let (id, commit) = match (todo, want) {
            ("reuse", _) => match &recorded {
                Some(record) => (record.id.clone(), None),
                None => {
                    return Err(DeployFailure::driver(format!(
                        "no Hyperdrive config is recorded for `{worker}`'s `{binding}`"
                    )));
                }
            },
            ("adopt", Some(Want::Adopt { id })) => {
                let api = need_api(&run, worker, binding)?;
                let name = api
                    .name_of(id)
                    .map_err(|e| {
                        DeployFailure::driver(format!(
                            "could not look up the Hyperdrive config `{id}` for `{worker}`'s `{binding}`: {e}"
                        ))
                    })?
                    .ok_or_else(|| {
                        DeployFailure::driver(format!(
                            "the Hyperdrive config `{id}` that bynk.toml names for `{worker}`'s `{binding}` does not exist on this account"
                        ))
                    })?;
                let done = format!("changed the Hyperdrive config `{name}`");
                let commit = record(
                    HyperdriveRecord {
                        name,
                        id: id.clone(),
                        adopted: true,
                        superseded: Vec::new(),
                    },
                    &done,
                );
                (id.clone(), Some(commit))
            }
            ("create", Some(Want::Connection { origin, raw, .. })) => {
                let api = need_api(&run, worker, binding)?;
                let id = api.create(&physical, origin).map_err(|e| {
                    DeployFailure::driver(format!(
                        "could not create the Hyperdrive config `{physical}` for `{worker}`'s `{binding}`: {}",
                        scrub(&e.to_string(), &[raw.expose(), origin.password.expose()])
                    ))
                })?;
                let commit = record(
                    HyperdriveRecord {
                        name: physical.clone(),
                        id: id.clone(),
                        adopted: false,
                        superseded: Vec::new(),
                    },
                    &format!("changed the Hyperdrive config `{physical}`"),
                );
                (id, Some(commit))
            }
            ("sync", Some(Want::Connection { origin, raw, .. })) => {
                let api = need_api(&run, worker, binding)?;
                let Some(old) = recorded else {
                    return Err(DeployFailure::driver(format!(
                        "no Hyperdrive config is recorded for `{worker}`'s `{binding}`"
                    )));
                };
                api.update(&old.id, &physical, origin).map_err(|e| {
                    DeployFailure::driver(format!(
                        "could not sync the Hyperdrive config `{physical}` for `{worker}`'s `{binding}`: {}",
                        scrub(&e.to_string(), &[raw.expose(), origin.password.expose()])
                    ))
                })?;
                let done = format!("changed the Hyperdrive config `{}`", old.name);
                let id = old.id.clone();
                (id, Some(record(old, &done)))
            }
            _ => {
                return Err(DeployFailure::driver(format!(
                    "no Hyperdrive settings were read for `{worker}`'s `{binding}`"
                )));
            }
        };
        Ok(Applied::bound(binding, id).with_any(commit))
    }

    fn materialise(
        config: &Path,
        worker: &str,
        bound: &BTreeMap<String, String>,
    ) -> Result<(), DeployFailure> {
        if !bound.is_empty() && !materialise_hyperdrive_ids(config, bound) {
            return Err(DeployFailure::driver(format!(
                "could not materialise the Hyperdrive config ids into `{worker}`'s generated configuration"
            )));
        }
        Ok(())
    }

    fn bound_id(record: &HyperdriveRecord) -> Option<&str> {
        Some(&record.id)
    }
}

/// One Hyperdrive config's step (1D.2b): the declaration, the config's name, what the ledger
/// records for it, and what [`action`] decided from those and the run's wants. The plan's line
/// shows the same action: it is decided once, here.
#[derive(Debug, Clone)]
pub(crate) struct HyperdriveStep<'r> {
    pub(crate) decl: &'r HyperdriveDecl,
    pub(crate) physical: String,
    pub(crate) recorded: Option<HyperdriveRecord>,
    pub(crate) action: &'static str,
}

/// `cx.worker`'s step for `decl`, from the ledger and what `run` wants.
fn hyperdrive_step<'r>(
    cx: &PlanCx<'_>,
    decl: &'r HyperdriveDecl,
    run: HyperdriveRun<'_>,
) -> HyperdriveStep<'r> {
    let physical = config_name(cx.ledger.environment, cx.worker, decl);
    let recorded = cx
        .ledger
        .recorded::<Hyperdrive>(cx.worker, &decl.binding)
        .cloned();
    let want = run
        .wants
        .get(&(cx.worker.to_string(), decl.binding.clone()));
    HyperdriveStep {
        decl,
        action: action(recorded.as_ref(), &physical, want),
        physical,
        recorded,
    }
}

/// One recorded Hyperdrive config the build no longer declares (#3319): a
/// capability removed or renamed, a config replaced, or a whole Worker gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrphanHyperdrive {
    pub(crate) worker: String,
    pub(crate) binding: String,
    /// The config's name on the account.
    pub(crate) config: String,
    /// Its id, which `--prune` deletes it by. Not serialised: the plan names
    /// the config, and an id is not for the reader.
    #[serde(skip)]
    pub(crate) id: String,
    /// An adopted config is reported and never deleted.
    pub(crate) adopted: bool,
}

/// #3319: one Hyperdrive config.
#[derive(Debug, Serialize)]
pub(crate) struct PlanHyperdrive<'a> {
    /// `HYPERDRIVE_<SNAKE>`.
    pub(crate) binding: &'a str,
    /// `create` (a config made from the connection string), `sync` (the
    /// recorded config, updated from the variable again), `reuse` (the
    /// recorded config, nothing to sync) or `adopt` (a config `bynk.toml`
    /// names by id).
    pub(crate) action: &'static str,
    /// The config's account-facing name, or for an adopted config its id.
    /// Never the connection string, or any part of it.
    pub(crate) config: String,
    /// The environment variable the connection string is read from, named so
    /// the plan says where a change would come from. A name, not a value.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) connection_env: Option<String>,
}

#[cfg(test)]
mod tests {
    // ---- 1D.1c (#3929): the `Hyperdrive` impl ----

    use super::*;
    use crate::deploy::config::tests::project;

    fn record(name: &str, id: &str, adopted: bool) -> HyperdriveRecord {
        HyperdriveRecord {
            name: name.to_string(),
            id: id.to_string(),
            adopted,
            superseded: Vec::new(),
        }
    }

    #[test]
    fn a_replaced_config_is_superseded_unless_it_was_adopted_and_forgotten_by_id() {
        let mut env = Environment::default();
        HyperdriveTable::insert(
            &mut env,
            "shop",
            "HYPERDRIVE_MAIN",
            record("shop-main", "id-1", false),
        );
        // A sync records the same config again: nothing is superseded.
        HyperdriveTable::insert(
            &mut env,
            "shop",
            "HYPERDRIVE_MAIN",
            record("shop-main", "id-1", false),
        );
        assert!(
            env.hyperdrive_instances["shop"]["HYPERDRIVE_MAIN"]
                .superseded
                .is_empty()
        );
        HyperdriveTable::insert(
            &mut env,
            "shop",
            "HYPERDRIVE_MAIN",
            record("shop-main", "id-2", false),
        );
        assert_eq!(
            env.hyperdrive_instances["shop"]["HYPERDRIVE_MAIN"].superseded,
            vec![HyperdrivePast {
                name: "shop-main".to_string(),
                id: "id-1".to_string(),
                adopted: false
            }]
        );
        HyperdriveTable::forget_config(&mut env, "shop", "HYPERDRIVE_MAIN", "id-1");
        assert!(
            env.hyperdrive_instances["shop"]["HYPERDRIVE_MAIN"]
                .superseded
                .is_empty()
        );
        HyperdriveTable::forget_config(&mut env, "shop", "HYPERDRIVE_MAIN", "id-2");
        assert_eq!(env, Environment::default());
        // Moving off an adopted config leaves nothing to report.
        HyperdriveTable::insert(
            &mut env,
            "shop",
            "HYPERDRIVE_X",
            record("theirs", "t", true),
        );
        HyperdriveTable::insert(&mut env, "shop", "HYPERDRIVE_X", record("ours", "o", false));
        assert!(
            env.hyperdrive_instances["shop"]["HYPERDRIVE_X"]
                .superseded
                .is_empty()
        );
    }

    #[test]
    fn a_bare_plan_line_is_the_ledgers_and_one_with_wants_adopts_by_id() {
        let mut lock = DeployLock::default();
        lock.record::<Hyperdrive>(
            "default",
            "shop",
            "HYPERDRIVE_MAIN",
            record("shop-main", "id-1", false),
        );
        let declared = Resources::default().hyperdrive(&["HYPERDRIVE_MAIN", "HYPERDRIVE_NEW"]);
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "default"),
            worker: "shop",
        };
        let bare: Vec<_> = Hyperdrive::declared(&declared)
            .into_iter()
            .map(|decl| Hyperdrive::plan_line(&cx, decl, HyperdriveRun::default()))
            .collect();
        let mut out = String::new();
        for line in &bare {
            Hyperdrive::short_line(line, &mut out);
        }
        assert_eq!(
            out,
            "hyperdrive reuse shop-main (HYPERDRIVE_MAIN)\nhyperdrive create shop-new (HYPERDRIVE_NEW)\n"
        );
        assert_eq!(
            Hyperdrive::listing(&bare[1]),
            None,
            "a config's name is not listed"
        );
        let wants = super::super::super::hyperdrive::Wants::from([(
            ("shop".to_string(), "HYPERDRIVE_NEW".to_string()),
            Want::Adopt {
                id: "cfg-theirs".to_string(),
            },
        )]);
        let run = HyperdriveRun {
            wants: &wants,
            api: None,
        };
        let line = Hyperdrive::plan_line(&cx, &declared.hyperdrive[1], run);
        assert_eq!((line.action, line.config.as_str()), ("adopt", "cfg-theirs"));
        // Taken from the run's lives, not some other kind's.
        let lives = Lives {
            hyperdrive: run,
            ..Lives::default()
        };
        assert_eq!(Hyperdrive::live_in(lives).wants.len(), 1);
    }

    #[test]
    fn orphans_are_undeclared_and_superseded_configs_and_only_ours_are_pruned() {
        let mut lock = DeployLock::default();
        lock.record::<Hyperdrive>(
            "default",
            "shop",
            "HYPERDRIVE_MAIN",
            record("shop-main", "id-1", false),
        );
        lock.record::<Hyperdrive>(
            "default",
            "shop",
            "HYPERDRIVE_MAIN",
            record("shop-main", "id-2", false),
        );
        lock.record::<Hyperdrive>(
            "default",
            "gone",
            "HYPERDRIVE_X",
            record("theirs", "t", true),
        );
        let resources = project(vec![(
            "shop",
            Resources::default().hyperdrive(&["HYPERDRIVE_MAIN"]),
        )]);
        let orphans = Hyperdrive::orphans(&OrphanCx {
            env: &lock.environments["default"],
            resources: &resources,
        });
        let mut out = String::new();
        for orphan in &orphans {
            Hyperdrive::short_orphan(orphan, &mut out);
        }
        assert_eq!(
            out,
            "orphan hyperdrive gone HYPERDRIVE_X (theirs; adopted, never deleted)\n\
             orphan hyperdrive shop HYPERDRIVE_MAIN (shop-main; deleted by --prune)\n"
        );
        assert_eq!(orphans[1].id, "id-1");
        assert!(!Hyperdrive::prunable(&orphans[0]));
        assert!(Hyperdrive::prunable(&orphans[1]));
        assert_eq!(
            Hyperdrive::prune_notice(&orphans[1], "default"),
            Some(Notice::Plain(
                "bynk: will delete Hyperdrive config `shop-main` of `shop`'s `HYPERDRIVE_MAIN`"
                    .to_string()
            ))
        );
    }

    /// #3970: the recorded config's id is what the deploy facts and `bynk revert` read, adopted
    /// or not.
    #[test]
    fn a_config_binds_its_recorded_id_for_the_facts_and_revert() {
        let mut lock = DeployLock::default();
        lock.record::<Hyperdrive>(
            "default",
            "shop",
            "HYPERDRIVE_MAIN",
            record("shop-main", "id-1", false),
        );
        lock.record::<Hyperdrive>(
            "default",
            "shop",
            "HYPERDRIVE_LEGACY",
            record("legacy", "id-adopted", true),
        );
        let mut ids = BTreeMap::new();
        kind_bound_ids(
            LedgerView::of(&lock, "default"),
            "shop",
            &Resources::default().hyperdrive(&["HYPERDRIVE_MAIN", "HYPERDRIVE_LEGACY"]),
            &mut ids,
        );
        assert_eq!(
            ids,
            BTreeMap::from([
                ("HYPERDRIVE_LEGACY".to_string(), "id-adopted".to_string()),
                ("HYPERDRIVE_MAIN".to_string(), "id-1".to_string()),
            ])
        );
        assert_eq!(
            recorded_bound_id(LedgerView::of(&lock, "default"), "shop", "HYPERDRIVE_MAIN"),
            Some("id-1".to_string())
        );
        // Another environment's record is not this one's.
        assert_eq!(
            recorded_bound_id(LedgerView::of(&lock, "staging"), "shop", "HYPERDRIVE_MAIN"),
            None
        );
    }

    #[test]
    fn pruning_without_an_api_leaves_the_config_and_the_ledger() {
        let mut lock = DeployLock::default();
        lock.record::<Hyperdrive>(
            "default",
            "gone",
            "HYPERDRIVE_MAIN",
            record("gone-main", "id-1", false),
        );
        let before = toml::to_string(&lock).unwrap();
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        let commit = Hyperdrive::prune(
            &StepCx {
                environment: "default",
                worker: "gone",
                project: &project,
                lock: &lock,
                live: HyperdriveRun::default(),
                tools: Tools {
                    api: &crate::cloudflare::Account::new(Wrangler::new(
                        &provenance,
                        Path::new("/nonexistent"),
                    )),
                    project_root: Path::new("/nonexistent"),
                },
            },
            &OrphanHyperdrive {
                worker: "gone".to_string(),
                binding: "HYPERDRIVE_MAIN".to_string(),
                config: "gone-main".to_string(),
                id: "id-1".to_string(),
                adopted: false,
            },
        )
        .expect("nothing to do without an API");
        assert!(commit.is_none(), "nothing deleted, so nothing forgotten");
        assert_eq!(toml::to_string(&lock).unwrap(), before);
    }

    #[test]
    fn materialising_into_a_missing_configuration_fails_with_its_worker() {
        let e = Hyperdrive::materialise(
            Path::new("/nonexistent/wrangler.toml"),
            "shop",
            &BTreeMap::from([("HYPERDRIVE_MAIN".to_string(), "id-1".to_string())]),
        )
        .unwrap_err();
        assert!(
            e.message
                .contains("could not materialise the Hyperdrive config ids into `shop`'s"),
            "{}",
            e.message
        );
    }
}
