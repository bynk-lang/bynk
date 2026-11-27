//! 1D.1b (#3929): D1, each `D1` instance's database (`capability Orders = D1`), as one
//! [`ResourceKind`].
//!
//! A database is created by its environment-qualified name, and the id Cloudflare mints is recorded
//! before the push (P4a, #2602; ADR 0194), in `Environment::d1_instances`. One of its name already
//! on the account is adopted. A changed name makes a new database, and the old one, with its rows,
//! is kept as a superseded record and reported as an orphan. Orphans are never deleted. Its pending
//! migrations are applied before the push (P4b, #2608), and `bynk revert` checks the bound id.

use super::*;
use crate::cloudflare::CloudflareError;

/// The D1 kind.
pub(crate) struct D1;

/// [`D1`]'s records: `Environment::d1_instances`, by Worker and binding.
pub(crate) struct D1Table;

impl D1Record {
    /// The record of database `name` with id `id`, with nothing superseded yet.
    pub(crate) fn of(name: &str, id: &str, jurisdiction: Option<&str>) -> Self {
        D1Record {
            name: name.to_string(),
            id: id.to_string(),
            jurisdiction: jurisdiction.map(str::to_string),
            superseded: Vec::new(),
        }
    }
}

impl LedgerTable for D1Table {
    type Record = D1Record;

    fn get<'l>(env: &'l Environment, worker: &str, binding: &str) -> Option<&'l D1Record> {
        env.d1_instances
            .get(worker)
            .and_then(|instances| instances.get(binding))
    }

    /// A database this replaces is not forgotten: its rows are still there, so it joins the new
    /// record's `superseded` (with the ones it had already), unless it is the same database.
    fn insert(env: &mut Environment, worker: &str, binding: &str, record: D1Record) {
        let instances = env.d1_instances.entry(worker.to_string()).or_default();
        let mut superseded = Vec::new();
        if let Some(old) = instances.remove(binding) {
            superseded = old.superseded;
            if old.name != record.name || old.id != record.id {
                superseded.push(D1Past {
                    name: old.name,
                    id: old.id,
                });
            }
        }
        instances.insert(
            binding.to_string(),
            D1Record {
                superseded,
                ..record
            },
        );
    }

    /// Never called by the deploy: a database's record is removed by hand. Drops a Worker's map
    /// once it is empty.
    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if let Some(instances) = env.d1_instances.get_mut(worker) {
            instances.remove(binding);
            if instances.is_empty() {
                env.d1_instances.remove(worker);
            }
        }
    }

    fn entries(env: &Environment) -> Vec<(&str, &str, &D1Record)> {
        env.d1_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances
                    .iter()
                    .map(move |(binding, db)| (worker.as_str(), binding.as_str(), db))
            })
            .collect()
    }
}

impl ResourceKind for D1 {
    const ID: KindId = KindId::D1;

    type Decl<'r> = &'r D1Decl;
    type Record = D1Record;
    type Table = D1Table;
    type Line<'r> = PlanD1<'r>;
    type Orphan = OrphanD1;
    type Live<'a> = ();
    /// The database's account-facing name and its id, which the config binds it by.
    type Bound = (String, String);
    type Change = TableChange<D1Table>;
    type Step<'r> = D1Step<'r>;

    fn declared(r: &Resources) -> Vec<&D1Decl> {
        r.d1_databases.iter().collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        &decl.binding
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanD1<'r>] {
        &context.d1
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanD1<'r>> {
        &mut context.d1
    }

