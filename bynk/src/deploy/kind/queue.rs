//! 1D.1d (#3929): queues, each queue a Worker consumes (`from queue("n")`) or sends to
//! (`capability X = Queue @name("n")`, P2 #2578), as one [`ResourceKind`].
//!
//! A queue is an account resource addressed by name, not something a Worker owns: two contexts
//! consuming `"jobs"` mean one queue, so its records are environment-wide (`Environment::queues`,
//! with the ones only found on the account in `Environment::reused_queues`, #2579), and the run
//! attempts each queue once, across Workers. The ledger is a planning aid here, authoritative for
//! nothing (ADR 0194 D2): every queue is checked against the account on every deploy. A queue
//! nothing names any more is an orphan, which `--prune` deletes unless the project only reused it.

use super::*;

/// The queue kind.
pub(crate) struct Queue;

/// [`Queue`]'s records: environment-wide sets, by name. The Worker is not part of the key.
pub(crate) struct QueueTable;

impl QueueTable {
    /// Note that this project has created `queue`: whatever the ledger held before is stale, and
    /// the queue is Bynk's to prune now. Whether the ledger changed, so a re-run that provisions
    /// nothing also writes nothing.
    pub(crate) fn created(env: &mut Environment, queue: &str) -> bool {
        let was_reused = env.reused_queues.remove(queue);
        env.queues.insert(queue.to_string()) || was_reused
    }

    /// Note that `queue` was already on the account when this project met it (#2579): confirmed
    /// to exist, so the plan says `reuse`, but not Bynk's to prune. A queue the ledger already
    /// holds is left as it was, whichever way. Whether the ledger changed.
    pub(crate) fn reused(env: &mut Environment, queue: &str) -> bool {
        if !env.queues.insert(queue.to_string()) {
            return false;
        }
        env.reused_queues.insert(queue.to_string());
        true
    }
}

/// What provisioning and pruning change in [`Queue`]'s environment-wide records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum QueueChange {
    /// The queue, provisioned: as Bynk's when this run `created` it, as reused otherwise
    /// ([`record_provisioned_queue`]). Written only when that changed the ledger.
    Provisioned { queue: String, created: bool },
    /// The queue, deleted by `--prune`.
    Forget(String),
}

impl LedgerChange for QueueChange {
    fn apply(self, lock: &mut DeployLock, environment: &str) -> bool {
        match self {
            QueueChange::Provisioned { queue, created } => record_provisioned_queue(
                lock.environments
                    .entry(environment.to_string())
                    .or_default(),
                &queue,
                created,
            ),
            QueueChange::Forget(queue) => {
                if let Some(env) = lock.environments.get_mut(environment) {
                    QueueTable::remove(env, "", &queue);
                }
                true
            }
        }
    }
}

impl LedgerTable for QueueTable {
    /// The queue's name: the record is that the queue is there.
    type Record = String;

    fn get<'l>(env: &'l Environment, _worker: &str, queue: &str) -> Option<&'l String> {
        env.queues.get(queue)
    }

    fn insert(env: &mut Environment, _worker: &str, queue: &str, _record: String) {
        QueueTable::created(env, queue);
    }

    fn remove(env: &mut Environment, _worker: &str, queue: &str) {
        env.queues.remove(queue);
        env.reused_queues.remove(queue);
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &String)> {
        env.queues
            .iter()
            .map(|queue| ("", queue.as_str(), queue))
            .collect()
    }
}

/// One queue a Worker names: by its logical name, and whether the Worker consumes it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct QueueDecl<'r> {
    pub(crate) name: &'r str,
    pub(crate) consumed: bool,
}

impl ResourceKind for Queue {
    const ID: KindId = KindId::Queue;

    type Decl<'r> = QueueDecl<'r>;
    type Record = String;
    type Table = QueueTable;
    type Line<'r> = PlanQueue;
    /// The queue's logical name, or (as the plan shows it) its account name.
    type Orphan = String;
    /// The queues this run has already attempted, across Workers.
    type Live<'a> = Option<&'a std::cell::RefCell<BTreeSet<String>>>;
    /// Nothing: the config binds a queue by the name it already carries.
    type Bound = ();
    type Change = QueueChange;
    type Step<'r> = QueueStep<'r>;

