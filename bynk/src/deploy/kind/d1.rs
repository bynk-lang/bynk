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
        let database = d1_database_name(cx.ledger.environment, &db.name);
        let recorded = cx.ledger.recorded::<D1>(cx.worker, &db.binding);
        PlanD1 {
            binding: &db.binding,
            action: if recorded.is_some_and(|r| r.name == database) {
                "reuse"
            } else {
                "create"
            },
            database,
            migrations: &db.migrations,
        }
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
    fn provision<'r>(
        cx: &StepCx<'_, '_, D1>,
        db: Self::Decl<'r>,
    ) -> Result<Provisioned<'r, D1>, DeployFailure> {
        let (worker, environment) = (cx.worker, cx.environment);
        let physical = d1_database_name(environment, &db.name);
        let recorded = cx
            .ledger()
            .recorded::<D1>(worker, &db.binding)
            .filter(|recorded| recorded.name == physical)
            .map(|recorded| recorded.id.clone());
        if let Some(id) = recorded {
            return Ok(Provisioned::bound(&db.binding, (physical, id)));
        }
        let id = create_d1(cx.tools.provenance, &physical, cx.tools.project_root).map_err(|e| {
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
        Ok(Provisioned::bound(&db.binding, (physical, id)).with(record))
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
    provenance: &Provenance,
    worker_dir: &Path,
    binding: &str,
    environment: &str,
) -> Result<(), String> {
    let Some(mut command) = workers::wrangler_command(provenance, "d1") else {
        return Err("wrangler not found".into());
    };
    command
        .args(["migrations", "apply", binding, "--remote"])
        .current_dir(worker_dir)
        .stdin(std::process::Stdio::null());
    if environment != "default" {
        command.args(["--env", environment]);
    }
    let output = command.output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(wrangler_said(&output))
    }
}

/// Create the D1 database `name` and return its `database_id`, or adopt the
/// one already there under that name (P4a, #2602). Run from the project root,
/// as KV and buckets are.
fn create_d1(provenance: &Provenance, name: &str, project_root: &Path) -> Result<String, String> {
    let Some(mut command) = workers::wrangler_command(provenance, "d1") else {
        return Err("wrangler not found".into());
    };
    let output = command
        .arg("create")
        .arg(name)
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        return parse_d1_id(&stdout).ok_or_else(|| {
            format!("`wrangler d1 create` succeeded but printed no `database_id`:\n{stdout}")
        });
    }
    let said = wrangler_said(&output);
    if !d1_database_already_exists(&said) {
        return Err(said);
    }
    let Some(mut list) = workers::wrangler_command(provenance, "d1") else {
        return Err("wrangler not found".into());
    };
    let listed = list
        .args(["list", "--json"])
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if !listed.status.success() {
        return Err(format!(
            "it already exists, and listing the account's databases failed: {}",
            wrangler_said(&listed)
        ));
    }
    parse_d1_listed_id(&String::from_utf8_lossy(&listed.stdout), name)
        .ok_or_else(|| "it already exists, but `wrangler d1 list` does not show it".to_string())
}

/// The `database_id` in `wrangler d1 create`'s output: the config snippet it
/// prints, as TOML (`database_id = "…"`) or JSON (`"database_id": "…"`).
fn parse_d1_id(output: &str) -> Option<String> {
    let at = output.find("database_id")?;
    let rest = &output[at + "database_id".len()..];
    let rest = rest.trim_start_matches('"').trim_start();
    let rest = rest.strip_prefix(['=', ':'])?.trim_start();
    let id = rest.strip_prefix('"')?.split('"').next()?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'))
        .then(|| id.to_string())
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

/// Does this `wrangler d1 create` failure just mean a database of that name
/// is already on the account? Wrangler says "A database with that name
/// already exists".
fn d1_database_already_exists(said: &str) -> bool {
    said.to_ascii_lowercase()
        .contains("database with that name already exists")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_d1_id_is_read_from_either_snippet_format() {
        let toml = "✅ Successfully created DB 'shop-orders' in region WEUR\nCreated your new D1 database.\n\n[[d1_databases]]\nbinding = \"shop_orders\"\ndatabase_name = \"shop-orders\"\ndatabase_id = \"9b3c2a1e-0000-4000-8000-0123456789ab\"\n";
        assert_eq!(
            parse_d1_id(toml).as_deref(),
            Some("9b3c2a1e-0000-4000-8000-0123456789ab")
        );
        let json = "{\n  \"d1_databases\": [\n    {\n      \"binding\": \"DB\",\n      \"database_name\": \"shop-orders\",\n      \"database_id\": \"9b3c2a1e-0000-4000-8000-0123456789ab\"\n    }\n  ]\n}";
        assert_eq!(
            parse_d1_id(json).as_deref(),
            Some("9b3c2a1e-0000-4000-8000-0123456789ab")
        );
        assert_eq!(parse_d1_id("Created your new D1 database."), None);
        assert!(d1_database_already_exists(
            "✘ [ERROR] A database with that name already exists"
        ));
        assert!(!d1_database_already_exists(
            "✘ [ERROR] Authentication error"
        ));
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
        let done = D1::provision(
            &StepCx {
                environment: "default",
                worker: "shop",
                project: &project,
                lock: &lock,
                live: (),
                tools: Tools {
                    provenance: &provenance,
                    project_root: Path::new("/nonexistent"),
                },
            },
            &db,
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
