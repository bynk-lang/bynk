//! #3073: `bynk revert` — make the previous deployment live again, after
//! checking what makes that unsafe.
//!
//! The mechanism is Cloudflare's own version rollback, driven by Bynk. What
//! Bynk adds is the judgement: it reads the facts each version carries about
//! itself ([`Facts`], set at deploy), compares the live and target versions,
//! and refuses, naming each reason, when going back would fault agents on
//! newer records, run on a schema the target was not built for, break a
//! contract a live peer holds, or bind a resource the ledger no longer
//! records. Every unknown is a refusal. Nothing is rebuilt and no `.bynk/`
//! build tree is needed, so it works from a fresh CI checkout.
//!
//! Everything that touches the account sits behind [`Account`], so the checks,
//! the history walk and the execution are exercised against a fake. The real
//! implementation ([`LiveAccount`]) rests on Cloudflare behaviour the
//! proposal lists as gated probes (response shapes, what a rollback restores).

use super::facts::{FACTS_VAR, Facts};
use super::*;
use crate::migrate::live::{Credentials, Rest};
use crate::migrate::sweep::{Cloudflare as DoApi, SETTLING_MARGIN};

/// One entry of a Worker's deployment list, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Deployment {
    pub(crate) id: String,
    /// RFC 3339.
    pub(crate) created_on: String,
    /// `annotations["workers/triggered_by"]`: `upload`, `secret`, `rollback`…
    pub(crate) trigger: Option<String>,
    /// `(version id, percentage)`.
    pub(crate) versions: Vec<(String, u32)>,
}

impl Deployment {
    fn is_secret(&self) -> bool {
        self.trigger.as_deref() == Some("secret")
    }
    /// A deployment that re-activates an existing version. Cloudflare records
    /// `wrangler rollback` (and so `bynk revert`) as trigger `deployment`,
    /// not `rollback` (#3112, observed on a real account); `rollback` is kept
    /// in case another path reports it.
    fn is_rollback(&self) -> bool {
        matches!(self.trigger.as_deref(), Some("rollback" | "deployment"))
    }
    /// The one version this deployment serves at 100%, if it is not split.
    fn sole_version(&self) -> Option<&str> {
        match self.versions.as_slice() {
            [(id, 100)] => Some(id),
            _ => None,
        }
    }
}

/// What the revert needs from the account.
pub(crate) trait Account {
    /// The Worker's deployments, newest first, as many as Cloudflare lists.
    fn deployments(&self, script: &str) -> Result<Vec<Deployment>, String>;
    /// The text of a version's [`FACTS_VAR`] variable, `None` when it has none.
    fn version_facts(&self, script: &str, version: &str) -> Result<Option<String>, String>;
    /// The migration files a D1 database has applied, in order.
    fn applied_migrations(&self, database_id: &str) -> Result<Vec<String>, String>;
    /// Whether any instance of agent `class` on `script` holds stored data.
    fn has_stored_data(&self, script: &str, class: &str) -> Result<bool, String>;
    /// Make `version` the Worker's live deployment.
    fn rollback(&self, script: &str, version: &str) -> Result<(), String>;
}

/// Where the live and target versions sit in a Worker's history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Located {
    /// The newest deployment of any trigger: the version whose facts describe
    /// what is live (a secret change keeps the code and the variables).
    pub(crate) live_version: String,
    /// That deployment's id: re-read before the rollback, so a deploy *or* a
    /// secret change made after the plan stops the revert.
    pub(crate) live_deployment: String,
    pub(crate) live_code_created: String,
    /// The version the revert goes back to.
    pub(crate) target_version: String,
    /// Secret-triggered deployments newer than the target's own.
    pub(crate) secret_changes: usize,
}

const GRADUAL: &str = "a gradual deployment is in progress (traffic is split between versions); finish or cancel it first";

/// Find the live and target versions in `deployments` (newest first).
///
/// The target is the deployment immediately before the *upload of the live
/// code version*: what was live, secrets included, just before the release
/// being left. Going back to an older *upload* instead would restore the
/// secret values from before any rotation in between. Other deployments are
/// read as follows. A rollback deployment re-activates a version and adds no
/// code, so the live code version is found by its own upload, and a second
/// revert goes back past that upload rather than to the release just left.
/// A secret change mints a version with the same code, so it never counts as
/// the live code, but it can be a target (it is what was live).
pub(crate) fn locate(deployments: &[Deployment], to: Option<&str>) -> Result<Located, String> {
    let newest = deployments
        .first()
        .ok_or("Cloudflare lists no deployment for it")?;
    let live_version = newest.sole_version().ok_or(GRADUAL)?.to_string();
    let live_code = deployments
        .iter()
        .find(|d| !d.is_secret())
        .ok_or("every listed deployment is a secret change; the code's history is out of view")?;
    let live_code_version = live_code.sole_version().ok_or(GRADUAL)?.to_string();
    let target_index = match to {
        Some(id) => {
            if id == live_code_version || id == live_version {
                return Err(format!("version `{id}` is the one already live"));
            }
            deployments
                .iter()
                .position(|d| d.sole_version() == Some(id))
                .ok_or_else(|| {
                    format!("version `{id}` is not among the Worker's listed deployments")
                })?
        }
        None => {
            let upload = deployments
                .iter()
                .position(|d| {
                    !d.is_secret()
                        && !d.is_rollback()
                        && d.sole_version() == Some(live_code_version.as_str())
                })
                .ok_or("the live version's own upload is out of the listed history, so what preceded it cannot be told")?;
            if upload + 1 >= deployments.len() {
                return Err("there is no earlier deployment in the Worker's listed history".into());
            }
            upload + 1
        }
    };
    let target = deployments[target_index].sole_version().ok_or(GRADUAL)?;
    if target == live_code_version || target == live_version {
        return Err(format!("version `{target}` is the one already live"));
    }
    let secret_changes = deployments[..target_index]
        .iter()
        .filter(|d| d.is_secret())
        .count();
    Ok(Located {
        live_version,
        live_deployment: newest.id.clone(),
        live_code_created: live_code.created_on.clone(),
        target_version: target.to_string(),
        secret_changes,
    })
}

/// One check's outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Finding {
    pub(crate) check: &'static str,
    pub(crate) subject: String,
    /// `ok` or `refused`.
    pub(crate) verdict: &'static str,
    pub(crate) detail: String,
}

impl Finding {
    fn ok(check: &'static str, subject: impl Into<String>, detail: impl Into<String>) -> Self {
        Finding {
            check,
            subject: subject.into(),
            verdict: "ok",
            detail: detail.into(),
        }
    }
    fn refused(check: &'static str, subject: impl Into<String>, why: impl Into<String>) -> Self {
        Finding {
            check,
            subject: subject.into(),
            verdict: "refused",
            detail: why.into(),
        }
    }
    pub(crate) fn is_refusal(&self) -> bool {
        self.verdict == "refused"
    }
}

/// What the account said about stored agent data for one class.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Stored {
    None,
    Present,
    /// The listing lags a write by minutes (ADR 0566): inside the margin of
    /// the live deploy, or unreadable, an empty listing proves nothing.
    Unknown(String),
}

/// What [`evaluate`] weighs for one Worker.
pub(crate) struct Case<'a> {
    pub(crate) worker: &'a str,
    pub(crate) environment: &'a str,
    pub(crate) live: &'a Facts,
    pub(crate) target: &'a Facts,
    /// Applied migrations by D1 binding.
    pub(crate) applied: &'a BTreeMap<String, Result<Vec<String>, String>>,
    /// Stored-data state by agent class, for the classes that need it.
    pub(crate) stored: &'a BTreeMap<String, Stored>,
    /// Each class's floor in the checkout's `bynk.schema.lock`.
    pub(crate) floors: &'a BTreeMap<String, i64>,
    /// The ledger as it will stand once every selected Worker is reverted.
    pub(crate) after: &'a DeployLock,
    /// The ledger as it stands now, for the recorded ids.
    pub(crate) lock: &'a DeployLock,
    pub(crate) secret_changes: usize,
    pub(crate) accept_schema_ahead: &'a BTreeSet<String>,
    pub(crate) accept_secret_changes: bool,
}