    const GUARDS: &'static [&'static str] = &[
        "a queue this run has already attempted, for an earlier Worker, is not made again",
        "a queue is made only when `queues info` finds no queue of its name on the account",
    ];

    /// Every queue the Worker consumes or sends to, in name order, each once
    /// ([`Resources::all_queues`]).
    fn declared(r: &Resources) -> Vec<QueueDecl<'_>> {
        let all: BTreeSet<&String> = r.queues.iter().chain(&r.producer_queues).collect();
        all.into_iter()
            .map(|name| QueueDecl {
                name,
                consumed: r.queues.contains(name),
            })
            .collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        decl.name
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanQueue] {
        &context.queues
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanQueue> {
        &mut context.queues
    }

    fn live_in<'a>(lives: Lives<'a>) -> Self::Live<'a> {
        lives.queues
    }

    /// P2 (#2578): an unrecorded consumed queue is `create`, a forecast that the first deploy's
    /// record corrects. A queue only sent to is recorded only if Bynk made it, so an adopted one
    /// would read `create` forever: it is `create or reuse` until a record says otherwise.
    fn plan_line<'r>(
        cx: &PlanCx<'_>,
        decl: Self::Decl<'r>,
        _live: Self::Live<'_>,
    ) -> Self::Line<'r> {
        PlanQueue {
            action: if cx.ledger.recorded::<Queue>(cx.worker, decl.name).is_some() {
                "reuse"
            } else if decl.consumed {
                "create"
            } else {
                "create or reuse"
            },
            queue: env_qualify(cx.ledger.environment, decl.name),
        }
    }

    fn short_line(queue: &PlanQueue, out: &mut String) {
        out.push_str(&format!("queue {} {}\n", queue.action, queue.queue));
    }

    fn listing(queue: &PlanQueue) -> Option<on_account::Listing> {
        matches!(queue.action, "create" | "create or reuse").then_some((Self::ID, None))
    }

    /// A queue already on the account is `reuse`; one only sent to that is not is `create`
    /// (#3494).
    fn checked_against(
        queue: &mut PlanQueue,
        _shown_worker: &str,
        on: &on_account::OnAccount,
        _notes: &mut Vec<String>,
    ) {
        queue.action = held_verb(queue.action, on.holds(Self::ID, None, &queue.queue));
    }

    /// #3877: a queue's name (a consumer's, a producer's or a dead-letter queue's) gains `-<environment>`
    /// outside `default`, and Cloudflare takes 63 characters at most. The build can check the name alone;
    /// only the plan knows the environment, so a name that overflows once qualified is refused here, before
    /// anything is created.
    fn name_problem(cx: &ProjectCx<'_>) -> Option<String> {
        queue_name_problem(cx.resources, cx.ledger.environment)
    }

    fn orphans_in(orphans: &Orphans) -> &[String] {
        &orphans.queues
    }

    fn set_orphans(orphans: &mut Orphans, found: Vec<String>) {
        orphans.queues = found;
    }

    /// Every recorded queue no Worker of the project consumes or (P2, #2578) sends to, less the
    /// ones it only reused. A queue two contexts still consume is never an orphan because a third
    /// that used to consume it was removed.
    fn orphans(cx: &OrphanCx<'_>) -> Vec<String> {
        let live_queues: BTreeSet<&str> = cx
            .resources
            .values()
            .flat_map(|r| {
                r.queues
                    .iter()
                    .chain(&r.producer_queues)
                    .map(String::as_str)
            })
            .collect();
        cx.env
            .queues
            .iter()
            .filter(|queue| !live_queues.contains(queue.as_str()))
            .filter(|queue| !cx.env.reused_queues.contains(queue.as_str()))
            .cloned()
            .collect()
    }

    /// By the [`env_qualify`]d name `--prune` deletes (#2028).
    fn shown_orphan(queue: &String, environment: &str) -> String {
        env_qualify(environment, queue)
    }

    fn short_orphan(queue: &String, out: &mut String) {
        out.push_str(&format!("orphan queue {queue}\n"));
    }

    /// Environment-wide: no Worker's.
    fn orphan_worker(_queue: &String) -> &str {
        ""
    }

    fn prunable(_queue: &String) -> bool {
        true
    }

    /// The name `--prune` deletes, not the ledger key (#2028).
    fn prune_notice(queue: &String, environment: &str) -> Option<Notice> {
        Some(Notice::Styled(format!(
            "bynk: will delete queue `{}`",
            env_qualify(environment, queue)
        )))
    }

    /// With the project's own Workers the prune detaches first, when it read any (#4601).
    fn prune_notice_in(queue: &String, environment: &str, orphans: &Orphans) -> Option<Notice> {
        let detaching = orphans.queue_detaching.get(queue).filter(|d| !d.is_empty());
        let Some(detaching) = detaching else {
            return Self::prune_notice(queue, environment);
        };
        let named: Vec<String> = detaching.iter().map(|w| format!("`{w}`")).collect();
        Some(Notice::Styled(format!(
            "bynk: will delete queue `{}` (detaching {})",
            env_qualify(environment, queue),
            named.join(", ")
        )))
    }

    fn prune(
        cx: &StepCx<'_, '_, Queue>,
        queue: &String,
    ) -> Result<Option<Commit<QueueChange>>, DeployFailure> {
        let physical = env_qualify(cx.environment, queue);
        let own = own_workers(cx.lock, cx.environment);
        delete_queue(cx.tools.api, &physical, &own).map_err(|e| {
            DeployFailure::driver(format!(
                "could not delete the orphaned queue `{physical}`: {e}"
            ))
        })?;
        Ok(Some(Commit::recorded(
            QueueChange::Forget(queue.clone()),
            &format!("deleted the orphaned queue `{physical}`"),
        )))
    }

    /// One step for every queue the Worker names: each is made before any is recorded.
    fn steps<'r>(
        _cx: &PlanCx<'_>,
        declared: &'r Resources,
        _live: Self::Live<'_>,
    ) -> Vec<QueueStep<'r>> {
        if declared.all_queues().is_empty() {
            return Vec::new();
        }
        vec![QueueStep { declared }]
    }

    fn steps_of<'p, 'r>(steps: &'p WorkerSteps<'r>) -> &'p [QueueStep<'r>] {
        &steps.queues
    }

    fn steps_mut<'p, 'r>(steps: &'p mut WorkerSteps<'r>) -> &'p mut Vec<QueueStep<'r>> {
        &mut steps.queues
    }

    /// Reconciled against the account on every run, never against the ledger (ADR 0194 D2):
    /// each queue this run has not yet attempted is made if the account lacks it, and only then
    /// is any recorded, as Bynk's when this run made it and as reused otherwise (#2579). A queue
    /// only sent to is recorded only when this run made it (P2, #2578). The account-facing name
    /// is environment-qualified (slice 4, DECISION C); the ledger keys by the logical name.
    fn apply(
        cx: &StepCx<'_, '_, Queue>,
        step: &QueueStep<'_>,
    ) -> Result<Applied<Queue>, DeployFailure> {
        apply_queues(cx, step.declared)
    }
}

