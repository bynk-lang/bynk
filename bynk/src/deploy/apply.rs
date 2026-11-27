//! Back-half paydown 1D.2 (#4001): the apply, and the one place it writes the ledger.
//!
//! A kind never writes `bynk.deploy.lock`. Its provisioning and pruning read the ledger through a
//! [`StepCx`] and return what they would record or forget as [`Commit`]s, and [`LedgerWriter`]
//! writes each, in order, before the kind is asked for anything else: so a record still lands
//! before the next account call and before the push (ADR 0180). A failed step returns no commit,
//! so its changes are never written. The post-push record goes through the same writer, and so
//! `write_lock` has one caller in the deploy.
//!
//! What one write covers is what it covered before 1D.2 (the table in #4001's "What exists
//! today"): one per created or changed instance of KV, R2, D1, Hyperdrive and Vectorize (an index
//! before its metadata indexes); one per Worker for the rate limiters and each linked kind, only
//! when something changed, and for container images whenever the Worker has a container; one per
//! queue, after every queue is made, only when it changed the ledger; one per deleted orphan; and
//! one per pushed Worker.
//!
//! **What is applied is the plan (1D.2b).** `derive_plan` decides each Worker's typed steps from
//! its declarations and the ledger, beside the lines it prints, and holds them in its
//! [`ContextPlan`]. [`apply_worker`] executes them in [`for_each_apply_phase`]'s fixed order, one
//! [`ResourceKind::apply`] per step, each step's commits written before the next step. A step is
//! what one write covers today, so the timing cannot move: a Vectorize index is two steps, its
//! record written between them. The account check, which rewrites only the printed lines, never
//! reaches a step. Where the apply still asks the account on top of a step's decision, the kind
//! names the guard ([`ResourceKind::GUARDS`]).

use super::edge_slots::{apply_edge_deletes, record_edge_slots};
use super::*;
use bynk_driver::{colour, emsg};

/// The ledger and where it lives: the one place the deploy writes it (1D.2a, #4001).
pub(crate) struct LedgerWriter<'l> {
    lock: &'l mut DeployLock,
    path: &'l Path,
}

impl<'l> LedgerWriter<'l> {
    pub(crate) fn new(lock: &'l mut DeployLock, path: &'l Path) -> Self {
        LedgerWriter { lock, path }
    }

    /// The ledger as it stands.
    pub(crate) fn lock(&self) -> &DeployLock {
        self.lock
    }

    /// Where it is written.
    pub(crate) fn path(&self) -> &Path {
        self.path
    }

    /// Make `commit`'s changes in `environment`, in order, and write the ledger once if any of
    /// them asks it to be. A failed write is `commit`'s failure, " in {path}: {error}".
    pub(crate) fn commit<C: LedgerChange>(
        &mut self,
        environment: &str,
        commit: Commit<C>,
    ) -> Result<(), DeployFailure> {
        let mut write = false;
        for change in commit.changes {
            write |= change.apply(self.lock, environment);
        }
        if !write {
            return Ok(());
        }
        self.write().map_err(|e| {
            DeployFailure::driver(format!(
                "{} in {}: {e}",
                commit.failure,
                self.path.display()
            ))
        })
    }

    /// Change the ledger with `change`, then write it, whatever `change` did.
    pub(crate) fn change(&mut self, change: impl FnOnce(&mut DeployLock)) -> Result<(), String> {
        change(self.lock);
        self.write()
    }

    fn write(&mut self) -> Result<(), String> {
        write_lock(self.path, self.lock)
    }
}

/// Where a kind is provisioned: one Worker of one environment, in the whole project, through
/// the run's tools.
#[derive(Clone, Copy)]
pub(crate) struct Site<'a> {
    pub(crate) tools: Tools<'a>,
    pub(crate) environment: &'a str,
    pub(crate) worker: &'a str,
    pub(crate) project: &'a BTreeMap<String, Resources>,
}

