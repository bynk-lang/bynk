//! 1D.1d (#3929): rate limiters, each `RateLimit` instance (`capability Login = RateLimit …`), as
//! one [`ResourceKind`].
//!
//! A limiter has no account resource: its `namespace_id` is the ledger's to allocate (P3, #2342),
//! unique across every environment of the ledger, since environments may share an account. The
//! ids a Worker needs are allocated together and recorded in one write before the push, so an
//! interrupted run never allocates a second id and resets a limiter's counters. A limiter the
//! source no longer declares is not an orphan: it is forgotten once the Worker's push has
//! succeeded ([`ResourceKind::after_push`]), when the live Worker no longer binds its id.

use super::*;

/// The rate limiter kind.
pub(crate) struct RateLimit;

/// [`RateLimit`]'s records: `Environment::ratelimit_instances`, by Worker and binding.
pub(crate) struct RateLimitTable;

impl RateLimitNamespace {
    /// The record of namespace `id`.
    pub(crate) fn of(id: &str) -> Self {
        RateLimitNamespace {
            namespace_id: id.to_string(),
        }
    }
}

impl LedgerTable for RateLimitTable {
    type Record = RateLimitNamespace;

    fn get<'l>(
        env: &'l Environment,
        worker: &str,
        binding: &str,
    ) -> Option<&'l RateLimitNamespace> {
        env.ratelimit_instances
            .get(worker)
            .and_then(|instances| instances.get(binding))
    }

    fn insert(env: &mut Environment, worker: &str, binding: &str, record: RateLimitNamespace) {
        env.ratelimit_instances
            .entry(worker.to_string())
            .or_default()
            .insert(binding.to_string(), record);
    }

    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if let Some(instances) = env.ratelimit_instances.get_mut(worker) {
            instances.remove(binding);
            if instances.is_empty() {
                env.ratelimit_instances.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &RateLimitNamespace)> {
        env.ratelimit_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances
                    .iter()
                    .map(move |(binding, limiter)| (worker.as_str(), binding.as_str(), limiter))
            })
            .collect()
    }
}

/// The `namespace_id` for `worker`'s rate limiter `binding`: the recorded one, else a new one —
/// the first id from its [`ratelimit_seed`] upward (wrapping within the span) that no rate limiter
/// in any environment of the ledger holds — put in the ledger (not yet written). `true` when it
/// was allocated.
fn allocate(cx: &mut ApplyCx<'_, '_, RateLimit>, binding: &str) -> (String, bool) {
    let worker = cx.worker;
    if let Some(recorded) = cx.ledger().recorded::<RateLimit>(worker, binding) {
        return (recorded.namespace_id.clone(), false);
    }
    let id = {
        let ledger = cx.ledger();
        let taken: BTreeSet<&str> = ledger
            .all()
            .flat_map(|(_, env)| RateLimitTable::entries(env))
            .map(|(_, _, r)| r.namespace_id.as_str())
            .collect();
        let seed = ratelimit_seed(cx.environment, worker, binding) - FIRST_RATELIMIT_NAMESPACE;
        (0..RATELIMIT_NAMESPACE_SPAN)
            .map(|k| {
                (FIRST_RATELIMIT_NAMESPACE + (seed + k) % RATELIMIT_NAMESPACE_SPAN).to_string()
            })
            .find(|n| !taken.contains(n.as_str()))
            .expect("an unused id")
    };
    RateLimitTable::insert(cx.env_mut(), worker, binding, RateLimitNamespace::of(&id));
    (id, true)
}

/// [`allocate`] in `lock`, without writing it: what the ledger tests drive.
#[cfg(test)]
pub(crate) fn allocate_in(
    lock: &mut DeployLock,
    environment: &str,
    worker: &str,
    binding: &str,
) -> (String, bool) {
    let provenance = Provenance::Path("/nonexistent/wrangler".into());
    let project = BTreeMap::new();
    allocate(
        &mut ApplyCx {
            environment,
            worker,
            project: &project,
            lock,
            lock_path: Path::new("/nonexistent/bynk.deploy.lock"),
            live: (),
            tools: Tools {
                provenance: &provenance,
                project_root: Path::new("/nonexistent"),
            },
        },
        binding,
    )
}

impl ResourceKind for RateLimit {
    const ID: KindId = KindId::RateLimit;

    /// The binding, `RATELIMIT_<SNAKE>`.
    type Decl<'r> = &'r str;
    type Record = RateLimitNamespace;
    type Table = RateLimitTable;
    type Line<'r> = PlanRateLimit<'r>;
    type Orphan = NoOrphan;
    type Live<'a> = ();
    /// The `namespace_id`.
    type Bound = String;