/// The D1 bindings whose applied list a case needs.
pub(crate) fn databases_to_read(live: &Facts, target: &Facts) -> BTreeSet<String> {
    live.migrations
        .keys()
        .chain(target.migrations.keys())
        .cloned()
        .collect()
}

/// The agent classes whose stored data a case needs to know about: those the
/// live version writes at a higher schema version than the target, and, when the
/// live version writes a later storage layout than the target (#3189), every class
/// the two share.
pub(crate) fn classes_to_probe(live: &Facts, target: &Facts) -> BTreeSet<String> {
    let layout_ahead = live.layout() > target.layout();
    live.agents
        .iter()
        .filter(|(class, lv)| {
            target
                .agents
                .get(*class)
                .is_some_and(|tv| tv < lv || layout_ahead)
        })
        .map(|(class, _)| class.clone())
        .collect()
}

fn worker_of_dependency(dependency: &str) -> String {
    dependency.replace('.', "-")
}

/// The binding's id as the ledger records it now.
fn recorded_id(
    lock: &DeployLock,
    worker: &str,
    binding: &str,
    environment: &str,
) -> Option<String> {
    recorded_kv_binding(lock, worker, binding, environment)
        .map(str::to_string)
        .or_else(|| recorded_d1(lock, worker, binding, environment).map(|d| d.id.clone()))
        .or_else(|| recorded_ratelimit(lock, worker, binding, environment).map(str::to_string))
}

/// Every check for one Worker. A refusal is any finding with verdict
/// `refused`; nothing here is advisory.
pub(crate) fn evaluate(c: &Case<'_>) -> Vec<Finding> {
    let mut out = Vec::new();

    // D1 migrations are forward-only (ADR 0664).
    for binding in databases_to_read(c.live, c.target) {
        let target_files = c.target.migrations.get(&binding);
        if target_files.is_none() && !c.target.ids.contains_key(&binding) {
            out.push(Finding::ok("d1", &binding, "not used by the target"));
            continue;
        }
        let applied = match c.applied.get(&binding) {
            Some(Ok(a)) => a,
            Some(Err(e)) => {
                out.push(Finding::refused(
                    "d1",
                    &binding,
                    format!("could not read the applied migrations: {e}"),
                ));
                continue;
            }
            None => {
                out.push(Finding::refused(
                    "d1",
                    &binding,
                    "the applied migrations were not read",
                ));
                continue;
            }
        };
        let known: BTreeSet<&String> = target_files.into_iter().flatten().collect();
        let ahead: Vec<&String> = applied.iter().filter(|f| !known.contains(f)).collect();
        if ahead.is_empty() {
            out.push(Finding::ok(
                "d1",
                &binding,
                "no migration newer than the target",
            ));
        } else {
            let files = ahead
                .iter()
                .map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            if c.accept_schema_ahead.contains(&binding) {
                out.push(Finding::ok(
                    "d1",
                    &binding,
                    format!("applied after the target was built, accepted as additive: {files}"),
                ));
            } else {
                out.push(Finding::refused(
                    "d1",
                    &binding,
                    format!(
                        "the live version applied {files} after the target was built; migrations are forward-only, so the target may not run on this schema (if each is additive, `--accept-schema-ahead {binding}`)"
                    ),
                ));
            }
        }
    }

    // Agent state is stamped with its schema version (ADR 0545) and faults on
    // newer records.
    if c.live.classes != c.target.classes {
        let added: Vec<&String> = c
            .live
            .classes
            .iter()
            .filter(|x| !c.target.classes.contains(x))
            .collect();
        let removed: Vec<&String> = c
            .target
            .classes
            .iter()
            .filter(|x| !c.live.classes.contains(x))
            .collect();
        out.push(Finding::refused(
            "classes",
            c.worker,
            format!(
                "the Durable Object classes differ (added since the target: [{}]; removed since: [{}]); Cloudflare cannot roll back across a class migration",
                added.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
                removed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            ),
        ));
    }
    let classes: BTreeSet<&String> = c.live.agents.keys().chain(c.target.agents.keys()).collect();
    for class in classes {
        match (c.live.agents.get(class), c.target.agents.get(class)) {
            (Some(_), None) | (None, Some(_)) => out.push(Finding::refused(
                "agent",
                class.as_str(),
                "the class is declared by only one of the two versions",
            )),
            (Some(lv), Some(tv)) => {
                let floor = c.floors.get(class).copied().unwrap_or(1);
                if *tv < floor {
                    out.push(Finding::refused(
                        "agent",
                        class.as_str(),
                        format!("the target writes schema v{tv}, below the class's floor v{floor} (a sweep raised it; instances there have no migration path)"),
                    ));
                } else if tv < lv {
                    match c.stored.get(class) {
                        Some(Stored::None) => out.push(Finding::ok(
                            "agent",
                            class.as_str(),
                            format!("target writes v{tv}, live v{lv}, and no instance holds stored data"),
                        )),
                        Some(Stored::Present) => out.push(Finding::refused(
                            "agent",
                            class.as_str(),
                            format!("the target writes schema v{tv} but the live version wrote v{lv}, and stored instances exist; the target would fault on them (NewerSchema)"),
                        )),
                        Some(Stored::Unknown(why)) => out.push(Finding::refused(
                            "agent",
                            class.as_str(),
                            format!("the target writes schema v{tv}, live v{lv}, and whether instances hold data is not known: {why}"),
                        )),
                        None => out.push(Finding::refused(
                            "agent",
                            class.as_str(),
                            "stored data was not read",
                        )),
                    }
                } else {
                    out.push(Finding::ok(
                        "agent",
                        class.as_str(),
                        format!("target v{tv}, live v{lv}"),
                    ));
                }
            }
            (None, None) => {}
        }
    }

    // Storage layout (#3189): a build that writes an older layout than the live one
    // would read records in a layout it does not know. The build that has the
    // forward-compatibility guard faults `NewerLayout`; one without it zero-merges
    // the missing collections and writes wholesale, orphaning every per-entry key.
    // Either way an instance with stored data is not safe to roll back over, and
    // nothing here can tell the two builds apart, so any rollback to an older layout
    // with stored data is refused. It is a property of the whole build, so every
    // class the two share is looked at.
    if c.live.layout() > c.target.layout() {
        let (ll, tl) = (c.live.layout(), c.target.layout());
        let shared: Vec<&String> = c
            .live
            .agents
            .keys()
            .filter(|class| c.target.agents.contains_key(*class))
            .collect();
        let mut found = false;
        for class in &shared {
            found = true;
            match c.stored.get(*class) {
                Some(Stored::None) => {}
                Some(Stored::Present) => out.push(Finding::refused(
                    "layout",
                    class.as_str(),
                    format!("the target writes storage layout {tl} but the live version wrote layout {ll}, and stored instances exist; the target cannot read them (NewerLayout), and a build without that check would orphan their entries"),
                )),
                Some(Stored::Unknown(why)) => out.push(Finding::refused(
                    "layout",
                    class.as_str(),
                    format!("the target writes storage layout {tl}, live {ll}, and whether instances hold data is not known: {why}"),
                )),
                None => out.push(Finding::refused(
                    "layout",
                    class.as_str(),
                    "stored data was not read",
                )),
            }
        }
        if !found
            || shared
                .iter()
                .all(|class| matches!(c.stored.get(*class), Some(Stored::None)))
        {
            out.push(Finding::ok(
                "layout",
                c.worker,
                format!("target writes layout {tl}, live {ll}, and no instance holds stored data"),
            ));
        }
    }

    // Contracts (#643, #2077): the target's records against the ledger as it
    // will stand with every selected Worker reverted.
    let mut skew = Vec::new();
    for s in contract_skews(
        &c.target.expects,
        c.after,
        worker_of_dependency,
        c.environment,
    ) {
        skew.push(format!(
            "calls {}.{} at {} but it provides {}",
            s.dependency, s.service, s.expected, s.live
        ));
    }
    for s in callee_contract_skews(
        c.worker,
        &c.target.provides,
        c.after,
        worker_of_dependency,
        c.environment,
    ) {
        skew.push(format!(
            "live caller `{}` stamps {}.{} at {} but the target provides {}",
            s.caller, s.dependency, s.service, s.expected, s.provided
        ));
    }
    if skew.is_empty() {
        out.push(Finding::ok(
            "contracts",
            c.worker,
            "no skew against the other live Workers",
        ));
    } else {
        out.push(Finding::refused("contracts", c.worker, skew.join("; ")));
    }

    // Resource ids: the target must bind what the ledger records now.
    let mut id_problems = Vec::new();
    for (binding, id) in &c.target.ids {
        match recorded_id(c.lock, c.worker, binding, c.environment) {
            Some(now) if now == *id => {}
            Some(now) => id_problems.push(format!("{binding} was {id}, the ledger records {now}")),
            None => id_problems.push(format!("{binding} ({id}) is no longer recorded")),
        }
    }
    if id_problems.is_empty() {
        out.push(Finding::ok(
            "ids",
            c.worker,
            "every bound resource is the one the ledger records",
        ));
    } else {
        out.push(Finding::refused(
            "ids",
            c.worker,
            format!(
                "the target would bind a resource that has since been replaced: {}",
                id_problems.join("; ")
            ),
        ));
    }

    // Secrets changed between the target and now.
    if c.secret_changes == 0 {
        out.push(Finding::ok(
            "secrets",
            c.worker,
            "no secret change since the target",
        ));
    } else if c.accept_secret_changes {
        out.push(Finding::ok(
            "secrets",
            c.worker,
            format!(
                "{} secret change(s) since the target, accepted",
                c.secret_changes
            ),
        ));
    } else {
        out.push(Finding::refused(
            "secrets",
            c.worker,
            format!(
                "{} secret change(s) since the target; a rollback may restore a secret (an edge key, say) its peers no longer accept (a `bynk migrate sweep` makes two; if only those, `--accept-secret-changes`)",
                c.secret_changes
            ),
        ));
    }
    out
}

