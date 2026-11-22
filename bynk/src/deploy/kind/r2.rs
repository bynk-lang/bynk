//! 1D.1b (#3929): R2, each `R2` instance's bucket (`capability Archives = R2`), as one
//! [`ResourceKind`].
//!
//! A bucket has no minted id: its environment-qualified name is its identity, so the ledger
//! records the name and the jurisdiction it was made in (P5, #2323), in `Environment::r2_instances`.
//! A changed `@jurisdiction` makes a new bucket, and the old one, with its objects, is kept as a
//! superseded record and reported as an orphan. Orphans are never deleted: a bucket holds data.

use super::*;

/// The R2 kind.
pub(crate) struct R2;

/// [`R2`]'s records: `Environment::r2_instances`, by Worker and binding.
pub(crate) struct R2Table;

impl R2Bucket {
    /// The record of bucket `name`, made in `jurisdiction`, with nothing superseded yet.
    pub(crate) fn of(name: &str, jurisdiction: Option<&str>) -> Self {
        R2Bucket {
            name: name.to_string(),
            jurisdiction: jurisdiction.map(str::to_string),
            superseded: Vec::new(),
        }
    }
}

impl LedgerTable for R2Table {
    type Record = R2Bucket;

    fn get<'l>(env: &'l Environment, worker: &str, binding: &str) -> Option<&'l R2Bucket> {
        env.r2_instances
            .get(worker)
            .and_then(|instances| instances.get(binding))
    }

    /// A bucket this replaces is not forgotten: its objects are still there, so it joins the
    /// new record's `superseded` (with the ones it had already), unless it is the same bucket.
    fn insert(env: &mut Environment, worker: &str, binding: &str, record: R2Bucket) {
        let instances = env.r2_instances.entry(worker.to_string()).or_default();
        let mut superseded = Vec::new();
        if let Some(old) = instances.remove(binding) {
            superseded = old.superseded;
            if old.name != record.name || old.jurisdiction != record.jurisdiction {
                superseded.push(R2Past {
                    name: old.name,
                    jurisdiction: old.jurisdiction,
                });
            }
        }
        instances.insert(
            binding.to_string(),
            R2Bucket {
                superseded,
                ..record
            },
        );
    }

    /// Never called by the deploy: a bucket's record is removed by hand. Drops a Worker's map
    /// once it is empty.
    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if let Some(instances) = env.r2_instances.get_mut(worker) {
            instances.remove(binding);
            if instances.is_empty() {
                env.r2_instances.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &R2Bucket)> {
        env.r2_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances
                    .iter()
                    .map(move |(binding, bucket)| (worker.as_str(), binding.as_str(), bucket))
            })
            .collect()
    }
}

impl ResourceKind for R2 {
    const ID: KindId = KindId::R2;

    type Decl<'r> = &'r R2Decl;
    type Record = R2Bucket;
    type Table = R2Table;
    type Line<'r> = PlanR2<'r>;
    type Orphan = OrphanR2;
    type Live<'a> = ();
    /// Nothing: the config binds a bucket by the name it already carries.
    type Bound = ();
    type Change = TableChange<R2Table>;
    type Step<'r> = R2Step<'r>;

    fn declared(r: &Resources) -> Vec<&R2Decl> {
        r.r2_buckets.iter().collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        &decl.binding
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanR2<'r>] {
        &context.r2
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanR2<'r>> {
        &mut context.r2
    }

