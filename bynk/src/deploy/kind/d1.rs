//! 1D.1b (#3929): D1, each `D1` instance's database (`capability Orders = D1`), as one
//! [`ResourceKind`].
//!
//! A database is created by its environment-qualified name, and the id Cloudflare mints is recorded
//! before the push (P4a, #2602; ADR 0194), in `Environment::d1_instances`. One of its name already
//! on the account is adopted. A changed name makes a new database, and the old one, with its rows,
//! is kept as a superseded record and reported as an orphan. Orphans are never deleted. Its pending
//! migrations are applied before the push (P4b, #2608), and `bynk revert` checks the bound id.

use super::*;

/// The D1 kind.
pub(crate) struct D1;

/// [`D1`]'s records: `Environment::d1_instances`, by Worker and binding.
pub(crate) struct D1Table;

impl D1Record {
    /// The record of database `name` with id `id`, with nothing superseded yet.
    pub(crate) fn of(name: &str, id: &str) -> Self {
        D1Record {
            name: name.to_string(),
            id: id.to_string(),
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
            "d1 {} {} ({})\n",
            d1.action, d1.database, d1.binding
        ));
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
        _notes: &mut Vec<String>,
    ) {
        d1.action = held_verb(d1.action, on.holds(Self::ID, None, &d1.database));
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
        let id = create_d1(cx.tools.api, &physical).map_err(|e| {
            DeployFailure::driver(format!(
                "could not create the D1 database `{physical}` for `{worker}`'s `{}`: {e}",
                db.binding
            ))
        })?;
        let record = cx.recording(
            &db.binding,
            D1Record::of(&physical, &id),
            &format!("created the D1 database `{physical}`"),
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
) -> Result<(), String> {
    api.d1_migrations_apply(worker_dir, binding, environment)
        .map_err(|e| e.to_string())
}

/// Create the D1 database `name` and return its `database_id`, or adopt the
/// one already there under that name (P4a, #2602). Run from the project root,
/// as KV and buckets are.
pub(crate) fn create_d1(api: &dyn CloudflareApi, name: &str) -> Result<String, String> {
    let e = match api.d1_create(name) {
        Ok(id) => return Ok(id),
        Err(e) => e,
    };
    if !e.already_exists() {
        return Err(e.to_string());
    }
    let listed = api.d1_list().map_err(|e| {
        if e.exited() {
            format!("it already exists, and listing the account's databases failed: {e}")
        } else {
            e.to_string()
        }
    })?;
    parse_d1_listed_id(&listed, name)
        .ok_or_else(|| "it already exists, but `wrangler d1 list` does not show it".to_string())
}

/// The `uuid` of database `name` in `wrangler d1 list --json`'s array.
fn parse_d1_listed_id(json: &str, name: &str) -> Option<String> {
    // From the first line starting with `[` that the rest parses from, so a
    // bracketed banner line (`[WARNING] …`) before the array is skipped.
    let listed: Vec<serde_json::Value> = std::iter::once(0)
        .chain(json.match_indices('\n').map(|(i, _)| i + 1))
        .map(|i| json[i..].trim_start())
        .filter(|rest| rest.starts_with('['))
        .find_map(|rest| serde_json::from_str(rest).ok())?;
    listed
        .iter()
        .find(|db| db.get("name").and_then(|n| n.as_str()) == Some(name))?
        .get("uuid")?
        .as_str()
        .map(str::to_string)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_d1_id_is_read_from_either_snippet_format() {
        let exists = |stderr: &str| {
            crate::cloudflare::fake::failed(
                &crate::cloudflare::Op::D1Create { name: "d" },
                "",
                stderr,
            )
            .already_exists()
        };
        assert!(exists("✘ [ERROR] A database with that name already exists"));
        assert!(!exists("✘ [ERROR] Authentication error"));
        assert_eq!(
            parse_d1_listed_id(
                "[{\"uuid\": \"u-1\", \"name\": \"a\"}, {\"uuid\": \"u-2\", \"name\": \"b\"}]",
                "b"
            )
            .as_deref(),
            Some("u-2")
        );
        assert_eq!(parse_d1_listed_id("[]", "b"), None);
        // A bracketed banner before the array is skipped.
        assert_eq!(
            parse_d1_listed_id(
                "[WARNING] a newer wrangler is available\n[\n  {\"uuid\": \"u-3\", \"name\": \"c\"}\n]\n",
                "c"
            )
            .as_deref(),
            Some("u-3")
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
            D1Record::of("shop-orders", "id-1"),
        );
        D1Table::insert(
            &mut env,
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1"),
        );
        assert!(env.d1_instances["shop"]["D1_ORDERS"].superseded.is_empty());
        D1Table::insert(
            &mut env,
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders-v2", "id-2"),
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
            D1Record::of("shop-orders", "id-1"),
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
            fn names(&self, _: KindId, _: Option<&str>) -> Result<BTreeSet<String>, String> {
                Ok(BTreeSet::from(["shop-new".to_string()]))
            }
        }
        let on = on_account::OnAccount::read(&BTreeSet::from([(KindId::D1, None)]), &Holds);
        let mut line = PlanD1 {
            binding: "D1_NEW",
            action: "create",
            database: "shop-new".to_string(),
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
            D1Record::of("shop-orders", "id-1"),
        );
        lock.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders-v2", "id-2"),
        );
        lock.record::<D1>(
            "default",
            "shop",
            "D1_OLD",
            D1Record::of("shop-old", "id-3"),
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
            D1Record::of("shop-orders", "id-1"),
        );
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        let db = D1Decl {
            binding: "D1_ORDERS".to_string(),
            name: "shop-orders".to_string(),
            migrations: Vec::new(),
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
                    api: &Wrangler::new(&provenance, Path::new("/nonexistent")),
                    provenance: &provenance,
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