/// The order to revert in: callers before the callees they call, so a
/// reverted callee never answers a caller still on the newer contract
/// (ADR 0648 covers the window for compatible changes). Cycles and
/// ties fall back to name order.
pub(crate) fn revert_order(workers: &BTreeMap<String, &Facts>) -> Vec<String> {
    let mut remaining: BTreeSet<String> = workers.keys().cloned().collect();
    let mut order = Vec::new();
    while !remaining.is_empty() {
        let next = remaining
            .iter()
            .find(|w| {
                // Callers first: `w` is next when nothing remaining calls it.
                !remaining.iter().any(|other| {
                    other != *w
                        && workers[other]
                            .expects
                            .keys()
                            .any(|dep| worker_of_dependency(dep) == **w)
                })
            })
            .or_else(|| remaining.iter().next())
            .cloned()
            .expect("remaining is not empty");
        remaining.remove(&next);
        order.push(next);
    }
    order
}

/// A copy of the ledger (it is not `Clone`: it round-trips through TOML, as it
/// does on disk).
fn clone_lock(lock: &DeployLock) -> DeployLock {
    toml::to_string(lock)
        .ok()
        .and_then(|text| toml::from_str::<DeployLock>(&text).ok())
        .unwrap_or_default()
}

/// A copy of `lock` with `target`'s contract records standing for `worker`.
fn with_target_contracts(
    lock: &DeployLock,
    environment: &str,
    worker: &str,
    target: &Facts,
) -> DeployLock {
    let mut copy = clone_lock(lock);
    apply_target(&mut copy, environment, worker, target);
    copy
}

/// What a reverted Worker's ledger entry says: the contracts of the version
/// now live. Ids, queues and tombstones are untouched.
fn apply_target(lock: &mut DeployLock, environment: &str, worker: &str, target: &Facts) {
    lock.record_deployed(environment, worker, Some(target.provides.clone()));
    lock.record_expects(environment, worker, target.expects.clone());
    lock.record_foreign(environment, worker, target.foreign.clone());
}

/// `bynk revert`'s options.
#[derive(Debug, Clone, Default)]
pub struct RevertOptions {
    pub environment: String,
    /// One context (dotted name or Worker directory); the whole project when
    /// `None`.
    pub context: Option<String>,
    /// A Cloudflare version id to go back to instead of the previous upload.
    pub to: Option<String>,
    pub dry_run: bool,
    pub json: bool,
    pub yes: bool,
    /// D1 bindings whose newer migrations the operator asserts are additive.
    pub accept_schema_ahead: Vec<String>,
    /// Accept secret changes between the target and now.
    pub accept_secret_changes: bool,
}

/// One Worker's planned revert.
#[derive(Debug, Serialize)]
pub(crate) struct WorkerPlan {
    pub(crate) worker: String,
    pub(crate) script: String,
    pub(crate) live_version: String,
    pub(crate) target_version: String,
    pub(crate) checks: Vec<Finding>,
    pub(crate) ok: bool,
    /// Whether the revert changes this Worker's entry in `bynk.deploy.lock`.
    pub(crate) lock_changes: bool,
}

/// The plan `bynk revert --dry-run --format json` prints.
#[derive(Debug, Serialize)]
pub(crate) struct RevertPlan {
    pub(crate) version: u32,
    pub(crate) environment: String,
    /// In the order they would be reverted.
    pub(crate) workers: Vec<WorkerPlan>,
    pub(crate) ok: bool,
    /// What to know before confirming: today, that a whole-project revert
    /// steps every Worker back, whatever the last release touched.
    pub(crate) notes: Vec<String>,
}

impl RevertPlan {
    pub(crate) fn render_short(&self) -> String {
        let mut out = String::new();
        for w in &self.workers {
            out.push_str(&format!(
                "revert {} (env {}): {} -> {}\n",
                w.worker, self.environment, w.live_version, w.target_version
            ));
            for f in &w.checks {
                let mark = if f.is_refusal() { "refused" } else { "ok" };
                out.push_str(&format!(
                    "  {:<9} {:<14} {}: {}\n",
                    f.check, f.subject, mark, f.detail
                ));
            }
            if w.ok {
                out.push_str(if w.lock_changes {
                    "  bynk.deploy.lock: this Worker's contract records change; commit it afterwards\n"
                } else {
                    "  bynk.deploy.lock: unchanged\n"
                });
            }
        }
        for note in &self.notes {
            out.push_str(&format!("note: {note}\n"));
        }
        out.push_str(if self.ok {
            "Every check passed.\n"
        } else {
            "Refused: nothing was changed.\n"
        });
        out
    }
}

/// A plan with what executing it needs: each Worker's position in its
/// history and the target's facts.
pub(crate) struct Planned {
    pub(crate) plan: RevertPlan,
    pub(crate) located: BTreeMap<String, Located>,
    pub(crate) targets: BTreeMap<String, Facts>,
}