    fn live_in<'a>(_lives: Lives<'a>) -> Self::Live<'a> {}

    fn plan_line<'r>(cx: &PlanCx<'_>, db: Self::Decl<'r>, _live: ()) -> PlanD1<'r> {
        let step = d1_step(cx, db);
        PlanD1 {
            binding: &db.binding,
            action: step.action(),
            database: step.database,
            migrations: &db.migrations,
            jurisdiction: db.jurisdiction.as_deref(),
        }
    }

    fn steps<'r>(cx: &PlanCx<'_>, declared: &'r Resources, _live: ()) -> Vec<D1Step<'r>> {
        declared
            .d1_databases
            .iter()
            .map(|db| d1_step(cx, db))
            .collect()
    }

    fn steps_of<'p, 'r>(steps: &'p WorkerSteps<'r>) -> &'p [D1Step<'r>] {
        &steps.d1
    }

    fn steps_mut<'p, 'r>(steps: &'p mut WorkerSteps<'r>) -> &'p mut Vec<D1Step<'r>> {
        &mut steps.d1
    }

    /// The database's line, then (P4b, #2608) its migrations, applied after the database is
    /// bound and before the push.
    fn short_line(d1: &PlanD1<'_>, out: &mut String) {
        out.push_str(&format!(
            "d1 {} {} ({})",
            d1.action, d1.database, d1.binding
        ));
        if let Some(jurisdiction) = d1.jurisdiction {
            out.push_str(&format!(" in jurisdiction {jurisdiction}"));
        }
        out.push('\n');
        if !d1.migrations.is_empty() {
            let n = d1.migrations.len();
            out.push_str(&format!(
                "d1 migrate {} ({n} file{}; pending ones applied before the push)\n",
                d1.database,
                if n == 1 { "" } else { "s" }
            ));
        }
    }

    fn listing(d1: &PlanD1<'_>) -> Option<on_account::Listing> {
        matches!(d1.action, "create" | "create or reuse").then_some((Self::ID, None))
    }

    /// A database of its name the account already holds is `reuse` (#3494): the deploy adopts it.
    fn checked_against(
        d1: &mut PlanD1<'_>,
        _shown_worker: &str,
        on: &on_account::OnAccount,
        notes: &mut Vec<String>,
    ) {
        let held = on.holds(Self::ID, None, &d1.database);
        // #3820: a database already on the account that the ledger does not hold would be
        // adopted, and it was not created in the jurisdiction the source names: say so, and do
        // not call it `reuse`.
        if let (Some(jurisdiction), Some(true), "create" | "create or reuse") =
            (d1.jurisdiction, held, d1.action)
        {
            notes.push(format!(
                "d1 {} already exists on the account and the ledger does not hold it; its jurisdiction cannot be confirmed, so the deploy will refuse to adopt it as `{jurisdiction}`",
                d1.database
            ));
            return;
        }
        d1.action = held_verb(d1.action, held);
    }

    fn name_problem(cx: &ProjectCx<'_>) -> Option<String> {
        d1_database_problem(cx.resources, cx.ledger)
    }

    fn orphans_in(orphans: &Orphans) -> &[OrphanD1] {
        &orphans.d1
    }

    fn set_orphans(orphans: &mut Orphans, found: Vec<OrphanD1>) {
        orphans.d1 = found;
    }

    /// Each recorded database its Worker no longer declares (or whose Worker is gone), then each
    /// database a changed name left behind, which is an orphan while its binding is live.
    fn orphans(cx: &OrphanCx<'_>) -> Vec<OrphanD1> {
        let resources = cx.resources;
        cx.env
            .d1_instances
            .iter()
            .flat_map(|(worker, instances)| {
                instances
                    .iter()
                    .filter(move |(binding, _)| {
                        resources
                            .get(worker)
                            .is_none_or(|r| !r.d1_databases.iter().any(|d| &d.binding == *binding))
                    })
                    .map(move |(binding, db)| OrphanD1 {
                        worker: worker.clone(),
                        binding: binding.clone(),
                        database: db.name.clone(),
                    })
            })
            .chain(cx.env.d1_instances.iter().flat_map(|(worker, instances)| {
                instances.iter().flat_map(move |(binding, db)| {
                    db.superseded.iter().map(move |past| OrphanD1 {
                        worker: worker.clone(),
                        binding: binding.clone(),
                        database: past.name.clone(),
                    })
                })
            }))
            .collect()
    }

    fn shown_orphan(d1: &OrphanD1, _environment: &str) -> OrphanD1 {
        d1.clone()
    }

    fn short_orphan(d1: &OrphanD1, out: &mut String) {
        out.push_str(&format!(
            "orphan d1 {} {} ({}; not deleted)\n",
            d1.worker, d1.binding, d1.database
        ));
    }

    fn orphan_worker(d1: &OrphanD1) -> &str {
        &d1.worker
    }

    /// P4a (#2602): each `D1` database, created by name with the id Cloudflare
    /// mints recorded before the push (an interrupted run never makes a second
    /// one), then bound by that name and id. One of the same name already on
    /// the account is adopted, as a bucket is: the name is account-unique and
    /// derived from this project's, so it is this project's. A database the
    /// ledger already holds under this environment's name is not asked of
    /// the account again.
    fn apply(cx: &StepCx<'_, '_, D1>, step: &D1Step<'_>) -> Result<Applied<D1>, DeployFailure> {
        let (worker, db, physical) = (cx.worker, step.db, step.database.clone());
        if let Some(id) = &step.recorded {
            return Ok(Applied::bound(&db.binding, (physical, id.clone())));
        }
        let (id, adopted) = create_d1(cx.tools.api, &physical, db.jurisdiction.as_deref())
            .map_err(|e| {
                DeployFailure::driver(format!(
                    "could not create the D1 database `{physical}` for `{worker}`'s `{}`: {e}",
                    db.binding
                ))
            })?;
        // #3820: a database that was already there was not created in the jurisdiction, and
        // Cloudflare does not say which it has, so the ledger is never told it is pinned.
        if let (true, Some(jurisdiction)) = (adopted, db.jurisdiction.as_deref()) {
            return Err(DeployFailure::driver(format!(
                "the D1 database `{physical}` for `{worker}`'s `{}` already exists on the account, so it was not created in jurisdiction `{jurisdiction}`, and Bynk cannot read the jurisdiction of an existing database back; it will not record one it did not apply. Check the database in the Cloudflare dashboard, then either remove `@jurisdiction` to adopt it as it is, or delete it so the next deploy creates it in `{jurisdiction}`",
                db.binding
            )));
        }
        let done = if adopted {
            format!("adopted the D1 database `{physical}` already on the account")
        } else {
            format!("created the D1 database `{physical}`")
        };
        let record = cx.recording(
            &db.binding,
            D1Record::of(&physical, &id, db.jurisdiction.as_deref()),
            &done,
        );
        Ok(Applied::bound(&db.binding, (physical, id)).with(record))
    }

    fn materialise(
        config: &Path,
        worker: &str,
        bound: &BTreeMap<String, (String, String)>,
    ) -> Result<(), DeployFailure> {
        if !bound.is_empty() && !materialise_d1_databases(config, bound) {
            return Err(DeployFailure::driver(format!(
                "could not materialise the D1 database ids into `{worker}`'s generated configuration"
            )));
        }
        Ok(())
    }

    fn bound_id(db: &D1Record) -> Option<&str> {
        Some(&db.id)
    }
}