    fn declared(r: &Resources) -> Vec<&str> {
        r.ratelimits.iter().map(String::as_str).collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        decl
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanRateLimit<'r>] {
        &context.ratelimits
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanRateLimit<'r>> {
        &mut context.ratelimits
    }

    fn live_in<'a>(_lives: Lives<'a>) -> Self::Live<'a> {}

    fn plan_line<'r>(cx: &PlanCx<'_>, binding: Self::Decl<'r>, _live: ()) -> PlanRateLimit<'r> {
        let recorded = cx.ledger.recorded::<RateLimit>(cx.worker, binding);
        PlanRateLimit {
            binding,
            action: if recorded.is_some() {
                "reuse"
            } else {
                "allocate"
            },
            namespace_id: recorded.map(|r| r.namespace_id.clone()),
        }
    }

    fn short_line(limiter: &PlanRateLimit<'_>, out: &mut String) {
        match &limiter.namespace_id {
            Some(id) => out.push_str(&format!(
                "ratelimit {} {} (namespace {id})\n",
                limiter.action, limiter.binding
            )),
            None => out.push_str(&format!(
                "ratelimit {} {}\n",
                limiter.action, limiter.binding
            )),
        }
    }

    /// An id allocated in CI and never committed would be allocated again next run, possibly
    /// another number, resetting the limiter's counters (P3, #2342).
    fn unrecordable_in_ci(cx: &ProjectCx<'_>) -> Option<String> {
        let ledger = cx.ledger;
        let (worker, binding) = cx.order.iter().find_map(|worker| {
            cx.resources[worker]
                .ratelimits
                .iter()
                .find(|binding| ledger.recorded::<RateLimit>(worker, binding).is_none())
                .map(|binding| (worker.as_str(), binding.as_str()))
        })?;
        Some(format!(
            "bynk: error: the rate limiter `{binding}` of `{worker}` is unrecorded; deploy locally first and commit {LOCK_FILE}"
        ))
    }

    fn shown_orphan(orphan: &NoOrphan, _environment: &str) -> NoOrphan {
        match *orphan {}
    }

    fn short_orphan(orphan: &NoOrphan, _out: &mut String) {
        match *orphan {}
    }

    fn orphan_worker(orphan: &NoOrphan) -> &str {
        match *orphan {}
    }

    /// One limiter alone, recorded at once. The deploy provisions a Worker's limiters together
    /// ([`ResourceKind::provision_all`]).
    fn provision(
        cx: &mut ApplyCx<'_, '_, RateLimit>,
        binding: &str,
    ) -> Result<Provisioned<String>, DeployFailure> {
        let worker = cx.worker;
        let (id, allocated) = allocate(cx, binding);
        if allocated {
            cx.commit(&format!(
                "allocated `{worker}`'s rate limiter ids but could not record them"
            ))?;
        }
        Ok(Provisioned {
            bound: Some((binding.to_string(), id)),
        })
    }

    /// Every limiter's id allocated (no account call: the id is ours to choose), then recorded in
    /// one write before the push.
    fn provision_all(
        cx: &mut ApplyCx<'_, '_, RateLimit>,
        declared: &Resources,
    ) -> Result<BTreeMap<String, String>, DeployFailure> {
        let worker = cx.worker;
        let mut ids = BTreeMap::new();
        let mut ledger_changed = false;
        for binding in &declared.ratelimits {
            let (id, allocated) = allocate(cx, binding);
            ledger_changed |= allocated;
            ids.insert(binding.clone(), id);
        }
        if ledger_changed {
            cx.commit(&format!(
                "allocated `{worker}`'s rate limiter ids but could not record them"
            ))?;
        }
        Ok(ids)
    }

    /// A limiter holds no data and has no account resource, so a removed instance needs nothing
    /// deleted, only its row dropped (its id is then free again).
    fn after_push(env: &mut Environment, worker: &str, declared: &Resources) {
        let Some(instances) = env.ratelimit_instances.get_mut(worker) else {
            return;
        };
        instances.retain(|binding, _| declared.ratelimits.contains(binding));
        if instances.is_empty() {
            env.ratelimit_instances.remove(worker);
        }
    }

    fn materialise(
        config: &Path,
        worker: &str,
        bound: &BTreeMap<String, String>,
    ) -> Result<(), DeployFailure> {
        if !bound.is_empty() && !materialise_ratelimit_ids(config, bound) {
            return Err(DeployFailure::driver(format!(
                "could not materialise the rate limiter ids into `{worker}`'s generated configuration"
            )));
        }
        Ok(())
    }

    fn bound_id(limiter: &RateLimitNamespace) -> Option<&str> {
        Some(&limiter.namespace_id)
    }
}