    fn live_in<'a>(_lives: Lives<'a>) -> Self::Live<'a> {}

    fn plan_line<'r>(cx: &PlanCx<'_>, bucket: Self::Decl<'r>, _live: ()) -> PlanR2<'r> {
        PlanR2 {
            binding: &bucket.binding,
            action: r2_step(cx, bucket).decision.action(),
            bucket: r2_bucket_name(cx.ledger.environment, &bucket.name),
            jurisdiction: bucket.jurisdiction.as_deref(),
        }
    }

    fn steps<'r>(cx: &PlanCx<'_>, declared: &'r Resources, _live: ()) -> Vec<R2Step<'r>> {
        declared
            .r2_buckets
            .iter()
            .map(|bucket| r2_step(cx, bucket))
            .collect()
    }

    fn steps_of<'p, 'r>(steps: &'p WorkerSteps<'r>) -> &'p [R2Step<'r>] {
        &steps.r2
    }

    fn steps_mut<'p, 'r>(steps: &'p mut WorkerSteps<'r>) -> &'p mut Vec<R2Step<'r>> {
        &mut steps.r2
    }

    fn short_line(r2: &PlanR2<'_>, out: &mut String) {
        out.push_str(&format!("r2 {} {} ({})", r2.action, r2.bucket, r2.binding));
        if let Some(jurisdiction) = r2.jurisdiction {
            out.push_str(&format!(" in jurisdiction {jurisdiction}"));
        }
        if r2.action == "recreate" {
            out.push_str(" — the old bucket and its objects stay where they are");
        }
        out.push('\n');
    }

    /// A bucket's listing is per jurisdiction: the same name in another is another bucket.
    fn listing(r2: &PlanR2<'_>) -> Option<on_account::Listing> {
        matches!(r2.action, "create" | "create or reuse")
            .then(|| (Self::ID, r2.jurisdiction.map(str::to_string)))
    }

    /// A bucket of its name the account already holds is `reuse` (#3494): the deploy adopts it.
    fn checked_against(
        r2: &mut PlanR2<'_>,
        _shown_worker: &str,
        on: &on_account::OnAccount,
        _notes: &mut Vec<String>,
    ) {
        r2.action = held_verb(r2.action, on.holds(Self::ID, r2.jurisdiction, &r2.bucket));
    }

    fn name_problem(cx: &ProjectCx<'_>) -> Option<String> {
        r2_bucket_problem(cx.resources, cx.ledger)
    }

    fn orphans_in(orphans: &Orphans) -> &[OrphanR2] {
        &orphans.r2
    }

    fn set_orphans(orphans: &mut Orphans, found: Vec<OrphanR2>) {
        orphans.r2 = found;
    }

    /// Each recorded bucket its Worker no longer declares (or whose Worker is gone), then each
    /// bucket a changed jurisdiction left behind, which is an orphan while its binding is live.
    fn orphans(cx: &OrphanCx<'_>) -> Vec<OrphanR2> {
        let resources = cx.resources;
        cx.env
            .r2_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances
                    .iter()
                    .filter(move |(binding, _)| {
                        resources
                            .get(worker)
                            .is_none_or(|r| !r.r2_buckets.iter().any(|b| &b.binding == *binding))
                    })
                    .map(move |(binding, bucket)| OrphanR2 {
                        worker: worker.clone(),
                        binding: binding.clone(),
                        bucket: bucket.name.clone(),
                        jurisdiction: bucket.jurisdiction.clone(),
                    })
            })
            .chain(cx.env.r2_instances.iter().flat_map(|(worker, instances)| {
                instances.iter().flat_map(move |(binding, bucket)| {
                    bucket.superseded.iter().map(move |past| OrphanR2 {
                        worker: worker.clone(),
                        binding: binding.clone(),
                        bucket: past.name.clone(),
                        jurisdiction: past.jurisdiction.clone(),
                    })
                })
            }))
            .collect()
    }

    fn shown_orphan(r2: &OrphanR2, _environment: &str) -> OrphanR2 {
        r2.clone()
    }

    fn short_orphan(r2: &OrphanR2, out: &mut String) {
        out.push_str(&format!(
            "orphan r2 {} {} ({}{}; not deleted)\n",
            r2.worker,
            r2.binding,
            r2.bucket,
            r2.jurisdiction
                .as_deref()
                .map(|j| format!(", jurisdiction {j}"))
                .unwrap_or_default()
        ));
    }

    fn orphan_worker(r2: &OrphanR2) -> &str {
        &r2.worker
    }

    /// P5 (#2323): each `R2` bucket, made by name (the name is the identity, so an
    /// existing one is the same end state) and recorded before the push. A bucket
    /// the ledger already holds under the same jurisdiction is not asked of the
    /// account again. Buckets are named as `environment` names them in the config
    /// the push reads: the top level already carries the default environment's, and
    /// the synthesised `[env.<name>]` block the rest.
    fn apply(cx: &StepCx<'_, '_, R2>, step: &R2Step<'_>) -> Result<Applied<R2>, DeployFailure> {
        let (worker, environment, bucket) = (cx.worker, cx.environment, step.bucket);
        if step.decision == R2Decision::Reuse {
            return Ok(Applied::nothing());
        }
        let physical = r2_bucket_name(environment, &bucket.name);
        create_r2_bucket(cx.tools.api, &physical, bucket.jurisdiction.as_deref()).map_err(|e| {
            DeployFailure::driver(format!(
                "could not create the R2 bucket `{physical}` for `{worker}`'s `{}`: {e}",
                bucket.binding
            ))
        })?;
        Ok(Applied::nothing().with(cx.recording(
            &bucket.binding,
            R2Bucket::of(&physical, bucket.jurisdiction.as_deref()),
            &format!("created the R2 bucket `{physical}`"),
        )))
    }
}