/// One recorded D1 database the build no longer declares (P4a, #2602): a
/// capability removed or renamed, or a whole Worker gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrphanD1 {
    pub(crate) worker: String,
    pub(crate) binding: String,
    /// The database's name on the account, which stays there.
    pub(crate) database: String,
}

/// P4a (#2602): one D1 database.
#[derive(Debug, Serialize)]
pub(crate) struct PlanD1<'a> {
    /// `D1_<SNAKE>`.
    pub(crate) binding: &'a str,
    /// `create` or `reuse` (the one the ledger recorded, by its id, or #3494
    /// one of its name the account already holds, which the deploy adopts).
    pub(crate) action: &'static str,
    /// The database's account-facing name, [`d1_database_name`]d.
    pub(crate) database: String,
    /// #3820: the jurisdiction the database is created in, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) jurisdiction: Option<&'a str>,
    /// P4b (#2608): its migration files. `bynk deploy` applies the ones the
    /// database hasn't recorded before the push; the plan does not read the
    /// databases, so it can't say which those are.
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    pub(crate) migrations: &'a [String],
}

/// P4a (#2602): why the databases the build declares cannot all be made in
/// `environment`, or `None`. Deploy adopts a database whose name is already on
/// the account, so two instances sharing a derived name would silently share
/// one database: `shop.cart`'s `Orders` and `shop`'s `CartOrders` are both
/// `shop-cart-orders`, and `shop`'s `OrdersStaging` by default is `shop`'s
/// `Orders` under `--env staging`. Checked within the build, and against
/// every database the ledger records in any environment.
pub(crate) fn d1_database_problem(
    resources: &BTreeMap<String, Resources>,
    ledger: LedgerView<'_>,
) -> Option<String> {
    let environment = ledger.environment;
    let mut seen: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for (worker, declared) in resources {
        for db in &declared.d1_databases {
            let name = d1_database_name(environment, &db.name);
            // #3820: a database's jurisdiction is fixed when it is made. A source that now names
            // another is refused, never recreated: the database holds data.
            let provisioned = ledger
                .recorded::<D1>(worker, &db.binding)
                .filter(|recorded| recorded.name == name);
            if let Some(recorded) = provisioned.filter(|r| r.jurisdiction != db.jurisdiction) {
                let shown = |j: &Option<String>| match j {
                    Some(j) => format!("jurisdiction `{j}`"),
                    None => "no jurisdiction".to_string(),
                };
                return Some(format!(
                    "`{}` of `{worker}` is the D1 database `{name}`, which was created with {}; the source now asks for {}. A database's jurisdiction is fixed when it is created, and `bynk deploy` will not recreate a database that holds data. Restore the `@jurisdiction` the database was created with, or create a new `D1` capability, move the data across, and remove this one",
                    db.binding,
                    shown(&recorded.jurisdiction),
                    shown(&db.jurisdiction)
                ));
            }
            if let Some((other_worker, other_binding)) =
                seen.insert(name.clone(), (worker, &db.binding))
            {
                return Some(format!(
                    "`{}` of `{worker}` and `{other_binding}` of `{other_worker}` would both be the D1 database `{name}`; database names are scoped to the account, so rename one capability",
                    db.binding
                ));
            }
            for (other_env, env) in ledger.all() {
                let clash = env
                    .d1_instances
                    .iter()
                    .flat_map(|(w, instances)| instances.iter().map(move |(b, r)| (w, b, r)))
                    .find(|(w, b, r)| {
                        r.name == name
                            && (other_env != environment
                                || w.as_str() != worker
                                || **b != db.binding)
                    });
                if let Some((other_worker, other_binding, _)) = clash {
                    return Some(format!(
                        "`{}` of `{worker}` would be the D1 database `{name}`, which `{other_binding}` of `{other_worker}` already holds in environment `{other_env}`; database names are scoped to the account, so rename one capability",
                        db.binding
                    ));
                }
            }
        }
    }
    None
}