/// #3877: the first queue of `resources` whose environment-qualified name is longer than Cloudflare's 63
/// characters, as a message naming the Worker and the environment, or `None`.
pub(crate) fn queue_name_problem(
    resources: &BTreeMap<String, Resources>,
    environment: &str,
) -> Option<String> {
    for (worker, declared) in resources {
        for queue in declared.all_queues() {
            let physical = env_qualify(environment, &queue);
            if physical.len() > 63 {
                return Some(format!(
                    "queue `{physical}` (`{queue}` of `{worker}`) cannot be created in environment `{environment}`: with `-{environment}` added the name is {} characters and Cloudflare takes 63 at most; shorten the queue's name",
                    physical.len()
                ));
            }
        }
    }
    None
}

/// #4056 Decision E: whether the run's queue steps call the account's REST API. Every queue a
/// Worker declares is looked up on each deploy, recorded or not, so any declared queue does; so
/// does a `--prune` with a queue orphan to delete.
pub(crate) fn queues_calls_the_account(
    cx: &ProjectCx<'_>,
    orphans: &[String],
    prune: bool,
) -> bool {
    cx.order
        .iter()
        .any(|worker| !cx.resources[worker].all_queues().is_empty())
        || (prune && !orphans.is_empty())
}

/// A Worker's queues' step (1D.2b): every queue it consumes or sends to, read from its
/// declarations, since whether each is made is the account's to say when the step is applied.
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueueStep<'r> {
    pub(crate) declared: &'r Resources,
}