impl<'a> Site<'a> {
    /// What a kind reads at this site, with the ledger as `lock` holds it now.
    pub(crate) fn cx<'c, 'l, K: ResourceKind>(
        &self,
        lock: &'c DeployLock,
        live: K::Live<'l>,
    ) -> StepCx<'c, 'l, K>
    where
        'a: 'c,
    {
        StepCx {
            environment: self.environment,
            worker: self.worker,
            project: self.project,
            lock,
            live,
            tools: self.tools,
        }
    }
}

/// Apply each of `steps` of `K` at `site`, in order, writing each step's commits before the next
/// step is applied (ADR 0180: an interrupted run never makes a second). A step that failed
/// before changing the account returns no commit; one stopped part-way returns what it did, which
/// is written before its failure is returned (#3999). What the push binds, by binding.
pub(crate) fn apply_kind<K: ResourceKind>(
    site: Site<'_>,
    steps: &[K::Step<'_>],
    writer: &mut LedgerWriter<'_>,
    lives: Lives<'_>,
) -> Result<BTreeMap<String, K::Bound>, DeployFailure> {
    let live = K::live_in(lives);
    let mut bound = BTreeMap::new();
    for step in steps {
        let applied = K::apply(&site.cx::<K>(writer.lock(), live), step)?;
        for commit in applied.commits {
            writer.commit(site.environment, commit)?;
        }
        // Said once the step's work is on the account and recorded (#4602).
        for note in &applied.notes {
            note.print();
        }
        if let Some(failure) = applied.stopped {
            return Err(failure);
        }
        bound.extend(applied.bound);
    }
    Ok(bound)
}

/// `declared`'s steps of `K` at `site`, decided from the ledger as `writer` holds it, then
/// applied: what a test of one kind drives, where the deploy applies the plan's steps.
#[cfg(test)]
pub(crate) fn provision_kind<K: ResourceKind>(
    site: Site<'_>,
    declared: &Resources,
    writer: &mut LedgerWriter<'_>,
    lives: Lives<'_>,
) -> Result<BTreeMap<String, K::Bound>, DeployFailure> {
    let steps = K::steps(
        &PlanCx {
            ledger: LedgerView::of(writer.lock(), site.environment),
            worker: site.worker,
        },
        declared,
        K::live_in(lives),
    );
    apply_kind::<K>(site, &steps, writer, lives)
}

/// Delete `orphan` of `K` from the account in `environment`, and forget it, writing the ledger.
pub(crate) fn prune_one<K: ResourceKind>(
    tools: Tools<'_>,
    writer: &mut LedgerWriter<'_>,
    environment: &str,
    orphan: &K::Orphan,
    lives: Lives<'_>,
) -> Result<(), DeployFailure> {
    let project = BTreeMap::new();
    let site = Site {
        tools,
        environment,
        worker: K::orphan_worker(orphan),
        project: &project,
    };
    let commit = K::prune(&site.cx::<K>(writer.lock(), K::live_in(lives)), orphan)?;
    match commit {
        Some(commit) => writer.commit(environment, commit),
        None => Ok(()),
    }
}

/// Delete each kind's orphans `--prune` deletes, in the list's order, each forgotten and the
/// ledger written as it goes, so a prune interrupted half-way is resumed rather than repeated.
pub(crate) fn prune_kind_orphans(
    tools: Tools<'_>,
    writer: &mut LedgerWriter<'_>,
    environment: &str,
    orphans: &Orphans,
    lives: Lives<'_>,
) -> Result<(), DeployFailure> {
    struct Prune<'p, 'w, 'v> {
        tools: Tools<'p>,
        writer: &'p mut LedgerWriter<'w>,
        environment: &'p str,
        orphans: &'p Orphans,
        lives: Lives<'v>,
        result: Result<(), DeployFailure>,
    }
    impl KindVisitor for Prune<'_, '_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            for orphan in K::orphans_in(self.orphans)
                .iter()
                .filter(|o| K::prunable(o))
            {
                if self.result.is_err() {
                    return;
                }
                self.result = prune_one::<K>(
                    self.tools,
                    self.writer,
                    self.environment,
                    orphan,
                    self.lives,
                );
            }
        }
    }
    let mut prune = Prune {
        tools,
        writer,
        environment,
        orphans,
        lives,
        result: Ok(()),
    };
    for_each_kind(&mut prune);
    prune.result
}