/// One recorded R2 bucket the build no longer declares (P5, #2323): a
/// capability removed or renamed, or a whole Worker gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrphanR2 {
    pub(crate) worker: String,
    pub(crate) binding: String,
    /// The bucket's name on the account, which stays there.
    pub(crate) bucket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) jurisdiction: Option<String>,
}

/// P5 (#2323): one R2 bucket. The name is the identity, so there is no id to
/// mint and `reuse` is a plain statement of the ledger.
#[derive(Debug, Serialize)]
pub(crate) struct PlanR2<'a> {
    /// `R2_<SNAKE>`.
    pub(crate) binding: &'a str,
    /// `create`, `reuse`, or `recreate`: a changed `@jurisdiction` makes a new
    /// bucket, and the objects of the old one do not move (see [`R2Decision`]).
    /// #3494: a bucket of its name the account already holds is `reuse`.
    pub(crate) action: &'static str,
    /// The bucket's account-facing name, [`r2_bucket_name`]d.
    pub(crate) bucket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) jurisdiction: Option<&'a str>,
}

/// P5 (#2323): what a deploy will do about `worker`'s R2 bucket `bucket` in
/// `environment`. The name is the identity, so a recorded bucket under the
/// same jurisdiction is `reuse`; one recorded under another is `recreate`,
/// since a bucket's jurisdiction is fixed when it is made.
fn r2_decision(ledger: LedgerView<'_>, worker: &str, bucket: &R2Decl) -> R2Decision {
    let environment = ledger.environment;
    match ledger.recorded::<R2>(worker, &bucket.binding) {
        None => R2Decision::Create,
        Some(recorded)
            if recorded.name == r2_bucket_name(environment, &bucket.name)
                && recorded.jurisdiction == bucket.jurisdiction =>
        {
            R2Decision::Reuse
        }
        Some(recorded) if recorded.name == r2_bucket_name(environment, &bucket.name) => {
            R2Decision::Recreate
        }
        Some(_) => R2Decision::Create,
    }
}

/// One R2 bucket's step (1D.2b): the declaration, and what the ledger decided for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct R2Step<'r> {
    pub(crate) bucket: &'r R2Decl,
    pub(crate) decision: R2Decision,
}

/// What the ledger decides for one R2 bucket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum R2Decision {
    /// The ledger records it, under the same jurisdiction: nothing to ask the account.
    Reuse,
    /// The ledger does not, or records another name: made by name and recorded.
    Create,
    /// The ledger records it under another jurisdiction: a new bucket beside the old.
    Recreate,
}

impl R2Decision {
    /// The plan's word for the decision.
    pub(crate) fn action(self) -> &'static str {
        match self {
            R2Decision::Reuse => "reuse",
            R2Decision::Create => "create",
            R2Decision::Recreate => "recreate",
        }
    }
}

/// `worker`'s step for `bucket`, from the ledger.
fn r2_step<'r>(cx: &PlanCx<'_>, bucket: &'r R2Decl) -> R2Step<'r> {
    R2Step {
        bucket,
        decision: r2_decision(cx.ledger, cx.worker, bucket),
    }
}