/// [`Queue`]'s step: every queue the run has not yet attempted made first, then each recorded,
/// each with its own write.
fn apply_queues(
    cx: &StepCx<'_, '_, Queue>,
    declared: &Resources,
) -> Result<Applied<Queue>, DeployFailure> {
    let fresh = std::cell::RefCell::new(BTreeSet::new());
    let attempted = cx.live.unwrap_or(&fresh);
    let mut created = BTreeSet::new();
    // Bound first, so the borrow of the run's attempts ends before any queue is made.
    let todo = unattempted_queues(declared, &mut attempted.borrow_mut());
    for queue in todo {
        match attempt_create(cx, &queue) {
            Ok(true) => {
                created.insert(queue);
            }
            Ok(false) => {}
            // #3999: the queues this step made before the failure are on the account, so they
            // are recorded as Bynk's before the failure is returned, and `--prune` can delete them.
            Err(failure) => {
                let made = created.iter().map(|queue| record(cx, queue, true));
                return Ok(made
                    .fold(Applied::nothing(), Applied::with)
                    .stopping(failure));
            }
        }
    }
    // Recorded before the push, as KV is: what the ledger claims is only ever what it
    // watched succeed. A consumed queue another context already checked this run is
    // recorded here as well, so the order contexts deploy in changes nothing.
    let mut applied = Applied::nothing();
    for queue in queues_to_record(declared, &created) {
        let made = created.contains(&queue);
        applied = applied.with(record(cx, &queue, made));
    }
    Ok(applied)
}

/// Make `queue` unless the account has it. Whether this call made it (a race-loser that finds it
/// made in between did not).
fn attempt_create(cx: &StepCx<'_, '_, Queue>, queue: &str) -> Result<bool, DeployFailure> {
    let physical = env_qualify(cx.environment, queue);
    if queue_exists(cx.tools.api, &physical) {
        return Ok(false);
    }
    create_queue(cx.tools.api, &physical)
        .map_err(|e| DeployFailure::driver(format!("could not create the queue `{physical}`: {e}")))
}

/// Record `queue`, as Bynk's when this run `created` it and as reused otherwise: written when that
/// changes the ledger.
fn record(cx: &StepCx<'_, '_, Queue>, queue: &str, created: bool) -> Commit<QueueChange> {
    let physical = env_qualify(cx.environment, queue);
    Commit::recorded(
        QueueChange::Provisioned {
            queue: queue.to_string(),
            created,
        },
        &format!("provisioned the queue `{physical}`"),
    )
}

/// #2579: record one queue in `env`, as Bynk's own when this run `created` it and as merely
/// reused otherwise. Whether the ledger changed. Pure, so the decision that keeps `--prune` off
/// another Worker's queue is tested without spawning wrangler.
pub(crate) fn record_provisioned_queue(env: &mut Environment, queue: &str, created: bool) -> bool {
    if created {
        QueueTable::created(env, queue)
    } else {
        QueueTable::reused(env, queue)
    }
}

/// One queue a context consumes or sends to.
#[derive(Debug, Serialize)]
pub(crate) struct PlanQueue {
    /// `create` when this project has never made the queue, `reuse` when the
    /// ledger has it (or #3494 the account does), `create or reuse` for a queue
    /// only sent to when the account was not asked. Either way the provision step attempts the create and
    /// treats an existing queue as success, so `reuse` is a forecast — "expect
    /// nothing new" — not a promise to stay silent (ADR 0194 D2).
    pub(crate) action: &'static str,
    /// The queue's account name, [`env_qualify`]d as the provision step
    /// creates it (#2028).
    pub(crate) queue: String,
}

/// #4601: the script of every Worker the ledger records for `environment`: the project's own,
/// the only queue consumers `--prune` detaches.
fn own_workers(lock: &DeployLock, environment: &str) -> BTreeSet<String> {
    lock.environments
        .get(environment)
        .map(|env| env.worker_scripts(environment))
        .unwrap_or_default()
}