/// A Worker whose push succeeded, as the ledger records it: its directory (for the contracts it
/// provides and expects), its group's members and script, and what it declared.
pub(crate) struct PushedWorker<'a> {
    pub(crate) worker: &'a str,
    pub(crate) dir: std::path::PathBuf,
    pub(crate) members: Vec<String>,
    pub(crate) script: String,
    pub(crate) declared: &'a Resources,
}

impl<'a> PushedWorker<'a> {
    /// `worker`, built in `workers_dir`, in the project's `groups`.
    pub(crate) fn of(
        worker: &'a str,
        workers_dir: &Path,
        groups: &GroupNames,
        declared: &'a Resources,
    ) -> Self {
        PushedWorker {
            worker,
            dir: workers_dir.join(worker),
            members: groups.members(worker),
            script: groups.script(worker),
            declared,
        }
    }
}

/// Record that `pushed` was pushed to `environment`, and write the ledger once: its contracts, its
/// class tombstones, each kind's change after a push (a dropped rate limiter's id is free again),
/// and then `moves` (the fresh agents the run accepted). The write's error, for the caller's
/// message.
pub(crate) fn record_pushed(
    writer: &mut LedgerWriter<'_>,
    environment: &str,
    pushed: &PushedWorker<'_>,
    moves: impl FnOnce(&mut DeployLock),
) -> Result<(), String> {
    writer.change(|lock| {
        record_pushed_contracts(
            lock,
            environment,
            (pushed.worker, &pushed.dir),
            (&pushed.members, &pushed.script),
        );
        record_pushed_tombstones(lock, environment, pushed.worker, pushed.declared);
        kinds_after_push(lock, environment, pushed.worker, pushed.declared);
        moves(lock);
    })
}

/// What `run` applies a plan with (1D.2c, #4001 Decision E): the plan and its upload order, the
/// options, the tools, the project and what the run decided before the confirmation.
pub(crate) struct PlanApply<'a> {
    pub(crate) plan: &'a Plan<'a>,
    pub(crate) order: &'a [String],
    pub(crate) opts: &'a DeployOptions,
    pub(crate) tools: Tools<'a>,
    pub(crate) workers_dir: &'a Path,
    pub(crate) resources: &'a BTreeMap<String, Resources>,
    pub(crate) groups: &'a GroupNames,
    pub(crate) moves: &'a [AgentMove],
    pub(crate) keys: &'a Keys,
    pub(crate) secret_source: &'a SecretSource,
    /// Whether this wrangler takes `--secrets-file` ([`secrets_file_supported`]).
    pub(crate) with_upload: bool,
    pub(crate) runner: &'static str,
    pub(crate) lives: Lives<'a>,
}