/// P3 (#2342): one rate limiter. Its `namespace_id` is the ledger's to
/// allocate, so the plan can only say whether one is recorded; the push
/// allocates and records the missing ones.
#[derive(Debug, Serialize)]
pub(crate) struct PlanRateLimit<'a> {
    /// `RATELIMIT_<SNAKE>`.
    pub(crate) binding: &'a str,
    /// `allocate` or `reuse`.
    pub(crate) action: &'static str,
    /// The recorded id, when `reuse`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) namespace_id: Option<String>,
}

#[cfg(test)]
mod tests {
    // ---- 1D.1d (#3929): the `RateLimit` impl ----

    use super::*;

    #[test]
    fn an_id_is_allocated_once_and_never_one_another_environment_holds() {
        let mut lock = DeployLock::default();
        let (id, allocated) = allocate_in(&mut lock, "default", "api", "RATELIMIT_A");
        assert!(allocated);
        assert_eq!(
            allocate_in(&mut lock, "default", "api", "RATELIMIT_A"),
            (id.clone(), false)
        );
        let (other, _) = allocate_in(&mut lock, "staging", "api", "RATELIMIT_A");
        assert_ne!(other, id, "unique across environments sharing an account");
    }

    #[test]
    fn a_plan_line_allocates_or_reuses_and_names_the_recorded_id() {
        let mut lock = DeployLock::default();
        lock.record::<RateLimit>(
            "default",
            "shop",
            "RATELIMIT_LOGIN",
            RateLimitNamespace::of("1001"),
        );
        let declared = Resources::default().ratelimits(&["RATELIMIT_LOGIN", "RATELIMIT_SIGNUP"]);
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "default"),
            worker: "shop",
        };
        let mut out = String::new();
        for decl in RateLimit::declared(&declared) {
            RateLimit::short_line(&RateLimit::plan_line(&cx, decl, ()), &mut out);
        }
        assert_eq!(
            out,
            "ratelimit reuse RATELIMIT_LOGIN (namespace 1001)\nratelimit allocate RATELIMIT_SIGNUP\n"
        );
    }

    #[test]
    fn a_limiter_the_source_dropped_is_forgotten_after_the_push_and_its_id_is_bound() {
        let mut lock = DeployLock::default();
        lock.record::<RateLimit>(
            "default",
            "shop",
            "RATELIMIT_LOGIN",
            RateLimitNamespace::of("1001"),
        );
        lock.record::<RateLimit>(
            "default",
            "shop",
            "RATELIMIT_OLD",
            RateLimitNamespace::of("1002"),
        );
        let declared = Resources::default().ratelimits(&["RATELIMIT_LOGIN"]);
        let mut ids = BTreeMap::new();
        kind_bound_ids(
            LedgerView::of(&lock, "default"),
            "shop",
            &declared,
            &mut ids,
        );
        assert_eq!(
            ids,
            BTreeMap::from([("RATELIMIT_LOGIN".to_string(), "1001".to_string())])
        );
        kinds_after_push(&mut lock, "default", "shop", &declared);
        assert!(
            lock.recorded::<RateLimit>("default", "shop", "RATELIMIT_OLD")
                .is_none()
        );
        kinds_after_push(&mut lock, "default", "shop", &Resources::default());
        assert!(
            !lock.environments["default"]
                .ratelimit_instances
                .contains_key("shop")
        );
    }

    #[test]
    fn a_workers_limiters_are_recorded_in_one_write_whose_failure_says_them() {
        let mut lock = DeployLock::default();
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        let e = RateLimit::provision_all(
            &mut ApplyCx {
                environment: "default",
                worker: "shop",
                project: &project,
                lock: &mut lock,
                lock_path: Path::new("/nonexistent/bynk.deploy.lock"),
                live: (),
                tools: Tools {
                    provenance: &provenance,
                    project_root: Path::new("/nonexistent"),
                },
            },
            &Resources::default().ratelimits(&["RATELIMIT_A", "RATELIMIT_B"]),
        )
        .unwrap_err();
        assert!(
            e.message
                .starts_with("allocated `shop`'s rate limiter ids but could not record them in "),
            "{}",
            e.message
        );
        assert_eq!(
            lock.environments["default"].ratelimit_instances["shop"].len(),
            2
        );
    }
}
