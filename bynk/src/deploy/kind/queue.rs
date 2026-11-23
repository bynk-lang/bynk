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

    fn prune(
        cx: &StepCx<'_, '_, Queue>,
        queue: &String,
    ) -> Result<Option<Commit<QueueChange>>, DeployFailure> {
        let physical = env_qualify(cx.environment, queue);
        delete_queue(cx.tools.api, &physical).map_err(|e| {
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
        if attempt_create(cx, &queue)? {
            created.insert(queue);
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

#[cfg(test)]
mod tests {
    // ---- 1D.1d (#3929): the `Queue` impl ----

    use super::*;
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
                api: &Wrangler::new(&provenance, Path::new("/nonexistent")),
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
}