/// P5 (#2323): why the buckets the build declares cannot all be made in
/// `environment`, or `None`. A derived name that breaks R2's rules is an error
/// at plan time, naming the instance, never a failed push; and two instances
/// (or one here and one another environment's) must not share a name, since
/// R2 scopes bucket names to the account.
pub(crate) fn r2_bucket_problem(
    resources: &BTreeMap<String, Resources>,
    ledger: LedgerView<'_>,
) -> Option<String> {
    let environment = ledger.environment;
    let mut seen: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for (worker, declared) in resources {
        for bucket in &declared.r2_buckets {
            let name = r2_bucket_name(environment, &bucket.name);
            if let Some(why) = r2_bucket_name_problem(&name) {
                return Some(format!(
                    "R2 bucket `{name}` (`{}` of `{worker}`) cannot be created: {why}; rename the capability or the context",
                    bucket.binding
                ));
            }
            if let Some((other_worker, other_binding)) =
                seen.insert(name.clone(), (worker, &bucket.binding))
            {
                return Some(format!(
                    "`{}` of `{worker}` and `{other_binding}` of `{other_worker}` would both be the R2 bucket `{name}`; bucket names are scoped to the account, so rename one capability",
                    bucket.binding
                ));
            }
            for (other_env, env) in ledger.all() {
                let clash = env
                    .r2_instances
                    .iter()
                    .flat_map(|(w, instances)| instances.iter().map(move |(b, r)| (w, b, r)))
                    .find(|(w, b, r)| {
                        r.name == name
                            && (other_env != environment
                                || w.as_str() != worker
                                || **b != bucket.binding)
                    });
                if let Some((other_worker, other_binding, _)) = clash {
                    return Some(format!(
                        "`{}` of `{worker}` would be the R2 bucket `{name}`, which `{other_binding}` of `{other_worker}` already holds in environment `{other_env}`; bucket names are scoped to the account, so rename one capability",
                        bucket.binding
                    ));
                }
            }
        }
    }
    None
}