/// Provision → wire → push, per context of the plan, in dependency order, each pushed Worker
/// recorded (moved from `run` unchanged). `Err` is the code the run exits with.
pub(crate) fn apply_plan(
    apply: &PlanApply<'_>,
    writer: &mut LedgerWriter<'_>,
) -> Result<(), ExitCode> {
    let PlanApply {
        plan,
        order,
        opts,
        tools,
        workers_dir,
        resources,
        groups,
        moves,
        keys,
        secret_source,
        with_upload,
        runner,
        lives,
    } = *apply;
    let env = opts.environment.as_str();
    // Shared across the loop so two contexts wanting the same secret prompt
    // once. Dropped with the run — nothing here is ever written (ADR 0195 D1).
    let mut resolved_secrets = BTreeMap::new();
    for (i, (worker, context)) in order.iter().zip(&plan.contexts).enumerate() {
        if order.len() > 1 {
            eprint!(
                "{}",
                deploying_notice(&opts.environment, worker, i + 1, order.len())
            );
        }
        // 1D.2b (#4001): each context's plan is what is applied, for the Worker the run records.
        debug_assert_eq!(
            context.steps().worker.map(|(w, _)| w),
            Some(worker.as_str()),
            "a plan's contexts follow its order"
        );
        let run = &mut RunCx {
            tools,
            workers_dir,
            project: resources,
            writer,
            secrets: &mut Secrets {
                source: secret_source,
                force: opts.force,
                edge_keys: &keys.edge_keys,
                expired: &keys.expired,
                cleaned: &keys.cleaned,
                foreign_keys: &keys.foreign_keys,
                foreign_secrets: &keys.foreign_secrets,
                listed: &keys.listed,
                resolved: &mut resolved_secrets,
                with_upload,
            },
            wrangler_args: &opts.wrangler_args,
            environment: &opts.environment,
            lives,
        };
        match apply_worker(run, context) {
            Ok(Pushed::Ok) => {
                // P3 (#2342): e.g. a rate limiter's id the source dropped is free again.
                let pushed = PushedWorker::of(worker, workers_dir, groups, &resources[worker]);
                let recorded = record_pushed(writer, &opts.environment, &pushed, |lock| {
                    record_fresh_moves(lock, opts, worker, moves);
                    // #4526: what its rotations left retiring, and what was cleaned up.
                    record_edge_slots(
                        lock,
                        &opts.environment,
                        worker,
                        (&keys.edge_keys, &keys.cleaned.borrow()),
                    );
                });
                // I3 (#1971): the secrets just minted for this context's
                // foreign callers, handed to their operators.
                for caller in &resources[worker].foreign_callers {
                    if let Some(ForeignKey::Set {
                        value,
                        minted: true,
                    }) = keys
                        .foreign_keys
                        .get(&(worker.clone(), caller.binding.clone()))
                    {
                        colour::eprint(&foreign_hand_off(env, worker, caller, value, runner));
                    }
                }
                if let Err(e) = recorded {
                    colour::eprint(&unrecorded_deploy_failure(
                        &opts.environment,
                        worker,
                        &writer.path().display().to_string(),
                        &e,
                    ));
                    return Err(ExitCode::FAILURE);
                }
            }
            // A shared Ctrl-C. Stop, but report nothing and exit cleanly: the
            // user asked for this, and the terminal signalled us too. `worker`
            // is deliberately *not* recorded as deployed — the push was cut
            // short, so whether it landed is unknown, and the ledger only ever
            // claims what it watched succeed.
            Ok(Pushed::Interrupted) => return Err(ExitCode::SUCCESS),
            Err(f) => {
                emsg!("bynk: error: {}", f.message);
                // Stop rather than push on: everything left in the order either
                // binds to what just failed or would be uploaded into a
                // topology that is not what the plan described. `worker` itself
                // is excluded — the line above already named it as the failure,
                // and listing it here again as "not deployed" would double-count
                // it against the number.
                colour::eprint(&stopped_report(&opts.environment, &order[i + 1..]));
                // #2004 review: a run that re-keyed an edge and stopped between
                // its two ends leaves them refusing each other. The next plain
                // run sees a key on each end and keeps both, so say how to
                // repair it.
                if let Some(hint) =
                    interrupted_rotation_hint(&opts.environment, &keys.edge_keys, &order[..i])
                {
                    colour::eprint(&hint);
                }
                // I3 (#1971): a secret minted for this context's foreign
                // caller may already be set, and the next run would keep it
                // without ever showing it, so hand it off now.
                for caller in &resources[worker].foreign_callers {
                    if let Some(ForeignKey::Set {
                        value,
                        minted: true,
                    }) = keys
                        .foreign_keys
                        .get(&(worker.clone(), caller.binding.clone()))
                    {
                        colour::eprint(&foreign_binding_maybe_set_notice(env, worker, caller));
                        colour::eprint(&foreign_hand_off(env, worker, caller, value, runner));
                    }
                }
                // Wrangler's own code, not a flat 1 (slice 0's contract).
                return Err(ExitCode::from(f.code));
            }
        }
    }

    Ok(())
}