/// P4b (#2608): `wrangler d1 migrations apply <binding> --remote` from the
/// Worker's directory, which applies the files of the entry's
/// `migrations_dir` that the database hasn't recorded, each in its own
/// transaction. The binding names the entry in every environment, where the
/// database's name doesn't. Stdin is closed, so wrangler takes its "yes"
/// fallback instead of asking: the deploy's own confirmation already covered
/// this.
pub(crate) fn apply_d1_migrations(
    api: &dyn CloudflareApi,
    worker_dir: &Path,
    binding: &str,
    environment: &str,
) -> Result<(), CloudflareError> {
    api.d1_migrations_apply(worker_dir, binding, environment)
        .map_err(CloudflareError::from)
}

/// Create the D1 database `name` and return its `database_id` and whether it was adopted (not
/// created), the one already there under that name (P4a, #2602).
///
/// Since 1D.4b (#4056) the adoption is read from the account, not from an error code: Cloudflare's
/// reference documents no "already exists" code for D1. A create that Cloudflare refused
/// ([`CloudflareError::is_refusal`]) is followed by a lookup of that exact name, and the database
/// is adopted only when it is found; otherwise the create's own error stands.
pub(crate) fn create_d1(
    api: &dyn CloudflareApi,
    name: &str,
    jurisdiction: Option<&str>,
) -> Result<(String, bool), CloudflareError> {
    let e = match api.d1_create(name, jurisdiction) {
        Ok(id) => return Ok((id, false)),
        Err(e) => e,
    };
    if !e.is_refusal() {
        return Err(e);
    }
    match api.d1_find(name) {
        Ok(Some(id)) => Ok((id, true)),
        Ok(None) => Err(e),
        Err(lookup) => Err(format!(
            "{e} (looking the database up by name to adopt it also failed: {lookup})"
        )
        .into()),
    }
}

/// One D1 database's step (1D.2b): the declaration, its name in this environment, and the id the
/// ledger records for it under that name, if it does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct D1Step<'r> {
    pub(crate) db: &'r D1Decl,
    pub(crate) database: String,
    /// Reused when `Some`; created (or adopted, by name) and recorded when `None`.
    pub(crate) recorded: Option<String>,
}

impl D1Step<'_> {
    /// The plan's word for the step.
    pub(crate) fn action(&self) -> &'static str {
        if self.recorded.is_some() {
            "reuse"
        } else {
            "create"
        }
    }
}