/// Build the plan: locate, read facts, gather what the checks need, evaluate.
pub(crate) fn plan(
    api: &dyn Account,
    lock: &DeployLock,
    floors: &BTreeMap<String, i64>,
    workers: &[String],
    opts: &RevertOptions,
    now: std::time::SystemTime,
) -> Result<Planned, String> {
    let environment = opts.environment.as_str();
    let mut located = BTreeMap::new();
    let mut live_facts = BTreeMap::new();
    let mut target_facts = BTreeMap::new();
    let mut early: BTreeMap<String, Vec<Finding>> = BTreeMap::new();
    for worker in workers {
        let script = deployed_worker_name(environment, worker);
        // Anything that cannot be read or placed for one Worker is that
        // Worker's refusal, not the run's error.
        let outcome = (|| -> Result<(Located, Option<Facts>, Option<Facts>), String> {
            let deployments = api
                .deployments(&script)
                .map_err(|e| format!("could not read the deployments: {e}"))?;
            let loc = locate(&deployments, opts.to.as_deref())?;
            let read = |version: &str| -> Result<Option<Facts>, String> {
                Ok(api
                    .version_facts(&script, version)
                    .map_err(|e| format!("could not read version `{version}`: {e}"))?
                    .and_then(|t| Facts::decode(&t)))
            };
            let live = read(&loc.live_version)?;
            let target = read(&loc.target_version)?;
            Ok((loc, live, target))
        })();
        let (loc, live, target) = match outcome {
            Ok(found) => found,
            Err(why) => {
                early.insert(
                    worker.clone(),
                    vec![Finding::refused("history", worker.as_str(), why)],
                );
                continue;
            }
        };
        match (live, target) {
            (Some(l), Some(t)) => {
                live_facts.insert(worker.clone(), l);
                target_facts.insert(worker.clone(), t);
            }
            (l, t) => {
                let mut findings = Vec::new();
                if l.is_none() {
                    findings.push(Finding::refused(
                        "facts",
                        &loc.live_version,
                        "the live version records no deploy facts (it predates `bynk revert`, or its record did not fit); redeploy the previous tag instead",
                    ));
                }
                if t.is_none() {
                    findings.push(Finding::refused(
                        "facts",
                        &loc.target_version,
                        "the target version records no deploy facts (it predates `bynk revert`, or its record did not fit); redeploy its tag instead",
                    ));
                }
                early.insert(worker.clone(), findings);
            }
        }
        located.insert(worker.clone(), loc);
    }

    // The ledger as it will stand once every selected Worker is reverted.
    let mut after = clone_lock(lock);
    for (worker, target) in &target_facts {
        apply_target(&mut after, environment, worker, target);
    }

    let accepted: BTreeSet<String> = opts.accept_schema_ahead.iter().cloned().collect();
    let mut plans: BTreeMap<String, WorkerPlan> = BTreeMap::new();
    for worker in workers {
        let loc = located.get(worker);
        let script = deployed_worker_name(environment, worker);
        let (checks, lock_changes) = match (loc, live_facts.get(worker), target_facts.get(worker)) {
            (Some(loc), Some(live), Some(target)) => {
                let mut applied = BTreeMap::new();
                for binding in databases_to_read(live, target) {
                    let result = match live.ids.get(&binding).or_else(|| target.ids.get(&binding)) {
                        Some(id) => api.applied_migrations(id),
                        None => Err("no database id is recorded for it".to_string()),
                    };
                    applied.insert(binding, result);
                }
                // An unreadable time cannot show the live code is old enough
                // for an empty listing to count, so it reads as just deployed.
                let inside_margin = time::OffsetDateTime::parse(
                    &loc.live_code_created,
                    &time::format_description::well_known::Rfc3339,
                )
                .ok()
                .map(std::time::SystemTime::from)
                .and_then(|settled| now.duration_since(settled).ok())
                .map(|age| age < SETTLING_MARGIN)
                .unwrap_or(true);
                let mut stored = BTreeMap::new();
                for class in classes_to_probe(live, target) {
                    let agent = class.rsplit_once('.').map_or(class.as_str(), |(_, a)| a);
                    let state = match api.has_stored_data(&script, agent) {
                        Ok(true) => Stored::Present,
                        Ok(false) if inside_margin => Stored::Unknown(
                            "the live code was deployed less than 30 minutes ago, and Cloudflare's objects listing lags a write by minutes".into(),
                        ),
                        Ok(false) => Stored::None,
                        Err(e) => Stored::Unknown(e),
                    };
                    stored.insert(class, state);
                }
                let checks = evaluate(&Case {
                    worker,
                    environment,
                    live,
                    target,
                    applied: &applied,
                    stored: &stored,
                    floors,
                    after: &after,
                    lock,
                    secret_changes: loc.secret_changes,
                    accept_schema_ahead: &accepted,
                    accept_secret_changes: opts.accept_secret_changes,
                });
                let scratch = with_target_contracts(lock, environment, worker, target);
                let changed = toml::to_string(&scratch).ok() != toml::to_string(lock).ok();
                (checks, changed)
            }
            _ => (early.remove(worker).unwrap_or_default(), false),
        };
        let ok = !checks.is_empty() && checks.iter().all(|f| !f.is_refusal());
        plans.insert(
            worker.clone(),
            WorkerPlan {
                worker: worker.clone(),
                script,
                live_version: loc.map(|l| l.live_version.clone()).unwrap_or_default(),
                target_version: loc.map(|l| l.target_version.clone()).unwrap_or_default(),
                checks,
                ok,
                lock_changes,
            },
        );
    }
    let order = revert_order(
        &target_facts
            .iter()
            .map(|(w, f)| (w.clone(), f))
            .collect::<BTreeMap<_, _>>(),
    );
    let mut ordered: Vec<WorkerPlan> = Vec::new();
    for w in order {
        if let Some(p) = plans.remove(&w) {
            ordered.push(p);
        }
    }
    // Workers with no facts have no place in the call graph: list them last.
    ordered.extend(plans.into_values());
    let ok = ordered.iter().all(|w| w.ok);
    Ok(Planned {
        plan: RevertPlan {
            version: 1,
            environment: environment.to_string(),
            notes: if workers.len() > 1 && opts.context.is_none() {
                vec!["Every Worker goes back to its own previous deployment, whatever the last release touched. If the last release changed one context, revert just that one with `--context`.".to_string()]
            } else {
                Vec::new()
            },
            workers: ordered,
            ok,
        },
        located,
        targets: target_facts,
    })
}

/// Why an execution stopped.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Stopped {
    /// A Worker's live deployment changed after the plan was made.
    Moved(String),
    /// The rollback failed.
    Failed(String),
    /// The ledger could not be written after a rollback landed.
    Unrecorded(String),
}

/// Roll each planned Worker back, in plan order, recording each one in the
/// ledger as it lands. Re-reads every Worker's live deployment first and
/// refuses if it is no longer the one the plan was made against.
pub(crate) fn execute(
    api: &dyn Account,
    planned: &Planned,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    done: &mut Vec<String>,
) -> Result<(), Stopped> {
    let (plan, located, targets) = (&planned.plan, &planned.located, &planned.targets);
    for w in &plan.workers {
        let loc = &located[&w.worker];
        let current = api.deployments(&w.script).map_err(|e| {
            Stopped::Failed(format!(
                "could not re-read `{}`'s deployments: {e}",
                w.script
            ))
        })?;
        // The newest deployment of any trigger, not only the newest code
        // deployment: a secret change made since the plan (a concurrent sweep,
        // say) would otherwise be undone by the rollback without a word.
        if current.first().map(|d| d.id.as_str()) != Some(loc.live_deployment.as_str()) {
            return Err(Stopped::Moved(w.script.clone()));
        }
        api.rollback(&w.script, &w.target_version)
            .map_err(|e| Stopped::Failed(format!("rolling `{}` back failed: {e}", w.script)))?;
        done.push(w.worker.clone());
        apply_target(lock, environment, &w.worker, &targets[&w.worker]);
        write_lock(lock_path, lock)
            .map_err(|e| Stopped::Unrecorded(format!("`{}`: {e}", w.script)))?;
    }
    Ok(())
}

/// Cloudflare as the live revert sees it.
pub(crate) struct LiveAccount<'a> {
    pub(crate) rest: Rest,
    pub(crate) provenance: &'a probe::Provenance,
    pub(crate) root: &'a Path,
}