/// `--prune`, once every context has pushed (moved from `run` unchanged).
pub(crate) fn prune_plan(
    apply: &PlanApply<'_>,
    writer: &mut LedgerWriter<'_>,
) -> Result<(), ExitCode> {
    let PlanApply {
        plan,
        opts,
        tools,
        lives,
        ..
    } = *apply;
    // Slice 5: pruning is project-wide, independent of `--context` — the
    // orphan report already is (DECISION A), so pruning follows it. Runs
    // only after every selected context has pushed cleanly: a failed deploy
    // above already returned, so a mid-flight ledger never reaches this.
    if opts.prune && plan.orphans.has_prunable() {
        if !confirm_prune(opts.yes, &plan.orphans, &opts.environment) {
            return Err(ExitCode::FAILURE);
        }
        if let Err(e) = prune_orphans(tools, writer, &opts.environment, &plan.orphans, lives) {
            emsg!("bynk: error: {}", e.message);
            return Err(ExitCode::from(e.code));
        }
    }
    Ok(())
}

/// What a run applies every Worker with (1D.2b, #4001 Decision D): the tools, the ledger's one
/// writer, the secrets, the push's arguments, the environment, the whole project (for the check
/// across it a KV create re-runs) and what the run read of the account.
pub(crate) struct RunCx<'a, 'w, 's> {
    pub(crate) tools: Tools<'a>,
    pub(crate) workers_dir: &'a Path,
    pub(crate) project: &'a BTreeMap<String, Resources>,
    pub(crate) writer: &'a mut LedgerWriter<'w>,
    pub(crate) secrets: &'a mut Secrets<'s>,
    pub(crate) wrangler_args: &'a [String],
    pub(crate) environment: &'a str,
    pub(crate) lives: Lives<'a>,
}

/// Provision and push exactly one context, by executing its plan's steps (1D.2b): what
/// `deploy_one` did with the Worker's `Resources`, in [`for_each_apply_phase`]'s order.
///
/// The phase order is the contract (ADR 0194 D3): everything the upload needs
/// to already exist — the KV namespace, the queues a `[[queues.consumers]]`
/// binding would otherwise fail against — is provisioned first; Durable
/// Object namespaces are not provisioned at all, because Cloudflare creates
/// them from the `exports` declaration in the same config `wrangler deploy`
/// is pushing (#1796).
pub(crate) fn apply_worker(
    run: &mut RunCx<'_, '_, '_>,
    context: &ContextPlan<'_>,
) -> Result<Pushed, DeployFailure> {
    let steps = context.steps();
    let Some((worker, declared)) = steps.worker else {
        return Err(DeployFailure::driver(format!(
            "the plan for `{}` names no Worker to apply",
            context.worker
        )));
    };
    let worker_dir = run.workers_dir.join(worker);
    let mut apply = ApplyWorker {
        config: worker_dir.join("wrangler.toml"),
        worker_dir,
        worker,
        declared,
        steps,
        run,
        bound: BTreeMap::new(),
        outcome: Ok(None),
    };
    for_each_apply_phase(&mut apply);
    apply
        .outcome?
        .ok_or_else(|| DeployFailure::driver(format!("`{worker}` was not pushed")))
}

/// [`apply_worker`]'s pass over one Worker. Stops at the first failure: every later phase checks
/// `outcome` first.
struct ApplyWorker<'p, 'a, 'w, 's, 'r> {
    run: &'p mut RunCx<'a, 'w, 's>,
    steps: &'p WorkerSteps<'r>,
    worker: &'r str,
    declared: &'r Resources,
    worker_dir: std::path::PathBuf,
    config: std::path::PathBuf,
    /// What each kind's steps bound, by kind, for its materialisation.
    bound: BTreeMap<KindId, Box<dyn std::any::Any>>,
    /// The push's outcome once it has run, or the failure that stopped the Worker.
    outcome: Result<Option<Pushed>, DeployFailure>,
}