/// #4601 (the ruling for this area with #4097): read each orphaned queue's consumers before
/// `--prune` asks. A queue only the project's own Workers consume is pruned, its Workers named
/// in the confirmation and detached first. A queue a foreign Worker or an HTTP pull consumer
/// reads (or whose consumers cannot be read) is refused: dropped from what is pruned, with why,
/// so nothing is deleted for it and it stays reported. The other orphans are untouched.
pub(crate) fn vet_orphan_queues(
    api: &dyn CloudflareApi,
    lock: &DeployLock,
    environment: &str,
    orphans: &Orphans,
) -> (Orphans, Vec<String>) {
    let own = own_workers(lock, environment);
    let mut vetted = orphans.clone();
    let mut refused = Vec::new();
    let mut detaching_of = BTreeMap::new();
    vetted.queues.retain(|queue| {
        let physical = env_qualify(environment, queue);
        match api.queue_consumers(&physical) {
            Ok(None) => true,
            Ok(Some(consumers)) => {
                let foreign: Vec<_> = consumers.iter().filter(|c| !c.is_own(&own)).collect();
                if foreign.is_empty() {
                    let detaching = consumers.iter().filter_map(|c| c.script.clone()).collect();
                    detaching_of.insert(queue.clone(), detaching);
                    true
                } else {
                    refused.push(crate::cloudflare::rest::queues::foreign_consumers(
                        &physical, &foreign,
                    ));
                    false
                }
            }
            Err(e) => {
                refused.push(format!(
                    "the consumers of the queue `{physical}` could not be read, so nothing was \
                     deleted for it: {e}"
                ));
                false
            }
        }
    });
    vetted.queue_detaching = detaching_of;
    (vetted, refused)
}

#[cfg(test)]
mod tests {
    // ---- 1D.1d (#3929): the `Queue` impl ----

    use super::*;
    use crate::cloudflare::Op;
    use crate::cloudflare::fake::Recording;
    use crate::deploy::config::tests::project;

    #[test]
    fn a_queue_bynk_made_is_its_own_and_one_it_found_is_reused_until_made() {
        let mut env = Environment::default();
        assert!(QueueTable::reused(&mut env, "theirs"));
        assert!(!QueueTable::reused(&mut env, "theirs"), "already held");
        assert!(
            QueueTable::created(&mut env, "theirs"),
            "made since: Bynk's now"
        );
        assert!(!env.reused_queues.contains("theirs"));
        assert!(!QueueTable::created(&mut env, "theirs"));
        assert_eq!(
            QueueTable::get(&env, "any-worker", "theirs").map(String::as_str),
            Some("theirs")
        );
        QueueTable::remove(&mut env, "", "theirs");
        assert_eq!(env, Environment::default());
    }