impl Account for LiveAccount<'_> {
    fn deployments(&self, script: &str) -> Result<Vec<Deployment>, String> {
        let mut out = Vec::new();
        for page in 1..=20u64 {
            let body = self.rest.get(&format!(
                "/workers/scripts/{script}/deployments?per_page=50&page={page}"
            ))?;
            let items = body["result"]["deployments"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            let n = items.len();
            for d in items {
                out.push(Deployment {
                    id: d["id"].as_str().unwrap_or_default().to_string(),
                    created_on: d["created_on"].as_str().unwrap_or_default().to_string(),
                    trigger: d["annotations"]["workers/triggered_by"]
                        .as_str()
                        .map(str::to_string),
                    versions: d["versions"]
                        .as_array()
                        .map(|vs| {
                            vs.iter()
                                .filter_map(|v| {
                                    Some((
                                        v["version_id"].as_str()?.to_string(),
                                        v["percentage"].as_f64()?.round() as u32,
                                    ))
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                });
            }
            let pages = body["result_info"]["total_pages"].as_u64().unwrap_or(1);
            if n == 0 || page >= pages {
                break;
            }
        }
        Ok(out)
    }

    fn version_facts(&self, script: &str, version: &str) -> Result<Option<String>, String> {
        let body = self
            .rest
            .get(&format!("/workers/scripts/{script}/versions/{version}"))?;
        let bindings = body["result"]["resources"]["bindings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok(bindings
            .iter()
            .find(|b| b["name"].as_str() == Some(FACTS_VAR))
            .and_then(|b| b["text"].as_str())
            .map(str::to_string))
    }

    fn applied_migrations(&self, database_id: &str) -> Result<Vec<String>, String> {
        let body = self
            .rest
            .post(
                &format!("/d1/database/{database_id}/query"),
                &serde_json::json!({ "sql": "SELECT name FROM d1_migrations ORDER BY id" }),
            )
            .map_err(|e| {
                if e.contains("no such table") {
                    "the database has no `d1_migrations` table: no migration has been applied to it yet, or it records them under another name; Bynk reads the default table, so this is refused rather than assumed empty".to_string()
                } else {
                    e
                }
            })?;
        Ok(body["result"][0]["results"]
            .as_array()
            .map(|rows| {
                rows.iter()
                    .filter_map(|r| r["name"].as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default())
    }

    fn has_stored_data(&self, script: &str, class: &str) -> Result<bool, String> {
        let namespaces = self.rest.namespaces()?;
        match namespaces
            .iter()
            .find(|n| n.script == script && n.class == class)
        {
            // No namespace: the class has never stored anything there.
            None => Ok(false),
            Some(ns) => Ok(!self.rest.stored_objects(&ns.id)?.is_empty()),
        }
    }

    fn rollback(&self, script: &str, version: &str) -> Result<(), String> {
        let Some(mut command) = workers::wrangler_command(self.provenance, "rollback") else {
            return Err("wrangler not found".to_string());
        };
        let output = command
            .args([
                version,
                "--name",
                script,
                "--message",
                "bynk revert",
                "--yes",
            ])
            .current_dir(self.root)
            .output()
            .map_err(|e| e.to_string())?;
        if output.status.success() {
            Ok(())
        } else {
            Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
        }
    }
}

/// `bynk revert`.
pub fn revert(root: &Path, opts: &RevertOptions) -> ExitCode {
    match revert_run(root, opts) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("bynk: {message}");
            ExitCode::FAILURE
        }
    }
}

fn revert_run(root: &Path, opts: &RevertOptions) -> Result<ExitCode, String> {
    if opts.to.is_some() && opts.context.is_none() {
        return Err("`--to` names one Worker's version, so it needs `--context`".to_string());
    }
    let lock_path = root.join(LOCK_FILE);
    let mut lock = read_lock(&lock_path)?;
    let mut workers = lock.deployed_workers(&opts.environment);
    if let Some(context) = &opts.context {
        let dir = context.replace('.', "-");
        if !workers.contains(&dir) {
            return Err(format!(
                "`{context}` is not recorded as deployed in environment `{}`",
                opts.environment
            ));
        }
        workers = vec![dir];
    }
    if workers.is_empty() {
        return Err(format!(
            "bynk.deploy.lock records no deployed Worker in environment `{}`",
            opts.environment
        ));
    }
    let floors = schema_floors(root);
    let credentials = Credentials::from_env().map_err(|e| {
        format!(
            "{}; a revert also reads D1 migrations, so the token needs D1 read access",
            e.replace(" to sweep", " to revert")
        )
    })?;
    let rest = Rest {
        agent: crate::migrate::live::agent()?,
        credentials,
        base: crate::migrate::live::API.to_string(),
    };
    let probe = probe::detect(
        &probe::SystemToolbox,
        "wrangler",
        probe::DetectOpts {
            project_root: Some(root),
            allow_npx: true,
            manager: Some(bynk_driver::npm_install::system_tool_manager(root)),
        },
    );
    let account = LiveAccount {
        rest,
        provenance: &probe.provenance,
        root,
    };
    let planned = plan(
        &account,
        &lock,
        &floors,
        &workers,
        opts,
        std::time::SystemTime::now(),
    )?;
    if opts.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&planned.plan).map_err(|e| e.to_string())?
        );
    } else {
        print!("{}", planned.plan.render_short());
    }
    if !planned.plan.ok {
        return Ok(ExitCode::FAILURE);
    }
    if opts.dry_run {
        return Ok(ExitCode::SUCCESS);
    }
    if !confirm_revert(opts.yes) {
        return Ok(ExitCode::FAILURE);
    }
    let before = toml::to_string(&lock).ok();
    let mut done = Vec::new();
    match execute(
        &account,
        &planned,
        &mut lock,
        &lock_path,
        &opts.environment,
        &mut done,
    ) {
        Ok(()) => {
            eprintln!(
                "bynk: reverted {} Worker(s): {}",
                done.len(),
                done.join(", ")
            );
            if toml::to_string(&lock).ok() != before {
                eprintln!("bynk: bynk.deploy.lock changed; commit it");
            }
            Ok(ExitCode::SUCCESS)
        }
        Err(stop) => {
            if !done.is_empty() {
                eprintln!("bynk: already reverted: {}", done.join(", "));
            }
            Err(match stop {
                Stopped::Moved(script) => format!(
                    "`{script}` was deployed after the plan was made; nothing further was changed. Plan again."
                ),
                Stopped::Failed(m) => m,
                Stopped::Unrecorded(m) => format!(
                    "the rollback landed but bynk.deploy.lock could not be written ({m}); the contracts it records for the reverted Workers are now stale"
                ),
            })
        }
    }
}

/// Each agent class's floor in the checkout's `bynk.schema.lock`; empty
/// without one.
fn schema_floors(root: &Path) -> BTreeMap<String, i64> {
    let Ok(existing) = bynk_driver::schema_lock::read(root) else {
        return BTreeMap::new();
    };
    let Ok(registry) = bynk_project::schema_registry::parse(existing.as_deref(), root) else {
        return BTreeMap::new();
    };
    registry
        .agents()
        .map(|(class, entry)| (class.clone(), entry.floor))
        .collect()
}

fn confirm_revert(yes: bool) -> bool {
    if yes {
        return true;
    }
    if !io::stdin().is_terminal() {
        eprintln!("bynk: refusing to change Cloudflare in a non-interactive session without --yes");
        return false;
    }
    eprint!("Revert? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn dep(id: &str, trigger: &str, version: &str) -> Deployment {
        Deployment {
            id: id.into(),
            created_on: "2026-01-01T00:00:00Z".into(),
            trigger: Some(trigger.into()),
            versions: vec![(version.into(), 100)],
        }
    }

    #[test]
    fn the_target_is_what_was_live_just_before_the_live_upload() {
        let h = [
            dep("d3", "upload", "v3"),
            dep("d2", "upload", "v2"),
            dep("d1", "upload", "v1"),
        ];
        let l = locate(&h, None).unwrap();
        assert_eq!(
            (l.live_version.as_str(), l.target_version.as_str()),
            ("v3", "v2")
        );
        assert_eq!(l.secret_changes, 0);
    }

    #[test]
    fn a_secret_rotation_between_the_uploads_is_kept_not_rolled_back() {
        // Newest first: upload v3, secret v2s, upload v2. What was live before the
        // bad v3 is v2s (v2's code with the rotated secret), not v2.
        let h = [
            dep("d3", "upload", "v3"),
            dep("d2s", "secret", "v2s"),
            dep("d2", "upload", "v2"),
        ];
        let l = locate(&h, None).unwrap();
        assert_eq!(l.target_version, "v2s");
        assert_eq!(l.secret_changes, 0, "no secret changed after the target");
    }

    #[test]
    fn a_second_revert_goes_back_past_the_live_upload_not_to_the_bad_release() {
        // After reverting v3 -> v2 the list reads rollback->v2, upload v3, v2, v1.
        let h = [
            dep("d4", "rollback", "v2"),
            dep("d3", "upload", "v3"),
            dep("d2", "upload", "v2"),
            dep("d1", "upload", "v1"),
        ];
        let l = locate(&h, None).unwrap();
        assert_eq!(l.live_version, "v2");
        assert_eq!(l.target_version, "v1", "never back to the bad v3");
    }

    #[test]
    fn a_rollback_as_cloudflare_reports_it_is_not_taken_for_an_upload() {
        // #3112: a real `wrangler rollback` lists as `workers/triggered_by:
        // "deployment"`. Read as an upload, the second revert went to the bad
        // release (v2) instead of finding the rollback's own upload (v1).
        let h = [
            dep("d3", "deployment", "v1"),
            dep("d2", "upload", "v2"),
            dep("d1", "upload", "v1"),
        ];
        let l = locate(&h, None);
        assert!(
            l.unwrap_err().contains("no earlier deployment"),
            "v1 is the first upload; nothing precedes it"
        );
    }

    #[test]
    fn an_earlier_rollback_before_the_live_upload_is_what_was_live() {
        // upload v1, upload v2 (bad), rollback->v1, upload v3: before v3 was v1.
        let h = [
            dep("d4", "upload", "v3"),
            dep("d3", "rollback", "v1"),
            dep("d2", "upload", "v2"),
            dep("d1", "upload", "v1"),
        ];
        assert_eq!(locate(&h, None).unwrap().target_version, "v1");
    }

    #[test]
    fn secret_changes_after_the_live_upload_are_counted_and_not_the_live_code() {
        let h = [
            dep("d4", "secret", "v3s2"),
            dep("d3", "secret", "v3s1"),
            dep("d2", "upload", "v3"),
            dep("d1", "upload", "v2"),
        ];
        let l = locate(&h, None).unwrap();
        assert_eq!(
            l.live_version, "v3s2",
            "the newest version's facts describe what is live"
        );
        assert_eq!(l.live_deployment, "d4");
        assert_eq!(l.target_version, "v2");
        assert_eq!(l.secret_changes, 2);
    }

    #[test]
    fn a_gradual_deployment_or_missing_history_is_refused() {
        let mut split = dep("d2", "upload", "v2");
        split.versions = vec![("v2".into(), 50), ("v1".into(), 50)];
        assert!(
            locate(&[split, dep("d1", "upload", "v1")], None)
                .unwrap_err()
                .contains("gradual")
        );
        assert!(
            locate(&[dep("d1", "upload", "v1")], None)
                .unwrap_err()
                .contains("no earlier deployment")
        );
        assert!(locate(&[], None).is_err());
    }

    #[test]
    fn to_names_a_listed_version_and_not_the_live_one() {
        let h = [
            dep("d3", "upload", "v3"),
            dep("d2", "upload", "v2"),
            dep("d1", "upload", "v1"),
        ];
        assert_eq!(locate(&h, Some("v1")).unwrap().target_version, "v1");
        assert!(locate(&h, Some("v3")).unwrap_err().contains("already live"));
        assert!(locate(&h, Some("nope")).unwrap_err().contains("not among"));
    }

    // ---- the checks ----

    fn facts(mut f: impl FnMut(&mut Facts)) -> Facts {
        let mut x = Facts {
            v: 1,
            ..Default::default()
        };
        f(&mut x);
        x
    }

    struct Fx {
        live: Facts,
        target: Facts,
        applied: BTreeMap<String, Result<Vec<String>, String>>,
        stored: BTreeMap<String, Stored>,
        floors: BTreeMap<String, i64>,
        lock: DeployLock,
        after: DeployLock,
        secrets: usize,
        schema_ok: BTreeSet<String>,
        secrets_ok: bool,
    }

    impl Fx {
        fn new(live: Facts, target: Facts) -> Self {
            Fx {
                live,
                target,
                applied: BTreeMap::new(),
                stored: BTreeMap::new(),
                floors: BTreeMap::new(),
                lock: DeployLock::default(),
                after: DeployLock::default(),
                secrets: 0,
                schema_ok: BTreeSet::new(),
                secrets_ok: false,
            }
        }
        fn run(&self) -> Vec<Finding> {
            evaluate(&Case {
                worker: "shop",
                environment: "default",
                live: &self.live,
                target: &self.target,
                applied: &self.applied,
                stored: &self.stored,
                floors: &self.floors,
                after: &self.after,
                lock: &self.lock,
                secret_changes: self.secrets,
                accept_schema_ahead: &self.schema_ok,
                accept_secret_changes: self.secrets_ok,
            })
        }
    }

    fn refusals(findings: &[Finding]) -> Vec<&Finding> {
        findings.iter().filter(|f| f.is_refusal()).collect()
    }

    #[test]
    fn identical_versions_revert_cleanly() {
        let f = Fx::new(facts(|_| {}), facts(|_| {}));
        assert!(refusals(&f.run()).is_empty(), "{:?}", f.run());
    }

    fn with_migrations(files: &[&str]) -> Facts {
        facts(|f| {
            f.migrations.insert(
                "D1_ORDERS".into(),
                files.iter().map(|s| s.to_string()).collect(),
            );
            f.ids.insert("D1_ORDERS".into(), "db-1".into());
        })
    }

    #[test]
    fn an_applied_migration_the_target_does_not_know_is_refused_and_overridable_per_database() {
        let mut f = Fx::new(
            with_migrations(&["0001_a.sql", "0002_b.sql"]),
            with_migrations(&["0001_a.sql"]),
        );
        f.applied.insert(
            "D1_ORDERS".into(),
            Ok(vec!["0001_a.sql".into(), "0002_b.sql".into()]),
        );
        f.lock
            .record_d1("default", "shop", "D1_ORDERS", "shop-orders", "db-1");
        let r = f.run();
        let d1 = refusals(&r)
            .into_iter()
            .find(|x| x.check == "d1")
            .expect("a d1 refusal");
        assert!(
            d1.detail.contains("0002_b.sql")
                && d1.detail.contains("--accept-schema-ahead D1_ORDERS"),
            "{}",
            d1.detail
        );
        f.schema_ok.insert("D1_ORDERS".into());
        assert!(refusals(&f.run()).is_empty(), "{:?}", f.run());
        // An unread list is a refusal, never a pass.
        f.schema_ok.clear();
        f.applied.insert("D1_ORDERS".into(), Err("403".into()));
        assert!(
            f.run()
                .iter()
                .any(|x| x.check == "d1" && x.is_refusal() && x.detail.contains("403"))
        );
    }

    fn with_agent(v: i64) -> Facts {
        facts(|f| {
            f.agents.insert("shop.Cart".into(), v);
            f.classes = vec!["Cart".into()];
        })
    }

    #[test]
    fn an_older_target_schema_is_refused_with_stored_data_or_unknown_and_passes_without() {
        let mut f = Fx::new(with_agent(3), with_agent(2));
        f.stored.insert("shop.Cart".into(), Stored::Present);
        assert!(
            f.run()
                .iter()
                .any(|x| x.check == "agent" && x.is_refusal() && x.detail.contains("NewerSchema"))
        );
        f.stored
            .insert("shop.Cart".into(), Stored::Unknown("lag".into()));
        assert!(
            f.run()
                .iter()
                .any(|x| x.check == "agent" && x.is_refusal() && x.detail.contains("not known"))
        );
        f.stored.insert("shop.Cart".into(), Stored::None);
        assert!(refusals(&f.run()).is_empty());
        // The same version, or a newer target, needs no look at the data.
        assert!(refusals(&Fx::new(with_agent(2), with_agent(2)).run()).is_empty());
    }

    fn with_layout(v: i64, layout: i64) -> Facts {
        let mut f = with_agent(v);
        f.layout = Some(layout);
        f
    }

    #[test]
    fn a_target_on_an_older_layout_is_refused_with_stored_data_or_unknown_and_passes_without() {
        // Same schema version either side: only the layout differs.
        let mut f = Fx::new(with_layout(2, 2), with_agent(2));
        f.stored.insert("shop.Cart".into(), Stored::Present);
        let found = f.run();
        assert!(
            found
                .iter()
                .any(|x| x.check == "layout" && x.is_refusal() && x.detail.contains("NewerLayout")),
            "{found:?}"
        );
        f.stored
            .insert("shop.Cart".into(), Stored::Unknown("lag".into()));
        assert!(
            f.run()
                .iter()
                .any(|x| x.check == "layout" && x.is_refusal() && x.detail.contains("not known"))
        );
        f.stored.remove("shop.Cart");
        assert!(
            f.run()
                .iter()
                .any(|x| x.check == "layout" && x.is_refusal() && x.detail.contains("not read"))
        );
        f.stored.insert("shop.Cart".into(), Stored::None);
        assert!(refusals(&f.run()).is_empty());
    }

    #[test]
    fn the_layout_checks_are_silent_when_the_target_is_not_older() {
        // The same layout, or a newer target (the upgrade direction), needs no look at the data.
        for (live, target) in [
            (with_agent(2), with_agent(2)),
            (with_layout(2, 2), with_layout(2, 2)),
            (with_agent(2), with_layout(2, 2)),
        ] {
            let found = Fx::new(live, target).run();
            assert!(refusals(&found).is_empty(), "{found:?}");
            assert!(found.iter().all(|x| x.check != "layout"));
        }
    }

    #[test]
    fn a_layout_difference_makes_every_shared_class_probed_for_stored_data() {
        let live = with_layout(2, 2);
        let target = with_agent(2);
        assert_eq!(
            classes_to_probe(&live, &target),
            BTreeSet::from(["shop.Cart".to_string()])
        );
        // Without the layout difference the same schema version needs no probe.
        assert!(classes_to_probe(&with_agent(2), &with_agent(2)).is_empty());
    }

    #[test]
    fn a_target_below_the_floor_or_a_changed_class_set_is_refused() {
        let mut f = Fx::new(with_agent(4), with_agent(2));
        f.stored.insert("shop.Cart".into(), Stored::None);
        f.floors.insert("shop.Cart".into(), 3);
        assert!(
            f.run()
                .iter()
                .any(|x| x.check == "agent" && x.detail.contains("floor v3"))
        );
        let added = Fx::new(
            facts(|f| f.classes = vec!["Cart".into(), "Wishlist".into()]),
            facts(|f| f.classes = vec!["Cart".into()]),
        );
        assert!(
            added
                .run()
                .iter()
                .any(|x| x.check == "classes" && x.detail.contains("Wishlist"))
        );
    }

    #[test]
    fn contract_skew_against_another_live_worker_is_refused_using_the_post_revert_ledger() {
        // The target of `shop` expects `pay.charge` at `old`; `pay` is live at `new`.
        let target = facts(|f| {
            f.expects.insert(
                "pay".into(),
                BTreeMap::from([("charge".into(), "old".into())]),
            );
        });
        let mut after = DeployLock::default();
        after.record_deployed(
            "default",
            "pay",
            Some(BTreeMap::from([("charge".into(), "new".into())])),
        );
        let mut f = Fx::new(facts(|_| {}), target);
        f.after = after;
        let r = f.run();
        let c = refusals(&r)
            .into_iter()
            .find(|x| x.check == "contracts")
            .expect("a contract refusal");
        assert!(
            c.detail.contains("pay.charge") && c.detail.contains("old") && c.detail.contains("new"),
            "{}",
            c.detail
        );
        // When `pay` is reverted too, the post-revert ledger agrees and the skew is gone.
        let mut both = DeployLock::default();
        both.record_deployed(
            "default",
            "pay",
            Some(BTreeMap::from([("charge".into(), "old".into())])),
        );
        f.after = both;
        assert!(refusals(&f.run()).is_empty());
    }

    #[test]
    fn a_replaced_resource_id_is_refused() {
        let target = facts(|f| {
            f.ids.insert("KV".into(), "kv-old".into());
        });
        let mut f = Fx::new(facts(|_| {}), target);
        f.lock.record_kv("default", "shop", "KV", "kv-new");
        assert!(f.run().iter().any(|x| x.check == "ids"
            && x.is_refusal()
            && x.detail.contains("kv-old")
            && x.detail.contains("kv-new")));
        let mut g = Fx::new(
            facts(|_| {}),
            facts(|f| {
                f.ids.insert("KV".into(), "kv-1".into());
            }),
        );
        g.lock.record_kv("default", "shop", "KV", "kv-1");
        assert!(refusals(&g.run()).is_empty());
    }

    #[test]
    fn secret_changes_since_the_target_are_refused_unless_accepted() {
        let mut f = Fx::new(facts(|_| {}), facts(|_| {}));
        f.secrets = 2;
        assert!(
            f.run()
                .iter()
                .any(|x| x.check == "secrets" && x.is_refusal())
        );
        f.secrets_ok = true;
        assert!(refusals(&f.run()).is_empty());
    }

    #[test]
    fn callers_are_reverted_before_the_callees_they_call() {
        let shop = facts(|f| {
            f.expects.insert("pay".into(), BTreeMap::new());
        });
        let pay = facts(|_| {});
        let map = BTreeMap::from([("pay".to_string(), &pay), ("shop".to_string(), &shop)]);
        assert_eq!(revert_order(&map), ["shop", "pay"]);
    }

    // ---- plan and execution against a fake account ----

    #[derive(Default)]
    struct Fake {
        deployments: RefCell<BTreeMap<String, Vec<Deployment>>>,
        facts: BTreeMap<(String, String), String>,
        rolled: RefCell<Vec<(String, String)>>,
        /// Push a newer deployment when `deployments` is read this many times.
        move_after_reads: Option<usize>,
        reads: RefCell<usize>,
    }

    impl Account for Fake {
        fn deployments(&self, script: &str) -> Result<Vec<Deployment>, String> {
            *self.reads.borrow_mut() += 1;
            if self
                .move_after_reads
                .is_some_and(|n| *self.reads.borrow() > n)
            {
                self.deployments
                    .borrow_mut()
                    .get_mut(script)
                    .unwrap()
                    .insert(0, dep("dX", "upload", "vX"));
            }
            self.deployments
                .borrow()
                .get(script)
                .cloned()
                .ok_or_else(|| "no such Worker".to_string())
        }
        fn version_facts(&self, script: &str, version: &str) -> Result<Option<String>, String> {
            Ok(self
                .facts
                .get(&(script.to_string(), version.to_string()))
                .cloned())
        }
        fn applied_migrations(&self, _: &str) -> Result<Vec<String>, String> {
            Ok(vec![])
        }
        fn has_stored_data(&self, _: &str, _: &str) -> Result<bool, String> {
            Ok(false)
        }
        fn rollback(&self, script: &str, version: &str) -> Result<(), String> {
            self.rolled
                .borrow_mut()
                .push((script.into(), version.into()));
            Ok(())
        }
    }

    fn account(live: &Facts, target: Option<&Facts>) -> Fake {
        let mut fake = Fake::default();
        fake.deployments.borrow_mut().insert(
            "shop".into(),
            vec![dep("d2", "upload", "v2"), dep("d1", "upload", "v1")],
        );
        fake.facts
            .insert(("shop".into(), "v2".into()), live.encode().unwrap());
        if let Some(t) = target {
            fake.facts
                .insert(("shop".into(), "v1".into()), t.encode().unwrap());
        }
        fake
    }

    fn opts() -> RevertOptions {
        RevertOptions {
            environment: "default".into(),
            ..Default::default()
        }
    }

    fn long_ago() -> std::time::SystemTime {
        std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(4_000_000_000)
    }

    fn lock_with_shop() -> DeployLock {
        let mut lock: DeployLock = toml::from_str("version = 1\n").unwrap();
        lock.record_deployed("default", "shop", Some(BTreeMap::new()));
        lock
    }

    #[test]
    fn a_clean_revert_plans_ok_and_rolls_back_with_the_lock_recorded() {
        let live = facts(|f| {
            f.provides.insert("quote".into(), "new".into());
        });
        let target = facts(|f| {
            f.provides.insert("quote".into(), "old".into());
        });
        let api = account(&live, Some(&target));
        let mut lock = lock_with_shop();
        let planned = plan(
            &api,
            &lock,
            &BTreeMap::new(),
            &["shop".to_string()],
            &opts(),
            long_ago(),
        )
        .unwrap();
        let plan = &planned.plan;
        assert!(plan.ok, "{}", plan.render_short());
        assert!(plan.workers[0].lock_changes, "the contract record differs");
        let dir = std::env::temp_dir().join(format!("bynk-revert-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bynk.deploy.lock");
        let mut done = Vec::new();
        execute(&api, &planned, &mut lock, &path, "default", &mut done).unwrap();
        assert_eq!(
            *api.rolled.borrow(),
            [("shop".to_string(), "v1".to_string())]
        );
        assert_eq!(done, ["shop"]);
        let written = read_lock(&path).unwrap();
        assert_eq!(
            written.live_contracts("default", "shop").unwrap()["quote"],
            "old"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_version_without_facts_is_refused_and_nothing_is_rolled_back() {
        let live = facts(|_| {});
        let api = account(&live, None);
        let lock = lock_with_shop();
        let Planned { plan, .. } = plan(
            &api,
            &lock,
            &BTreeMap::new(),
            &["shop".to_string()],
            &opts(),
            long_ago(),
        )
        .unwrap();
        assert!(!plan.ok);
        assert!(
            plan.workers[0]
                .checks
                .iter()
                .any(|f| f.check == "facts" && f.detail.contains("redeploy"))
        );
        assert!(api.rolled.borrow().is_empty());
        // And a live version without facts is refused alike.
        let mut none = account(&live, Some(&live));
        none.facts.remove(&("shop".to_string(), "v2".to_string()));
        let Planned { plan, .. } = self::plan(
            &none,
            &lock,
            &BTreeMap::new(),
            &["shop".to_string()],
            &opts(),
            long_ago(),
        )
        .unwrap();
        assert!(!plan.ok);
    }

    #[test]
    fn live_code_deployed_inside_the_settling_margin_makes_stored_data_unknown() {
        let live = with_agent(3);
        let target = with_agent(2);
        let api = account(&live, Some(&target));
        let lock = lock_with_shop();
        let just_after =
            std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_767_225_600 + 60); // 2026-01-01T00:01:00Z
        let Planned { plan, .. } = plan(
            &api,
            &lock,
            &BTreeMap::new(),
            &["shop".to_string()],
            &opts(),
            just_after,
        )
        .unwrap();
        assert!(!plan.ok);
        assert!(
            plan.workers[0]
                .checks
                .iter()
                .any(|f| f.check == "agent" && f.detail.contains("not known"))
        );
        let Planned { plan, .. } = plan_at(&api, &lock, long_ago());
        assert!(plan.ok, "settled and empty: {}", plan.render_short());
    }

    fn plan_at(api: &Fake, lock: &DeployLock, now: std::time::SystemTime) -> Planned {
        plan(
            api,
            lock,
            &BTreeMap::new(),
            &["shop".to_string()],
            &opts(),
            now,
        )
        .unwrap()
    }

    #[test]
    fn a_deploy_that_lands_mid_revert_stops_it_before_the_rollback() {
        let live = facts(|_| {});
        let mut api = account(&live, Some(&live));
        let mut lock = lock_with_shop();
        let planned = plan_at(&api, &lock, long_ago());
        assert!(planned.plan.ok);
        api.move_after_reads = Some(*api.reads.borrow());
        let dir = std::env::temp_dir().join(format!("bynk-revert-moved-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut done = Vec::new();
        let stopped = execute(
            &api,
            &planned,
            &mut lock,
            &dir.join("bynk.deploy.lock"),
            "default",
            &mut done,
        )
        .unwrap_err();
        assert_eq!(stopped, Stopped::Moved("shop".into()));
        assert!(api.rolled.borrow().is_empty() && done.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_json_plan_is_pinned() {
        let live = facts(|_| {});
        let api = account(&live, Some(&live));
        let lock = lock_with_shop();
        let Planned { plan, .. } = plan_at(&api, &lock, long_ago());
        let json = serde_json::to_string_pretty(&plan).unwrap();
        assert_eq!(
            json,
            r#"{
  "version": 1,
  "environment": "default",
  "workers": [
    {
      "worker": "shop",
      "script": "shop",
      "live_version": "v2",
      "target_version": "v1",
      "checks": [
        {
          "check": "contracts",
          "subject": "shop",
          "verdict": "ok",
          "detail": "no skew against the other live Workers"
        },
        {
          "check": "ids",
          "subject": "shop",
          "verdict": "ok",
          "detail": "every bound resource is the one the ledger records"
        },
        {
          "check": "secrets",
          "subject": "shop",
          "verdict": "ok",
          "detail": "no secret change since the target"
        }
      ],
      "ok": true,
      "lock_changes": true
    }
  ],
  "ok": true,
  "notes": []
}"#
        );
    }

    #[test]
    fn a_secret_change_made_after_the_plan_stops_the_revert_too() {
        let live = facts(|_| {});
        let api = account(&live, Some(&live));
        let mut lock = lock_with_shop();
        let planned = plan_at(&api, &lock, long_ago());
        assert!(planned.plan.ok);
        // A concurrent `bynk migrate sweep` (or any secret put) lands.
        api.deployments
            .borrow_mut()
            .get_mut("shop")
            .unwrap()
            .insert(0, dep("dS", "secret", "v2s"));
        let dir = std::env::temp_dir().join(format!("bynk-revert-secret-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut done = Vec::new();
        let stopped = execute(
            &api,
            &planned,
            &mut lock,
            &dir.join("bynk.deploy.lock"),
            "default",
            &mut done,
        )
        .unwrap_err();
        assert_eq!(stopped, Stopped::Moved("shop".into()));
        assert!(api.rolled.borrow().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_worker_that_cannot_be_placed_is_a_refusal_not_an_error_for_the_run() {
        // `pay` was first added in the bad release: nothing precedes it.
        let live = facts(|_| {});
        let api = account(&live, Some(&live));
        api.deployments
            .borrow_mut()
            .insert("pay".into(), vec![dep("p1", "upload", "pv1")]);
        let mut lock = lock_with_shop();
        lock.record_deployed("default", "pay", Some(BTreeMap::new()));
        let planned = plan(
            &api,
            &lock,
            &BTreeMap::new(),
            &["pay".to_string(), "shop".to_string()],
            &opts(),
            long_ago(),
        )
        .unwrap();
        assert!(!planned.plan.ok);
        let pay = planned
            .plan
            .workers
            .iter()
            .find(|w| w.worker == "pay")
            .unwrap();
        assert!(
            pay.checks
                .iter()
                .any(|f| f.check == "history" && f.detail.contains("no earlier deployment"))
        );
        let shop = planned
            .plan
            .workers
            .iter()
            .find(|w| w.worker == "shop")
            .unwrap();
        assert!(shop.ok, "the other Worker is still judged on its own");
        assert!(planned.plan.notes[0].contains("--context"));
    }

    #[test]
    fn to_needs_a_context() {
        let dir = std::env::temp_dir().join(format!("bynk-revert-to-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = revert_run(
            &dir,
            &RevertOptions {
                environment: "default".into(),
                to: Some("v1".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("--context"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }
}