impl ApplyWorker<'_, '_, '_, '_, '_> {
    /// What `K`'s steps bound, by binding.
    fn bound<K: ResourceKind>(&self) -> BTreeMap<String, K::Bound>
    where
        K::Bound: Clone,
    {
        self.bound
            .get(&K::ID)
            .and_then(|bound| bound.downcast_ref::<BTreeMap<String, K::Bound>>())
            .cloned()
            .unwrap_or_default()
    }

    fn phase(&mut self, phase: impl FnOnce(&mut Self) -> Result<(), DeployFailure>) {
        if matches!(self.outcome, Ok(None))
            && let Err(e) = phase(self)
        {
            self.outcome = Err(e);
        }
    }

    /// Slice 4 (DECISION B+C): Cloudflare does not inherit bindings into a named
    /// environment, so a non-default `--env` needs its own `[env.<name>]` table —
    /// synthesised here, not by the emitter, since the environment name is a
    /// deploy-time concept the compiler never sees. The top-level stanza is left
    /// untouched; it continues to serve the plain, no-`--env` `bynk deploy`.
    fn environment_block(&mut self) -> Result<(), DeployFailure> {
        let (worker, environment, config) = (self.worker, self.run.environment, &self.config);
        if environment == "default" {
            return Ok(());
        }
        let config_text = std::fs::read_to_string(config).map_err(|e| {
            DeployFailure::driver(format!(
                "could not read `{worker}`'s generated configuration: {e}"
            ))
        })?;
        // #3883: `addresses` is top-level only, so Wrangler applies the file's list to this
        // environment too. It is the environment's own claim (nothing, unless `[deploy.email]`
        // maps it), or `wrangler deploy --env` would take production's routing rules over.
        let config_text = crate::deploy::email::set_top_level_addresses(
            &config_text,
            &self.declared.email_addresses,
        )
        .map_err(|e| {
            DeployFailure::driver(format!(
                "could not set the Email Routing addresses of `{worker}` for `{environment}`: {e}"
            ))
        })?;
        let foreign: BTreeSet<String> = self
            .declared
            .foreign_services
            .iter()
            .map(|f| f.binding.clone())
            .collect();
        let synthesised = bynk_emit::emitter::wrangler::environment::render(
            &config_text,
            environment,
            &bynk_emit::emitter::wrangler::environment::Resolved {
                kv_ids: &self.bound::<Kv>(),
                foreign_bindings: &foreign,
            },
        )
        .map_err(|e| {
            DeployFailure::driver(format!(
                "could not synthesise the `[env.{environment}]` configuration for `{worker}`: {e}"
            ))
        })?;
        std::fs::write(config, synthesised).map_err(|e| {
            DeployFailure::driver(format!(
                "could not write `{worker}`'s `[env.{environment}]` configuration: {e}"
            ))
        })
    }

    /// P4b (#2608): each database's pending migrations, applied once it is
    /// bound (and, for a named environment, once `[env.<name>]` binds it) and
    /// before the push, so the new code never meets the old schema. Wrangler
    /// records what it applied in the database itself, so the ledger doesn't.
    fn migrations(&mut self) -> Result<(), DeployFailure> {
        let (worker, environment) = (self.worker, self.run.environment);
        for db in self
            .declared
            .d1_databases
            .iter()
            .filter(|db| !db.migrations.is_empty())
        {
            let physical = d1_database_name(environment, &db.name);
            apply_d1_migrations(
                self.run.tools.api,
                &self.worker_dir,
                &db.binding,
                environment,
            )
            .map_err(|e| {
                DeployFailure::driver(format!(
                    "could not apply the migrations of the D1 database `{physical}` for `{worker}`'s `{}`, so nothing was pushed: {e}",
                    db.binding
                ))
            })?;
        }
        Ok(())
    }
}

impl ApplyVisitor for ApplyWorker<'_, '_, '_, '_, '_> {
    fn apply<K: ResourceKind>(&mut self) {
        self.phase(|this| {
            let site = Site {
                tools: this.run.tools,
                environment: this.run.environment,
                worker: this.worker,
                project: this.run.project,
            };
            let bound = apply_kind::<K>(
                site,
                K::steps_of(this.steps),
                this.run.writer,
                this.run.lives,
            )?;
            this.bound.insert(K::ID, Box::new(bound));
            Ok(())
        });
    }

    fn materialise<K: ResourceKind>(&mut self) {
        self.phase(|this| {
            let bound = this
                .bound
                .get(&K::ID)
                .and_then(|bound| bound.downcast_ref::<BTreeMap<String, K::Bound>>());
            match bound {
                Some(bound) => K::materialise(&this.config, this.worker, bound),
                None => K::materialise(&this.config, this.worker, &BTreeMap::new()),
            }
        });
    }

    fn slot(&mut self, slot: ApplySlot) {
        let environment = self.run.environment;
        match slot {
            ApplySlot::EnvironmentBlock => self.phase(Self::environment_block),
            // #3479: an adopted index keeps the name it has on the account.
            ApplySlot::AdoptedVectorize => self.phase(|this| {
                materialise_adopted_vectorize(&this.config, this.worker, this.declared, environment)
            }),
            // #3477: an adopted container image replaces the Dockerfile `wrangler` would build,
            // in the configuration it is about to read, and is recorded before the push.
            ApplySlot::AdoptedContainers => self.phase(|this| {
                materialise_adopted_containers(
                    &this.config,
                    this.worker,
                    this.declared,
                    environment,
                )
            }),
            ApplySlot::Migrations => self.phase(Self::migrations),
            ApplySlot::Push => self.phase(|this| {
                let pushed = push_worker(this.run, this.worker, &this.worker_dir, this.declared)?;
                this.outcome = Ok(Some(pushed));
                Ok(())
            }),
        }
    }
}