/// Create the R2 bucket `name`, in `jurisdiction` when given, or accept that it
/// is already there (P5, #2323): as for a queue, Cloudflare addresses a bucket
/// by the name the config binds, so a created bucket and an existing one are
/// the same end state. Run from the project root, as KV and queues are, so
/// wrangler cannot load and reject an incomplete config.
pub(crate) fn create_r2_bucket(
    api: &dyn CloudflareApi,
    name: &str,
    jurisdiction: Option<&str>,
) -> Result<(), String> {
    match api.r2_create(name, jurisdiction) {
        Ok(()) => Ok(()),
        Err(e) if e.already_exists() => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_existing_bucket_you_own_is_success_and_anothers_is_not() {
        let r2_bucket_already_exists = |stderr: &str| {
            crate::cloudflare::fake::failed(
                &crate::cloudflare::Op::R2Create {
                    name: "b",
                    jurisdiction: None,
                },
                "",
                stderr,
            )
            .already_exists()
        };
        assert!(r2_bucket_already_exists(
            "The bucket you tried to create already exists, and you own it. [code: 10004]"
        ));
        assert!(!r2_bucket_already_exists(
            "The requested bucket name is not available. [code: 10006]"
        ));
        assert!(!r2_bucket_already_exists("authentication error"));
    }

    // ---- 1D.1b (#3929): the `R2` impl ----

    use crate::deploy::config::tests::project;

    #[test]
    fn a_new_bucket_supersedes_the_one_it_replaces_unless_it_is_the_same() {
        let mut env = Environment::default();
        R2Table::insert(
            &mut env,
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", None),
        );
        // The same bucket again supersedes nothing.
        R2Table::insert(
            &mut env,
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", None),
        );
        assert!(env.r2_instances["shop"]["R2_FILES"].superseded.is_empty());
        R2Table::insert(
            &mut env,
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", Some("eu")),
        );
        let record = R2Table::get(&env, "shop", "R2_FILES").expect("recorded");
        assert_eq!(record.jurisdiction.as_deref(), Some("eu"));
        assert_eq!(
            record.superseded,
            vec![R2Past {
                name: "shop-files".to_string(),
                jurisdiction: None
            }]
        );
        assert_eq!(R2Table::entries(&env).len(), 1);
        R2Table::remove(&mut env, "shop", "R2_FILES");
        assert_eq!(env, Environment::default());
    }

    #[test]
    fn a_plan_line_reuses_recreates_or_creates_by_name_and_jurisdiction() {
        let mut lock = DeployLock::default();
        lock.record::<R2>(
            "default",
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", None),
        );
        lock.record::<R2>(
            "default",
            "shop",
            "R2_ARCHIVE",
            R2Bucket::of("shop-archive", None),
        );
        let declared = Resources::default().r2_buckets(&[
            ("R2_ARCHIVE", "shop-archive", Some("eu")),
            ("R2_FILES", "shop-files", None),
            ("R2_NEW", "shop-new", None),
        ]);
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "default"),
            worker: "shop",
        };
        let lines: Vec<_> = R2::declared(&declared)
            .into_iter()
            .map(|decl| R2::plan_line(&cx, decl, ()))
            .collect();
        let mut out = String::new();
        for line in &lines {
            R2::short_line(line, &mut out);
        }
        assert_eq!(
            out,
            "r2 recreate shop-archive (R2_ARCHIVE) in jurisdiction eu — the old bucket and its objects stay where they are\n\
             r2 reuse shop-files (R2_FILES)\n\
             r2 create shop-new (R2_NEW)\n"
        );
        assert_eq!(R2::listing(&lines[0]), None);
        assert_eq!(R2::listing(&lines[2]), Some((KindId::R2, None)));
    }

    #[test]
    fn orphans_are_undeclared_buckets_then_superseded_ones_and_none_is_pruned() {
        let mut lock = DeployLock::default();
        lock.record::<R2>(
            "default",
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", None),
        );
        lock.record::<R2>(
            "default",
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", Some("eu")),
        );
        lock.record::<R2>("default", "gone", "R2_OLD", R2Bucket::of("gone-old", None));
        let resources = project(vec![(
            "shop",
            Resources::default().r2_buckets(&[("R2_FILES", "shop-files", Some("eu"))]),
        )]);
        let orphans = R2::orphans(&OrphanCx {
            env: &lock.environments["default"],
            resources: &resources,
        });
        let mut out = String::new();
        for orphan in &orphans {
            R2::short_orphan(orphan, &mut out);
            assert!(!R2::prunable(orphan));
            assert_eq!(R2::prune_notice(orphan, "default"), None);
        }
        assert_eq!(
            out,
            "orphan r2 gone R2_OLD (gone-old; not deleted)\n\
             orphan r2 shop R2_FILES (shop-files; not deleted)\n"
        );
    }

    #[test]
    fn a_bucket_binds_no_id_for_the_facts_or_revert() {
        let mut lock = DeployLock::default();
        lock.record::<R2>(
            "default",
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", None),
        );
        assert_eq!(
            recorded_bound_id(LedgerView::of(&lock, "default"), "shop", "R2_FILES"),
            None
        );
    }

    #[test]
    fn a_recorded_bucket_is_reused_without_an_account_call() {
        let mut lock = DeployLock::default();
        lock.record::<R2>(
            "default",
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", None),
        );
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        let bucket = R2Decl {
            binding: "R2_FILES".to_string(),
            name: "shop-files".to_string(),
            jurisdiction: None,
        };
        let step = r2_step(
            &PlanCx {
                ledger: LedgerView::of(&lock, "default"),
                worker: "shop",
            },
            &bucket,
        );
        assert_eq!(step.decision, R2Decision::Reuse);
        let done = R2::apply(
            &StepCx {
                environment: "default",
                worker: "shop",
                project: &project,
                lock: &lock,
                live: (),
                tools: Tools {
                    api: &Wrangler::new(&provenance, Path::new("/nonexistent")),
                    provenance: &provenance,
                    project_root: Path::new("/nonexistent"),
                },
            },
            &step,
        )
        .expect("nothing to create");
        assert!(
            done.bound.is_empty() && done.commits.is_empty(),
            "nothing bound or recorded"
        );
    }
}