/// `worker`'s step for `db`, from the ledger.
fn d1_step<'r>(cx: &PlanCx<'_>, db: &'r D1Decl) -> D1Step<'r> {
    let database = d1_database_name(cx.ledger.environment, &db.name);
    let recorded = cx
        .ledger
        .recorded::<D1>(cx.worker, &db.binding)
        .filter(|recorded| recorded.name == database)
        .map(|recorded| recorded.id.clone());
    D1Step {
        db,
        database,
        recorded,
    }
}

/// Whether this run's D1 steps call the account (#4056 Decision E): a database in `cx.order` the
/// ledger does not record under this environment's name, which is created (or adopted). A
/// recorded database is reused with no call; its migrations still run through wrangler, which a
/// `wrangler login` session covers.
pub(crate) fn d1_calls_the_account(cx: &ProjectCx<'_>) -> bool {
    cx.order.iter().any(|worker| {
        let plan = PlanCx {
            ledger: cx.ledger,
            worker,
        };
        cx.resources[worker]
            .d1_databases
            .iter()
            .any(|db| d1_step(&plan, db).recorded.is_none())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1D.4b (#4056 Decision E): an unrecorded bucket or database needs the credentials and is
    /// refused without them, the first kind in order reporting; a redeploy with everything
    /// recorded asks for nothing, whatever it migrates.
    #[test]
    fn the_pre_flight_refuses_an_unrecorded_bucket_or_database_without_credentials() {
        use crate::cloudflare::CloudflareError;
        use crate::cloudflare::rest::Purpose;
        use crate::deploy::config::tests::project;
        let mut recorded = DeployLock::default();
        recorded.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1", None),
        );
        recorded.record::<R2>(
            "default",
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", None),
        );
        let mut declared = Resources::default()
            .d1_databases(&[("D1_ORDERS", "shop-orders")])
            .r2_buckets(&[("R2_FILES", "shop-files", None)]);
        declared.d1_databases[0].migrations = vec!["0001_init.sql".to_string()];
        let resources = project(vec![("shop", declared)]);
        let order = vec!["shop".to_string()];
        let asked = std::cell::RefCell::new(Vec::new());
        let missing = |purpose: Purpose| -> Result<(), CloudflareError> {
            asked.borrow_mut().push(purpose);
            Err(CloudflareError::Credentials(format!(
                "set CLOUDFLARE_API_TOKEN to {}: {} and the account it belongs to",
                purpose.doing(),
                purpose.needs()
            )))
        };
        let run = |lock: &DeployLock| {
            crate::deploy::gates::resource_credentials(
                &ProjectCx {
                    ledger: LedgerView::of(lock, "default"),
                    resources: &resources,
                    order: &order,
                },
                &[],
                &[],
                &[],
                false,
                missing,
            )
        };
        assert_eq!(run(&recorded), Ok(()));
        assert!(
            asked.borrow().is_empty(),
            "everything recorded: nothing asked"
        );
        // Nothing recorded: R2 comes before D1 and reports.
        let refusal = run(&DeployLock::default()).unwrap_err();
        assert_eq!(
            refusal,
            "set CLOUDFLARE_API_TOKEN to create the account's R2 buckets: an API token with \"Workers R2 Storage: Edit\" (a `wrangler login` session is not enough: the deploy calls Cloudflare's API directly) and the account it belongs to"
        );
        assert_eq!(asked.borrow().as_slice(), [Purpose::R2]);
        // Only the database unrecorded: D1 reports, naming its own permission.
        let mut bucket_only = DeployLock::default();
        bucket_only.record::<R2>(
            "default",
            "shop",
            "R2_FILES",
            R2Bucket::of("shop-files", None),
        );
        let refusal = run(&bucket_only).unwrap_err();
        assert!(
            refusal.contains("to create the account's D1 databases")
                && refusal.contains("\"D1: Edit\""),
            "{refusal}"
        );
    }

    /// 1D.4b (#4056): a refused create adopts only the database the account holds under exactly
    /// that name, whatever code the refusal carried; a denied or failed create is never followed
    /// by a lookup.
    #[test]
    fn a_refused_create_adopts_by_exact_name_whatever_the_code() {
        use crate::cloudflare::fake::Recording;
        use crate::cloudflare::{CloudflareError, Op, RestError};
        let run = |create: (u16, &'static str), found: &'static str| {
            let api = Recording::new(move |op| -> Result<String, CloudflareError> {
                match op {
                    Op::D1Create { .. } => {
                        Err(RestError::answered("POST", "/d1/database", create.0, create.1).into())
                    }
                    Op::D1List => Ok(found.to_string()),
                    _ => Ok(String::new()),
                }
            });
            (create_d1(&api, "shop-orders", None), api.calls())
        };
        // An undocumented code, a 409 and a plain 400 adopt alike when the name is found.
        for refusal in [
            (
                400,
                r#"[{"code":7502,"message":"A database with that name already exists"}]"#,
            ),
            (409, r#"[{"code":10073}]"#),
            (400, "[]"),
        ] {
            let (got, calls) = run(refusal, "u-1");
            assert_eq!(got.unwrap(), ("u-1".to_string(), true));
            assert_eq!(calls, ["d1 create shop-orders", "d1 list --json"]);
        }
        // Not found: the create's own error stands.
        let (got, _) = run((400, r#"[{"code":7502}]"#), "");
        assert_eq!(
            got.unwrap_err().to_string(),
            r#"POST /d1/database: Cloudflare answered 400 [{"code":7502}]"#
        );
        // A denied token or a server error is never answered by a lookup.
        for failure in [(403, r#"[{"code":10000}]"#), (500, "[]")] {
            let (got, calls) = run(failure, "u-1");
            assert!(got.is_err());
            assert_eq!(calls, ["d1 create shop-orders"]);
        }
        // A lookup that fails says so, after the create's own error.
        let api = Recording::new(|op| -> Result<String, CloudflareError> {
            match op {
                Op::D1Create { .. } => {
                    Err(RestError::answered("POST", "/d1/database", 400, "[]").into())
                }
                _ => Err(RestError::answered("GET", "/d1/database?name=x", 500, "[]").into()),
            }
        });
        assert_eq!(
            create_d1(&api, "x", None).unwrap_err().to_string(),
            "POST /d1/database: Cloudflare answered 400 [] (looking the database up by name to adopt it also failed: GET /d1/database?name=x: Cloudflare answered 500 [])"
        );
    }

    // ---- 1D.1b (#3929): the `D1` impl ----

    use crate::deploy::config::tests::project;

    #[test]
    fn a_new_database_supersedes_the_one_it_replaces_unless_it_is_the_same() {
        let mut env = Environment::default();
        D1Table::insert(
            &mut env,
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1", None),
        );
        D1Table::insert(
            &mut env,
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1", None),
        );
        assert!(env.d1_instances["shop"]["D1_ORDERS"].superseded.is_empty());
        D1Table::insert(
            &mut env,
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders-v2", "id-2", None),
        );
        let record = D1Table::get(&env, "shop", "D1_ORDERS").expect("recorded");
        assert_eq!(
            (record.name.as_str(), record.id.as_str()),
            ("shop-orders-v2", "id-2")
        );
        assert_eq!(
            record.superseded,
            vec![D1Past {
                name: "shop-orders".to_string(),
                id: "id-1".to_string()
            }]
        );
        D1Table::remove(&mut env, "shop", "D1_ORDERS");
        assert_eq!(env, Environment::default());
    }

    #[test]
    fn a_plan_line_reuses_a_database_recorded_under_its_name_and_says_it_migrates() {
        let mut lock = DeployLock::default();
        lock.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1", None),
        );
        let mut declared = Resources::default()
            .d1_databases(&[("D1_NEW", "shop-new"), ("D1_ORDERS", "shop-orders")]);
        declared.d1_databases[1].migrations = vec!["0001_init.sql".to_string()];
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "default"),
            worker: "shop",
        };
        let lines: Vec<_> = D1::declared(&declared)
            .into_iter()
            .map(|decl| D1::plan_line(&cx, decl, ()))
            .collect();
        let mut out = String::new();
        for line in &lines {
            D1::short_line(line, &mut out);
        }
        assert_eq!(
            out,
            "d1 create shop-new (D1_NEW)\n\
             d1 reuse shop-orders (D1_ORDERS)\n\
             d1 migrate shop-orders (1 file; pending ones applied before the push)\n"
        );
        assert_eq!(D1::listing(&lines[0]), Some((KindId::D1, None)));
        assert_eq!(D1::listing(&lines[1]), None);
        // Under another environment the same source is another database.
        let staging = PlanCx {
            ledger: LedgerView::of(&lock, "staging"),
            worker: "shop",
        };
        assert_eq!(
            D1::plan_line(&staging, &declared.d1_databases[1], ()).action,
            "create"
        );
    }

    #[test]
    fn a_database_on_the_account_is_reused_and_one_not_read_is_left_alone() {
        struct Holds;
        impl on_account::Listings for Holds {
            fn names(
                &self,
                _: KindId,
                _: Option<&str>,
            ) -> Result<BTreeSet<String>, crate::cloudflare::CloudflareError> {
                Ok(BTreeSet::from(["shop-new".to_string()]))
            }
        }
        let on = on_account::OnAccount::read(&BTreeSet::from([(KindId::D1, None)]), &Holds);
        let mut line = PlanD1 {
            binding: "D1_NEW",
            action: "create",
            database: "shop-new".to_string(),
            jurisdiction: None,
            migrations: &[],
        };
        D1::checked_against(&mut line, "shop", &on, &mut Vec::new());
        assert_eq!(line.action, "reuse");
        let unread = on_account::OnAccount::default();
        let mut line = PlanD1 {
            action: "create",
            ..line
        };
        D1::checked_against(&mut line, "shop", &unread, &mut Vec::new());
        assert_eq!(line.action, "create");
    }

    #[test]
    fn orphans_are_undeclared_databases_then_superseded_ones_and_the_id_is_bound() {
        let mut lock = DeployLock::default();
        lock.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1", None),
        );
        lock.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders-v2", "id-2", None),
        );
        lock.record::<D1>(
            "default",
            "shop",
            "D1_OLD",
            D1Record::of("shop-old", "id-3", None),
        );
        let declared = || Resources::default().d1_databases(&[("D1_ORDERS", "shop-orders-v2")]);
        let resources = project(vec![("shop", declared())]);
        let orphans = D1::orphans(&OrphanCx {
            env: &lock.environments["default"],
            resources: &resources,
        });
        let mut out = String::new();
        for orphan in &orphans {
            D1::short_orphan(orphan, &mut out);
            assert!(!D1::prunable(orphan));
        }
        assert_eq!(
            out,
            "orphan d1 shop D1_OLD (shop-old; not deleted)\n\
             orphan d1 shop D1_ORDERS (shop-orders; not deleted)\n"
        );
        let mut ids = BTreeMap::new();
        kind_bound_ids(
            LedgerView::of(&lock, "default"),
            "shop",
            &declared(),
            &mut ids,
        );
        assert_eq!(
            ids,
            BTreeMap::from([("D1_ORDERS".to_string(), "id-2".to_string())])
        );
    }

    #[test]
    fn a_recorded_database_is_bound_by_its_name_and_id_without_an_account_call() {
        let mut lock = DeployLock::default();
        lock.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1", None),
        );
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        let db = D1Decl {
            binding: "D1_ORDERS".to_string(),
            name: "shop-orders".to_string(),
            migrations: Vec::new(),
            jurisdiction: None,
        };
        let step = d1_step(
            &PlanCx {
                ledger: LedgerView::of(&lock, "default"),
                worker: "shop",
            },
            &db,
        );
        assert_eq!(step.action(), "reuse");
        let done = D1::apply(
            &StepCx {
                environment: "default",
                worker: "shop",
                project: &project,
                lock: &lock,
                live: (),
                tools: Tools {
                    api: &crate::cloudflare::Account::new(Wrangler::new(
                        &provenance,
                        Path::new("/nonexistent"),
                    )),
                    project_root: Path::new("/nonexistent"),
                },
            },
            &step,
        )
        .expect("nothing to create");
        assert_eq!(
            done.bound,
            [(
                "D1_ORDERS".to_string(),
                ("shop-orders".to_string(), "id-1".to_string())
            )]
        );
        assert!(done.commits.is_empty(), "nothing to record");
    }

    #[test]
    fn a_database_is_created_in_its_jurisdiction_and_the_plan_says_so() {
        // #3820: the `--jurisdiction` argument is what pins it; whether Cloudflare honours it
        // needs a live account, so this covers the argument, the record and the plan line.
        let api = crate::cloudflare::fake::Recording::new(|_| {
            Ok::<_, crate::cloudflare::WranglerError>("id-9".to_string())
        });
        assert_eq!(
            create_d1(&api, "shop-orders", Some("eu")).unwrap(),
            ("id-9".to_string(), false)
        );
        assert_eq!(
            create_d1(&api, "shop-audit", None).unwrap(),
            ("id-9".to_string(), false)
        );
        assert_eq!(
            api.calls(),
            [
                "d1 create shop-orders --jurisdiction eu",
                "d1 create shop-audit"
            ]
        );
        let record = D1Record::of("shop-orders", "id-9", Some("eu"));
        assert_eq!(record.jurisdiction.as_deref(), Some("eu"));
        let toml = toml::to_string(&record).unwrap();
        assert!(toml.contains("jurisdiction = \"eu\""), "{toml}");
        assert!(
            !toml::to_string(&D1Record::of("shop-audit", "id-9", None))
                .unwrap()
                .contains("jurisdiction")
        );
        let mut out = String::new();
        D1::short_line(
            &PlanD1 {
                binding: "D1_ORDERS",
                action: "create",
                database: "shop-orders".to_string(),
                jurisdiction: Some("eu"),
                migrations: &[],
            },
            &mut out,
        );
        assert_eq!(
            out,
            "d1 create shop-orders (D1_ORDERS) in jurisdiction eu\n"
        );
    }

    #[test]
    fn an_adopted_database_is_never_recorded_as_pinned() {
        // #3820: `d1 create` says it already exists, so nothing was created in the jurisdiction.
        let adopting = || {
            crate::cloudflare::fake::Recording::new(|op| match op {
                crate::cloudflare::Op::D1Create { .. } => {
                    Err(crate::cloudflare::CloudflareError::from(
                        crate::cloudflare::RestError::answered("POST", "/d1/database", 400, "[]"),
                    ))
                }
                _ => Ok("u-1".to_string()),
            })
        };
        assert_eq!(
            create_d1(&adopting(), "shop-orders", Some("eu")).unwrap(),
            ("u-1".to_string(), true)
        );
        let lock = DeployLock::default();
        let project = BTreeMap::new();
        let api = adopting();
        let cx = StepCx {
            environment: "default",
            worker: "shop",
            project: &project,
            lock: &lock,
            live: (),
            tools: Tools {
                api: &api,
                project_root: Path::new("/nonexistent"),
            },
        };
        let pinned = D1Decl {
            binding: "D1_ORDERS".to_string(),
            name: "shop-orders".to_string(),
            migrations: Vec::new(),
            jurisdiction: Some("eu".to_string()),
        };
        let unpinned = D1Decl {
            jurisdiction: None,
            ..pinned.clone()
        };
        let step = |db: &'static D1Decl| D1Step {
            db,
            database: "shop-orders".to_string(),
            recorded: None,
        };
        let (pinned, unpinned): (&'static D1Decl, &'static D1Decl) =
            (Box::leak(Box::new(pinned)), Box::leak(Box::new(unpinned)));
        let refused = match D1::apply(&cx, &step(pinned)) {
            Ok(_) => panic!("an adopted database must not be recorded as pinned"),
            Err(e) => e.message,
        };
        assert!(
            refused.contains("cannot read the jurisdiction"),
            "{refused}"
        );
        // Without a jurisdiction it is adopted as before, and recorded with none.
        let done = D1::apply(&cx, &step(unpinned)).expect("adopted");
        assert!(done.commits.len() == 1);
        // The plan says the same, and does not call it `reuse`.
        struct Holds;
        impl on_account::Listings for Holds {
            fn names(
                &self,
                _: KindId,
                _: Option<&str>,
            ) -> Result<BTreeSet<String>, crate::cloudflare::CloudflareError> {
                Ok(BTreeSet::from(["shop-orders".to_string()]))
            }
        }
        let on = on_account::OnAccount::read(&BTreeSet::from([(KindId::D1, None)]), &Holds);
        let mut line = PlanD1 {
            binding: "D1_ORDERS",
            action: "create",
            database: "shop-orders".to_string(),
            jurisdiction: Some("eu"),
            migrations: &[],
        };
        let mut notes = Vec::new();
        D1::checked_against(&mut line, "shop", &on, &mut notes);
        assert_eq!(line.action, "create");
        assert!(notes[0].contains("cannot be confirmed"), "{notes:?}");
    }

    #[test]
    fn materialising_into_a_missing_configuration_fails_with_its_worker() {
        let e = D1::materialise(
            Path::new("/nonexistent/wrangler.toml"),
            "shop",
            &BTreeMap::from([(
                "D1_ORDERS".to_string(),
                ("shop-orders".to_string(), "id-1".to_string()),
            )]),
        )
        .unwrap_err();
        assert!(
            e.message
                .contains("could not materialise the D1 database ids into `shop`'s"),
            "{}",
            e.message
        );
    }
}