/// #3073: the `--var` that sets the facts this version carries about itself,
/// for `bynk revert`.
fn facts_args(
    project_root: &Path,
    worker_dir: &Path,
    worker: &str,
    environment: &str,
    declared: &Resources,
    lock: &DeployLock,
) -> Vec<String> {
    facts::var_args(&facts::build_facts(
        worker,
        environment,
        declared,
        &read_contracts_manifest(worker_dir).unwrap_or_default(),
        lock,
        &schema_agent_versions(project_root),
        // #3686: a group's Worker carries its members' agent versions.
        &bynk_project::worker_layout::WorkerLayout::for_project(
            project_root,
            &std::collections::HashMap::new(),
        ),
    ))
}

/// The secrets and the push of `worker`, built in `worker_dir`, which declared `declared`.
fn push_worker(
    run: &mut RunCx<'_, '_, '_>,
    worker: &str,
    worker_dir: &Path,
    declared: &Resources,
) -> Result<Pushed, DeployFailure> {
    let (api, project_root, environment) = (run.tools.api, run.tools.project_root, run.environment);
    // Secrets straddle the push, and which side depends on whether the Worker
    // already exists (ADR 0195 D6).
    //
    // `wrangler secret put` against a Worker that is not on the account yet does
    // not fail — it creates a **stub draft Worker** (`export default { fetch()
    // {} }`) and puts the secret on that. Non-interactively it does so without
    // asking (its confirm falls back to yes), and interactively it prompts
    // mid-deploy — where a decline makes it exit **0** having set nothing, which
    // `deploy` would read as success and push behind. Neither is a thing to do
    // on a plan that said "deploy `api`".
    //
    // So: a Worker the ledger has pushed before exists, and its secrets are set
    // **before** the push, as the phase order intends — the running code never
    // sees a request without them.
    //
    // A first deploy uploads its secrets **with** the push (#2055):
    // `wrangler deploy --secrets-file` puts them in the new Worker's first
    // version, so it is never live without them. Setting them after the push,
    // as this did before, left a new Worker serving for a few seconds with no
    // auth secret (fail-closed, answering 401, but contrary to what the guide
    // promised). The file is the only way in for a Worker that does not exist
    // yet, and it is used only there: a value piped to `secret put` never
    // touches disk, and a file — however briefly, 0600, in a private temp
    // directory — does, so a redeploy keeps the stdin path.
    //
    // What does **not** straddle is the deciding and resolving: `prepare_bindings`
    // runs before the push on *both* paths, so a missing value is found before
    // anything is uploaded.
    let first_deploy = !run.writer.lock().is_deployed(environment, worker);
    let prepared = prepare_bindings(
        api,
        worker_dir,
        worker,
        declared,
        run.secrets,
        first_deploy,
        environment,
    )?;
    let deployed = deployed_worker_name(environment, worker);
    let facts_args = facts_args(
        project_root,
        worker_dir,
        worker,
        environment,
        declared,
        run.writer.lock(),
    );
    let wrangler_args: Vec<String> = run
        .wrangler_args
        .iter()
        .cloned()
        .chain(facts_args)
        .collect();
    let wrangler_args = wrangler_args.as_slice();
    if !first_deploy {
        apply_secrets(api, worker_dir, worker, &prepared, environment)?;
        apply_edge_deletes(api, run.secrets, worker, environment)?;
        return push(
            api,
            worker_dir,
            worker,
            wrangler_args,
            environment,
            (None, false),
        );
    }
    if prepared.is_empty() {
        return push(
            api,
            worker_dir,
            worker,
            wrangler_args,
            environment,
            (None, true),
        );
    }
    if !run.secrets.with_upload {
        // An installed wrangler older than `--secrets-file`: the old order,
        // said out loud, since the guide promises otherwise.
        emsg!(
            "bynk: warning: this wrangler is older than {SECRETS_FILE_MIN}, which cannot upload secrets with a new Worker, so `{deployed}` will be live for a moment before its secrets are set. Upgrade wrangler to close that window."
        );
        let pushed = push(
            api,
            worker_dir,
            worker,
            wrangler_args,
            environment,
            (None, true),
        )?;
        if pushed == Pushed::Ok {
            apply_secrets(api, worker_dir, worker, &prepared, environment)?;
        }
        return Ok(pushed);
    }
    let file = SecretsFile::write(&prepared).map_err(|e| {
        DeployFailure::driver(format!(
            "could not write the secrets for `{deployed}`'s first version to a temporary file: {e}"
        ))
    })?;
    let pushed = push(
        api,
        worker_dir,
        worker,
        wrangler_args,
        environment,
        (Some(file.path()), true),
    );
    // Removed whatever the push did, before anything else can fail.
    drop(file);
    let pushed = pushed?;
    if pushed == Pushed::Ok {
        report_first_version_bindings(&prepared, &deployed);
    }
    Ok(pushed)
}
#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("bynk-1d2-writer-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A change that asks for no write.
    struct Unchanged;

    impl LedgerChange for Unchanged {
        fn apply(self, _lock: &mut DeployLock, _environment: &str) -> bool {
            false
        }
    }

    #[test]
    fn a_commit_whose_changes_ask_for_no_write_writes_nothing() {
        let dir = scratch("unchanged");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        LedgerWriter::new(&mut lock, &path)
            .commit("default", Commit::of(vec![Unchanged], "never".to_string()))
            .unwrap();
        assert!(!path.exists(), "nothing was written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_record_is_made_in_the_environment_and_written() {
        let dir = scratch("record");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        LedgerWriter::new(&mut lock, &path)
            .commit(
                "staging",
                Commit::recorded(
                    TableChange::<KvTable>::insert("api", "KV", KvNamespace::of("id-1")),
                    "created it",
                ),
            )
            .unwrap();
        assert_eq!(
            lock.recorded::<Kv>("staging", "api", "KV")
                .map(|kv| kv.id.as_str()),
            Some("id-1")
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            toml::to_string_pretty(&lock).unwrap(),
            "and written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_says_what_was_done_and_where_it_could_not_be_recorded() {
        let dir = scratch("unwritable");
        // A directory where the ledger should be: the rename over it fails.
        let path = dir.join(LOCK_FILE);
        std::fs::create_dir_all(path.join("occupied")).unwrap();
        let mut lock = DeployLock::default();
        let e = LedgerWriter::new(&mut lock, &path)
            .commit(
                "default",
                Commit::recorded(
                    TableChange::<KvTable>::insert("api", "KV", KvNamespace::of("id-1")),
                    "created the KV namespace",
                ),
            )
            .unwrap_err();
        assert!(
            e.message.starts_with(&format!(
                "created the KV namespace but could not record it in {}: ",
                path.display()
            )),
            "{}",
            e.message
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn forgetting_in_an_environment_the_ledger_lacks_changes_nothing_but_still_writes() {
        let dir = scratch("forget");
        let path = dir.join(LOCK_FILE);
        let mut lock = DeployLock::default();
        LedgerWriter::new(&mut lock, &path)
            .commit(
                "default",
                Commit::recorded(TableChange::<KvTable>::remove("api", "KV"), "deleted it"),
            )
            .unwrap();
        assert_eq!(lock, DeployLock::default());
        assert!(path.exists(), "the ledger was written, as before");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