    #[test]
    fn a_plan_line_reuses_creates_or_forecasts_by_whether_the_queue_is_consumed() {
        let mut lock = DeployLock::default();
        QueueTable::created(
            lock.environments.entry("staging".to_string()).or_default(),
            "jobs",
        );
        let declared = Resources::default()
            .consumes(&["intake", "jobs"])
            .produces(&["outbound"]);
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "staging"),
            worker: "shop",
        };
        let lines: Vec<_> = Queue::declared(&declared)
            .into_iter()
            .map(|decl| Queue::plan_line(&cx, decl, None))
            .collect();
        let mut out = String::new();
        for line in &lines {
            Queue::short_line(line, &mut out);
        }
        assert_eq!(
            out,
            "queue create intake-staging\nqueue reuse jobs-staging\nqueue create or reuse outbound-staging\n"
        );
        assert_eq!(Queue::listing(&lines[1]), None);
        assert_eq!(Queue::listing(&lines[2]), Some((KindId::Queue, None)));
    }

    #[test]
    fn an_orphan_is_a_queue_no_worker_names_unless_only_reused() {
        let mut env = Environment::default();
        QueueTable::created(&mut env, "intake");
        QueueTable::created(&mut env, "old");
        QueueTable::reused(&mut env, "shared");
        let resources = project(vec![
            ("shop", Resources::default().consumes(&["intake"])),
            ("cart", Resources::default().produces(&["intake"])),
        ]);
        let orphans = Queue::orphans(&OrphanCx {
            env: &env,
            resources: &resources,
        });
        assert_eq!(orphans, vec!["old".to_string()]);
        assert_eq!(Queue::shown_orphan(&orphans[0], "staging"), "old-staging");
        assert!(Queue::prunable(&orphans[0]));
        assert_eq!(
            Queue::prune_notice(&orphans[0], "staging"),
            Some(Notice::Styled(
                "bynk: will delete queue `old-staging`".to_string()
            ))
        );
    }

    /// A Worker's queue step records a consumed queue another Worker already checked this run as
    /// reused, and a queue only sent to that this run did not make not at all. Each queue counts
    /// as attempted already this run, so nothing asks wrangler.
    #[test]
    fn a_workers_queue_step_records_a_consumed_queue_and_not_an_unmade_produced_one() {
        let declared = Resources::default()
            .consumes(&["jobs"])
            .produces(&["outbound"]);
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        let dir = std::env::temp_dir().join(format!("bynk-queue-one-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LOCK_FILE);
        let attempted =
            std::cell::RefCell::new(BTreeSet::from(["jobs".to_string(), "outbound".to_string()]));
        let site = Site {
            tools: Tools {
                api: &crate::cloudflare::Account::new(Wrangler::new(
                    &provenance,
                    Path::new("/nonexistent"),
                )),
                project_root: Path::new("/nonexistent"),
            },
            environment: "default",
            worker: "shop",
            project: &project,
        };
        let lives = Lives {
            queues: Some(&attempted),
            ..Lives::default()
        };
        let mut lock = DeployLock::default();
        provision_kind::<Queue>(
            site,
            &declared,
            &mut LedgerWriter::new(&mut lock, &path),
            lives,
        )
        .unwrap();
        let env = &lock.environments["default"];
        // Consumed and found: reused. Only sent to and not made by this run: not recorded.
        assert!(env.reused_queues.contains("jobs"));
        assert!(!env.queues.contains("outbound") && !env.reused_queues.contains("outbound"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #4056 Decision E (#4582 review): every deploy of a declared queue calls the account,
    /// recorded or not and consumed or only sent to; a queue orphan does only under `--prune`;
    /// a project with neither asks nothing.
    #[test]
    fn the_account_is_called_for_every_declared_queue_and_a_pruned_orphan() {
        let mut lock = DeployLock::default();
        QueueTable::created(
            lock.environments.entry("default".into()).or_default(),
            "jobs",
        );
        let resources = project(vec![
            ("shop", Resources::default().consumes(&["jobs"])),
            ("cart", Resources::default().produces(&["events"])),
            ("plain", Resources::default()),
        ]);
        let calls = |workers: &[&str], orphans: &[&str], prune: bool| {
            let order: Vec<String> = workers.iter().map(|w| w.to_string()).collect();
            let orphans: Vec<String> = orphans.iter().map(|o| o.to_string()).collect();
            queues_calls_the_account(
                &ProjectCx {
                    ledger: LedgerView::of(&lock, "default"),
                    resources: &resources,
                    order: &order,
                },
                &orphans,
                prune,
            )
        };
        assert!(
            calls(&["shop"], &[], false),
            "a recorded queue is still checked"
        );
        assert!(calls(&["cart"], &[], false), "so is a queue only sent to");
        assert!(
            calls(&["plain"], &["old-jobs"], true),
            "a queue orphan under --prune"
        );
        assert!(
            !calls(&["plain"], &["old-jobs"], false),
            "only under --prune"
        );
        assert!(
            !calls(&["plain"], &[], true),
            "no queue and no orphan: nothing asked"
        );
    }

    // ---- #4601: an orphaned queue's consumers, read before `--prune` asks ----

    /// A ledger whose `environment` records the Worker `shop-jobs` (the project's own).
    fn with_own_worker(environment: &str) -> DeployLock {
        let mut lock = DeployLock::default();
        lock.record_deployed(environment, "shop-jobs", None);
        lock
    }

    /// The fake account: each orphan's consumers, as [`Recording`]'s `queue_consumers` reads them.
    fn consumers_by_queue(by_queue: &'static [(&'static str, &'static str)]) -> Recording {
        Recording::new(move |op: &Op<'_>| match op {
            Op::QueueConsumers { name } => Ok::<_, CloudflareError>(
                by_queue
                    .iter()
                    .find(|(q, _)| q == name)
                    .map_or("absent", |(_, c)| c)
                    .to_string(),
            ),
            other => panic!("vetting only reads consumers, not {other:?}"),
        })
    }

    fn queues(names: &[&str]) -> Orphans {
        Orphans {
            queues: names.iter().map(|n| n.to_string()).collect(),
            ..Orphans::default()
        }
    }

    /// The ruling (#4097, #4601 review): a queue a foreign Worker or an HTTP pull consumer reads
    /// is refused, naming each with the command that detaches it; a queue only the project's own
    /// Workers consume is kept, its Workers named; one with no consumers, or already gone, is kept
    /// as it was. Vetting only reads: nothing is deleted here.
    #[test]
    fn a_foreign_or_pull_consumer_refuses_its_queue_and_the_others_are_kept() {
        let api = consumers_by_queue(&[
            ("clean", ""),
            ("theirs", "c1=billing"),
            ("pulled", "c2=pull"),
            ("ours", "c3=shop-jobs"),
            ("mixed", "c3=shop-jobs c4=billing"),
        ]);
        let lock = with_own_worker("default");
        let orphans = queues(&["clean", "theirs", "pulled", "ours", "mixed", "gone"]);
        let (vetted, refused) = vet_orphan_queues(&api, &lock, "default", &orphans);
        assert_eq!(vetted.queues, ["clean", "ours", "gone"]);
        assert_eq!(
            vetted.queue_detaching,
            BTreeMap::from([
                ("clean".to_string(), vec![]),
                ("ours".to_string(), vec!["shop-jobs".to_string()]),
            ])
        );
        assert_eq!(
            refused,
            [
                "the queue `theirs` is consumed by the Worker `billing` (not one of this \
                 project's Workers), so nothing was deleted for it: detach it by hand (`wrangler \
                 queues consumer remove theirs billing`) or keep the queue",
                "the queue `pulled` is consumed by the HTTP pull consumer `c2`, so nothing was \
                 deleted for it: detach it by hand (`wrangler queues consumer http remove \
                 pulled`) or keep the queue",
                "the queue `mixed` is consumed by the Worker `billing` (not one of this \
                 project's Workers), so nothing was deleted for it: detach it by hand (`wrangler \
                 queues consumer remove mixed billing`) or keep the queue",
            ]
        );
        let (notices, count) = prune_notices(&vetted, "default");
        assert_eq!(count, 3);
        assert!(notices.contains(&Notice::Styled(
            "bynk: will delete queue `ours` (detaching `shop-jobs`)".to_string()
        )));
        assert!(notices.contains(&Notice::Styled(
            "bynk: will delete queue `clean`".to_string()
        )));
    }

    /// In a named environment the queue and the project's Workers are both the qualified names
    /// the account holds: `shop-jobs-staging` is the project's own there, `shop-jobs` is not.
    #[test]
    fn a_named_environment_vets_the_qualified_queue_against_its_own_qualified_workers() {
        let api = consumers_by_queue(&[
            ("ours-staging", "c1=shop-jobs-staging"),
            ("theirs-staging", "c2=shop-jobs"),
        ]);
        let lock = with_own_worker("staging");
        let (vetted, refused) =
            vet_orphan_queues(&api, &lock, "staging", &queues(&["ours", "theirs"]));
        assert_eq!(vetted.queues, ["ours"]);
        assert_eq!(refused.len(), 1);
        assert!(
            refused[0].starts_with(
                "the queue `theirs-staging` is consumed by the Worker `shop-jobs` (not one of"
            ),
            "{}",
            refused[0]
        );
        assert_eq!(
            Queue::prune_notice_in(&"ours".to_string(), "staging", &vetted),
            Some(Notice::Styled(
                "bynk: will delete queue `ours-staging` (detaching `shop-jobs-staging`)"
                    .to_string()
            ))
        );
    }

    /// Consumers that cannot be read are not taken as none: the queue is refused, in
    /// Cloudflare's words.
    #[test]
    fn consumers_that_cannot_be_read_refuse_the_queue() {
        let api = Recording::new(|_: &Op<'_>| {
            Err::<String, _>(CloudflareError::Other("Cloudflare answered 500".into()))
        });
        let (vetted, refused) =
            vet_orphan_queues(&api, &DeployLock::default(), "default", &queues(&["jobs"]));
        assert!(vetted.queues.is_empty());
        assert_eq!(
            refused,
            [
                "the consumers of the queue `jobs` could not be read, so nothing was deleted for it: \
              Cloudflare answered 500"
            ]
        );
    }

    /// #3999: a queue create that fails after an earlier one succeeded leaves that earlier queue
    /// recorded as Bynk's, so `--prune` can delete it, and the failure still stops the deploy.
    #[test]
    fn the_queues_made_before_a_failed_create_are_recorded_as_created() {
        use crate::cloudflare::{CloudflareError, Op, fake::Recording};
        let declared = Resources::default().consumes(&["a-jobs", "b-jobs", "c-jobs"]);
        let api = Recording::new(|op: &Op<'_>| match op {
            Op::QueueInfo { .. } => Ok("absent".to_string()),
            Op::QueueCreate { name: "b-jobs" } => Err(CloudflareError::from(
                crate::cloudflare::rest::RestError::answered("POST", "/queues", 500, "[]"),
            )),
            _ => Ok(String::new()),
        });
        let project = BTreeMap::new();
        let dir = std::env::temp_dir().join(format!("bynk-queue-3999-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(LOCK_FILE);
        let site = Site {
            tools: Tools {
                api: &api,
                project_root: Path::new("/nonexistent"),
            },
            environment: "default",
            worker: "shop",
            project: &project,
        };
        let mut lock = DeployLock::default();
        let failed = provision_kind::<Queue>(
            site,
            &declared,
            &mut LedgerWriter::new(&mut lock, &path),
            Lives::default(),
        )
        .unwrap_err();
        assert!(
            failed
                .message
                .contains("could not create the queue `b-jobs`"),
            "{}",
            failed.message
        );
        assert_eq!(
            api.calls(),
            [
                "queues info a-jobs",
                "queues create a-jobs",
                "queues info b-jobs",
                "queues create b-jobs"
            ],
            "nothing is asked after the failure"
        );
        let env = &lock.environments["default"];
        assert!(env.queues.contains("a-jobs"), "{:?}", env.queues);
        assert!(!env.queues.contains("b-jobs") && !env.queues.contains("c-jobs"));
        assert!(env.reused_queues.is_empty());
        // Written to disk too, not only held in memory.
        let on_disk = std::fs::read_to_string(&path).unwrap();
        assert!(on_disk.contains("a-jobs"), "{on_disk}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// #4612 review: when the queues made before a failed create cannot be recorded either, both
    /// failures are reported, the create's first, so neither is lost.
    #[test]
    fn a_stopped_step_whose_record_fails_reports_both_failures() {
        use crate::cloudflare::{CloudflareError, Op, fake::Recording};
        let declared = Resources::default().consumes(&["a-jobs", "b-jobs"]);
        let api = Recording::new(|op: &Op<'_>| match op {
            Op::QueueInfo { .. } => Ok("absent".to_string()),
            Op::QueueCreate { name: "b-jobs" } => Err(CloudflareError::from(
                crate::cloudflare::rest::RestError::answered("POST", "/queues", 500, "[]"),
            )),
            _ => Ok(String::new()),
        });
        let project = BTreeMap::new();
        // A lock file in a directory that does not exist: every write fails.
        let path = std::env::temp_dir()
            .join(format!("bynk-queue-4612-missing-{}", std::process::id()))
            .join("nested")
            .join(LOCK_FILE);
        let site = Site {
            tools: Tools {
                api: &api,
                project_root: Path::new("/nonexistent"),
            },
            environment: "default",
            worker: "shop",
            project: &project,
        };
        let mut lock = DeployLock::default();
        let failed = provision_kind::<Queue>(
            site,
            &declared,
            &mut LedgerWriter::new(&mut lock, &path),
            Lives::default(),
        )
        .unwrap_err();
        let created = failed
            .message
            .find("could not create the queue `b-jobs`")
            .expect(&failed.message);
        let recorded = failed
            .message
            .find("could not record it in")
            .expect(&failed.message);
        assert!(
            created < recorded,
            "the create's failure first: {}",
            failed.message
        );
    }
}
