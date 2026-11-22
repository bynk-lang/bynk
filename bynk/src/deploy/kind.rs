//! Back-half paydown 1D.1 (#3929): one place per resource kind.
//!
//! A kind of account resource that `bynk deploy` creates or links (a KV namespace, an R2
//! bucket, …) implements [`ResourceKind`] once, in its own file under `kind/`. Every stage of
//! the deploy that used to handle each kind separately (the plan's lines and the account check,
//! the orphan report, provisioning, pruning, the deploy facts and `bynk revert`) is a loop over
//! [`for_each_kind`], the one fixed list of the kinds converted so far.
//!
//! The trait is typed, with associated types, rather than an object (Decision A of #3929): the
//! plan's JSON is a user contract, so each kind's plan line and orphan stay the structs they were,
//! with their serde attributes, and a kind's records stay where they were on disk. A kind's ledger
//! records are reached through its [`LedgerTable`], a view over the `Environment` fields that hold
//! them, so `bynk.deploy.lock` is written byte for byte as before (the ruling on Decision C).
//!
//! A kind takes what it needs through the contexts here and reaches into nothing else
//! (Decision B, and `design/tracks/back-half-paydown.md` §12): not `DeployOptions`, not
//! `plan::run`'s locals, not the CLI's flags. It does not write the ledger: provisioning and
//! pruning say what they recorded or forgot, and the caller writes the ledger at once, so a
//! record still lands before the push (ADR 0180).
//!
//! Converted so far: KV (1D.1a). The others follow in 1D.1b–e, and until then keep their own
//! code beside the loops.

use super::*;
use bynk_driver::emsg;

mod kv;

pub(crate) use kv::*;

/// Each kind of resource the deploy names on the account. Field-less, matched exhaustively.
///
/// The declaration order is the order a set of listings is asked in (`on_account`), which is
/// not the plan's order: that is [`for_each_kind`]'s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum KindId {
    Kv,
    R2,
    D1,
    Queue,
    Vectorize,
}

impl KindId {
    /// The kind as an account note names it.
    pub(crate) fn account_noun(self) -> &'static str {
        match self {
            KindId::Kv => "KV namespaces",
            KindId::R2 => "R2 buckets",
            KindId::D1 => "D1 databases",
            KindId::Queue => "queues",
            KindId::Vectorize => "Vectorize indexes",
        }
    }
}

/// Where one kind's records live in an environment of the ledger, by `(worker, binding)`. A
/// view over the `Environment` fields that hold them, so the file keeps its shape.
pub(crate) trait LedgerTable {
    type Record;

    /// `worker`'s record for `binding`.
    fn get<'l>(env: &'l Environment, worker: &str, binding: &str) -> Option<&'l Self::Record>;

    /// Record `record` for `worker`'s `binding`, replacing any.
    fn insert(env: &mut Environment, worker: &str, binding: &str, record: Self::Record);

    /// Forget `worker`'s `binding`, leaving the file as it would have been had it never been
    /// recorded.
    fn remove(env: &mut Environment, worker: &str, binding: &str);

    /// Every record, as `(worker, binding, record)`, in the order the ledger lists them.
    fn entries(env: &Environment) -> Vec<(&str, &str, &Self::Record)>;
}

/// The ledger as one environment's deploy reads it: that environment's records, and the
/// others', which an account-wide name is checked against.
#[derive(Clone, Copy)]
pub(crate) struct LedgerView<'l> {
    pub(crate) environment: &'l str,
    environments: &'l BTreeMap<String, Environment>,
}

impl<'l> LedgerView<'l> {
    pub(crate) fn of(lock: &'l DeployLock, environment: &'l str) -> Self {
        LedgerView {
            environment,
            environments: &lock.environments,
        }
    }

    /// This environment's records, if the ledger has any.
    pub(crate) fn this(&self) -> Option<&'l Environment> {
        self.environments.get(self.environment)
    }

    /// Every other environment the ledger knows, in name order.
    pub(crate) fn others(&self) -> impl Iterator<Item = (&'l str, &'l Environment)> + use<'l> {
        let environment = self.environment;
        self.environments
            .iter()
            .filter(move |(name, _)| name.as_str() != environment)
            .map(|(name, env)| (name.as_str(), env))
    }

    /// `worker`'s record of kind `K` for `binding` in this environment.
    pub(crate) fn recorded<K: ResourceKind>(
        &self,
        worker: &str,
        binding: &str,
    ) -> Option<&'l K::Record> {
        self.this()
            .and_then(|env| K::Table::get(env, worker, binding))
    }
}

impl DeployLock {
    /// `worker`'s record of kind `K` for `binding` in `environment`.
    pub(crate) fn recorded<K: ResourceKind>(
        &self,
        environment: &str,
        worker: &str,
        binding: &str,
    ) -> Option<&K::Record> {
        self.environments
            .get(environment)
            .and_then(|env| K::Table::get(env, worker, binding))
    }

    /// Record `record` of kind `K` for `worker`'s `binding` in `environment`.
    #[cfg(test)]
    pub(crate) fn record<K: ResourceKind>(
        &mut self,
        environment: &str,
        worker: &str,
        binding: &str,
        record: K::Record,
    ) {
        let env = self
            .environments
            .entry(environment.to_string())
            .or_default();
        K::Table::insert(env, worker, binding, record);
    }

    /// Forget `worker`'s `binding` of kind `K` in `environment`.
    #[cfg(test)]
    pub(crate) fn forget<K: ResourceKind>(
        &mut self,
        environment: &str,
        worker: &str,
        binding: &str,
    ) {
        if let Some(env) = self.environments.get_mut(environment) {
            K::Table::remove(env, worker, binding);
        }
    }
}

/// What a kind's plan line is derived from: one Worker in one environment.
pub(crate) struct PlanCx<'l> {
    pub(crate) ledger: LedgerView<'l>,
    pub(crate) worker: &'l str,
}

/// What a kind's checks over the whole project read: every Worker's declarations, the upload
/// order, and the ledger.
pub(crate) struct ProjectCx<'l> {
    pub(crate) ledger: LedgerView<'l>,
    pub(crate) resources: &'l BTreeMap<String, Resources>,
    pub(crate) order: &'l [String],
}

/// What a kind's orphans are found from: this environment's records and the whole project's
/// declarations (the build's, regardless of `--context`).
pub(crate) struct OrphanCx<'l> {
    pub(crate) env: &'l Environment,
    pub(crate) resources: &'l BTreeMap<String, Resources>,
}

/// The tools a kind's account calls go through. In 1D.1 a holder of what today's wrangler and
/// REST calls take; 1D.3 swaps its contents for the `Wrangler` trait and the shared REST client.
#[derive(Clone, Copy)]
pub(crate) struct Tools<'a> {
    pub(crate) provenance: &'a Provenance,
    pub(crate) project_root: &'a Path,
}

/// What a kind's provisioning and pruning act in: one Worker of one environment, the ledger to
/// record in (and, for an account-wide name, read across environments), the account state the
/// run read once, and the tools.
pub(crate) struct ApplyCx<'a, K: ResourceKind> {
    pub(crate) environment: &'a str,
    pub(crate) worker: &'a str,
    /// The whole project, for a check across it that a create re-runs.
    pub(crate) project: &'a BTreeMap<String, Resources>,
    pub(crate) environments: &'a mut BTreeMap<String, Environment>,
    pub(crate) live: &'a K::Live,
    pub(crate) tools: Tools<'a>,
}

impl<K: ResourceKind> ApplyCx<'_, K> {
    /// The ledger as it stands, for reading.
    pub(crate) fn ledger(&self) -> LedgerView<'_> {
        LedgerView {
            environment: self.environment,
            environments: self.environments,
        }
    }

    /// Record `record` for this Worker's `binding`.
    pub(crate) fn record(&mut self, binding: &str, record: K::Record) {
        let env = self
            .environments
            .entry(self.environment.to_string())
            .or_default();
        K::Table::insert(env, self.worker, binding, record);
    }

    /// Forget this Worker's `binding`.
    pub(crate) fn forget(&mut self, binding: &str) {
        if let Some(env) = self.environments.get_mut(self.environment) {
            K::Table::remove(env, self.worker, binding);
        }
    }
}

/// What provisioning one declared instance did.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Provisioned {
    /// The `(binding, id)` the push binds, written into the generated configuration.
    pub(crate) bound: Option<(String, String)>,
    /// `Some(what)` when a record was made: the caller writes the ledger before anything else
    /// happens (ADR 0180), and a failure to says "created {what} but could not record it".
    pub(crate) recorded: Option<String>,
}

/// A line `--prune` prints before it asks: through the styled message printer, or plain.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Notice {
    Styled(String),
    Plain(String),
}

impl Notice {
    pub(crate) fn print(&self) {
        match self {
            Notice::Styled(text) => emsg!("{text}"),
            Notice::Plain(text) => eprintln!("{text}"),
        }
    }
}

/// One kind of account resource the deploy creates or links (#3929, Decision A).
///
/// Its associated types are the shapes the deploy already had for it, unchanged: a declared
/// instance borrowed from `Resources`, its ledger record, its plan line and its orphan. Its
/// methods are each stage of the deploy for this kind. The defaults are a kind with nothing
/// to say at that stage.
pub(crate) trait ResourceKind: Sized {
    const ID: KindId;

    /// One declared instance, borrowed from the Worker's `Resources`.
    type Decl<'r>: Copy;
    /// What the ledger records for one instance.
    type Record;
    /// Where those records are in an environment.
    type Table: LedgerTable<Record = Self::Record>;
    /// The plan's line for one instance, as `--format json` writes it.
    type Line<'r>: Serialize;
    /// One recorded instance the build no longer declares.
    type Orphan: Serialize;
    /// Account state the run reads once, before any Worker is provisioned.
    type Live: Default;

    /// The Worker's declared instances, in the order they are planned and provisioned.
    fn declared(r: &Resources) -> Vec<Self::Decl<'_>>;

    /// The binding `decl` is bound under: its ledger key within its Worker.
    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str;

    /// The kind's lines in a context's plan.
    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [Self::Line<'r>];
    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<Self::Line<'r>>;

    /// The plan line for `decl`.
    fn plan_line<'r>(cx: &PlanCx<'_>, decl: Self::Decl<'r>) -> Self::Line<'r>;

    /// `line` in the short format, with its newline.
    fn short_line(line: &Self::Line<'_>, out: &mut String);

    /// What the account is asked for before this line can say `create` truly.
    fn listing(_line: &Self::Line<'_>) -> Option<on_account::Listing> {
        None
    }

    /// `line` checked against what the account holds, which may change its action or add a
    /// note to `notes`. `shown_worker` is the context's Worker as the plan names it.
    fn checked_against(
        _line: &mut Self::Line<'_>,
        _shown_worker: &str,
        _on: &on_account::OnAccount,
        _notes: &mut Vec<String>,
    ) {
    }

    /// Why the declared instances cannot all be named on the account, or `None`.
    fn name_problem(_cx: &ProjectCx<'_>) -> Option<String> {
        None
    }

    /// Why a CI job must not deploy `cx.order`: an instance whose id would be minted and lost
    /// with an uncommitted ledger. `None` when there is none.
    fn unrecordable_in_ci(_cx: &ProjectCx<'_>) -> Option<String> {
        None
    }

    /// The kind's orphans in a plan's orphan report.
    fn orphans_in(orphans: &Orphans) -> &[Self::Orphan];
    fn orphans_in_mut(orphans: &mut Orphans) -> &mut Vec<Self::Orphan>;

    /// Every recorded instance `cx.resources` no longer declares.
    fn orphans(cx: &OrphanCx<'_>) -> Vec<Self::Orphan>;

    /// `orphan` as the plan shows it in `environment`.
    fn shown_orphan(orphan: &Self::Orphan, environment: &str) -> Self::Orphan;

    /// `orphan` in the short format, with its newline.
    fn short_orphan(orphan: &Self::Orphan, out: &mut String);

    /// The Worker whose ledger entry `orphan` is.
    fn orphan_worker(orphan: &Self::Orphan) -> &str;

    /// Whether `--prune` deletes `orphan`.
    fn prunable(_orphan: &Self::Orphan) -> bool {
        false
    }

    /// What `--prune` says before it asks about `orphan`.
    fn prune_notice(_orphan: &Self::Orphan, _environment: &str) -> Option<Notice> {
        None
    }

    /// Delete `orphan` from the account and forget it. `Some(what)` when the ledger changed, for
    /// the caller to write at once. Report-only by default.
    fn prune(
        _cx: &mut ApplyCx<'_, Self>,
        _orphan: &Self::Orphan,
    ) -> Result<Option<String>, DeployFailure> {
        Ok(None)
    }

    /// Read what the run checks the ledger against, once.
    fn live(_tools: Tools<'_>, _cx: &ProjectCx<'_>) -> Self::Live {
        Self::Live::default()
    }

    /// Create or reuse `decl` on the account, recording what it made.
    fn provision(
        cx: &mut ApplyCx<'_, Self>,
        decl: Self::Decl<'_>,
    ) -> Result<Provisioned, DeployFailure>;

    /// Write the ids provisioning bound, by binding, into the generated configuration.
    fn materialise(
        _config: &Path,
        _worker: &str,
        _bound: &BTreeMap<String, String>,
    ) -> Result<(), DeployFailure> {
        Ok(())
    }

    /// The id a pushed version binds for `record`: what its deploy facts keep, and what
    /// `bynk revert` compares the ledger with.
    fn bound_id(_record: &Self::Record) -> Option<&str> {
        None
    }
}

/// A pass over every kind: one stage of the deploy, generic in the kind.
pub(crate) trait KindVisitor {
    fn visit<K: ResourceKind>(&mut self);
}

/// The one list of kinds, in the plan's line order (the same in both formats).
pub(crate) fn for_each_kind(v: &mut impl KindVisitor) {
    v.visit::<Kv>();
}

/// Fill each kind's lines of `context`, for Worker `worker`'s declarations `declared`.
pub(crate) fn fill_kind_lines<'r>(
    context: &mut ContextPlan<'r>,
    declared: &'r Resources,
    cx: &PlanCx<'_>,
) {
    struct Fill<'p, 'c, 'r> {
        context: &'p mut ContextPlan<'r>,
        declared: &'r Resources,
        cx: &'p PlanCx<'c>,
    }
    impl KindVisitor for Fill<'_, '_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            let lines = K::declared(self.declared)
                .into_iter()
                .map(|decl| K::plan_line(self.cx, decl))
                .collect();
            *K::lines_mut(self.context) = lines;
        }
    }
    for_each_kind(&mut Fill {
        context,
        declared,
        cx,
    });
}

/// Each kind's lines of `context` in the short format.
pub(crate) fn short_kind_lines(context: &ContextPlan<'_>, out: &mut String) {
    struct Short<'p, 'r> {
        context: &'p ContextPlan<'r>,
        out: &'p mut String,
    }
    impl KindVisitor for Short<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            for line in K::lines(self.context) {
                K::short_line(line, self.out);
            }
        }
    }
    for_each_kind(&mut Short { context, out });
}

/// The listings each kind's lines of `context` ask of the account.
pub(crate) fn kind_listings(context: &ContextPlan<'_>, wanted: &mut BTreeSet<on_account::Listing>) {
    struct Listings<'p, 'r> {
        context: &'p ContextPlan<'r>,
        wanted: &'p mut BTreeSet<on_account::Listing>,
    }
    impl KindVisitor for Listings<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            self.wanted
                .extend(K::lines(self.context).iter().filter_map(K::listing));
        }
    }
    for_each_kind(&mut Listings { context, wanted });
}

/// Each kind's lines of `context` checked against `on`.
pub(crate) fn check_kind_lines(
    context: &mut ContextPlan<'_>,
    on: &on_account::OnAccount,
    notes: &mut Vec<String>,
) {
    struct Check<'p, 'r> {
        context: &'p mut ContextPlan<'r>,
        on: &'p on_account::OnAccount,
        notes: &'p mut Vec<String>,
    }
    impl KindVisitor for Check<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            let shown_worker = self.context.worker.clone();
            for line in K::lines_mut(self.context) {
                K::checked_against(line, &shown_worker, self.on, self.notes);
            }
        }
    }
    for_each_kind(&mut Check { context, on, notes });
}

/// The first kind's account-name problem, in list order.
pub(crate) fn kind_name_problem(cx: &ProjectCx<'_>) -> Option<String> {
    struct First<'p, 'l> {
        cx: &'p ProjectCx<'l>,
        found: Option<String>,
    }
    impl KindVisitor for First<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            if self.found.is_none() {
                self.found = K::name_problem(self.cx);
            }
        }
    }
    let mut first = First { cx, found: None };
    for_each_kind(&mut first);
    first.found
}

/// The first kind's refusal to deploy from CI, in list order.
pub(crate) fn kind_unrecordable_in_ci(cx: &ProjectCx<'_>) -> Option<String> {
    struct First<'p, 'l> {
        cx: &'p ProjectCx<'l>,
        found: Option<String>,
    }
    impl KindVisitor for First<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            if self.found.is_none() {
                self.found = K::unrecordable_in_ci(self.cx);
            }
        }
    }
    let mut first = First { cx, found: None };
    for_each_kind(&mut first);
    first.found
}

/// Fill each kind's orphans of `orphans`.
pub(crate) fn find_kind_orphans(cx: &OrphanCx<'_>, orphans: &mut Orphans) {
    struct Find<'p, 'l> {
        cx: &'p OrphanCx<'l>,
        orphans: &'p mut Orphans,
    }
    impl KindVisitor for Find<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            *K::orphans_in_mut(self.orphans) = K::orphans(self.cx);
        }
    }
    for_each_kind(&mut Find { cx, orphans });
}

/// Each kind's orphans of `from`, as the plan shows them in `environment`, into `to`.
pub(crate) fn show_kind_orphans(from: &Orphans, environment: &str, to: &mut Orphans) {
    struct Shown<'p> {
        from: &'p Orphans,
        environment: &'p str,
        to: &'p mut Orphans,
    }
    impl KindVisitor for Shown<'_> {
        fn visit<K: ResourceKind>(&mut self) {
            *K::orphans_in_mut(self.to) = K::orphans_in(self.from)
                .iter()
                .map(|orphan| K::shown_orphan(orphan, self.environment))
                .collect();
        }
    }
    for_each_kind(&mut Shown {
        from,
        environment,
        to,
    });
}

/// Each kind's orphans of `orphans` in the short format.
pub(crate) fn short_kind_orphans(orphans: &Orphans, out: &mut String) {
    struct Short<'p> {
        orphans: &'p Orphans,
        out: &'p mut String,
    }
    impl KindVisitor for Short<'_> {
        fn visit<K: ResourceKind>(&mut self) {
            for orphan in K::orphans_in(self.orphans) {
                K::short_orphan(orphan, self.out);
            }
        }
    }
    for_each_kind(&mut Short { orphans, out });
}

/// How many of each kind's orphans `--prune` deletes, printing what it says of each.
pub(crate) fn announce_prunable_kinds(orphans: &Orphans, environment: &str) -> usize {
    struct Announce<'p> {
        orphans: &'p Orphans,
        environment: &'p str,
        count: usize,
    }
    impl KindVisitor for Announce<'_> {
        fn visit<K: ResourceKind>(&mut self) {
            for orphan in K::orphans_in(self.orphans) {
                if let Some(notice) = K::prune_notice(orphan, self.environment) {
                    notice.print();
                }
                self.count += usize::from(K::prunable(orphan));
            }
        }
    }
    let mut announce = Announce {
        orphans,
        environment,
        count: 0,
    };
    for_each_kind(&mut announce);
    announce.count
}

/// Whether any kind has an orphan `--prune` deletes.
pub(crate) fn kinds_have_prunable(orphans: &Orphans) -> bool {
    struct Any<'p> {
        orphans: &'p Orphans,
        found: bool,
    }
    impl KindVisitor for Any<'_> {
        fn visit<K: ResourceKind>(&mut self) {
            self.found |= K::orphans_in(self.orphans).iter().any(K::prunable);
        }
    }
    let mut any = Any {
        orphans,
        found: false,
    };
    for_each_kind(&mut any);
    any.found
}

/// The ids `worker` binds, by binding, as the ledger records them: each kind's
/// [`ResourceKind::bound_id`] for its declared instances.
pub(crate) fn kind_bound_ids(
    ledger: LedgerView<'_>,
    worker: &str,
    declared: &Resources,
    ids: &mut BTreeMap<String, String>,
) {
    struct Ids<'p, 'l> {
        ledger: LedgerView<'l>,
        worker: &'p str,
        declared: &'p Resources,
        ids: &'p mut BTreeMap<String, String>,
    }
    impl KindVisitor for Ids<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            for decl in K::declared(self.declared) {
                let binding = K::binding(decl);
                if let Some(id) = self
                    .ledger
                    .recorded::<K>(self.worker, binding)
                    .and_then(K::bound_id)
                {
                    self.ids.insert(binding.to_string(), id.to_string());
                }
            }
        }
    }
    for_each_kind(&mut Ids {
        ledger,
        worker,
        declared,
        ids,
    });
}

/// The id the ledger records for `worker`'s `binding`, of whichever kind binds it by id.
pub(crate) fn recorded_bound_id(
    ledger: LedgerView<'_>,
    worker: &str,
    binding: &str,
) -> Option<String> {
    struct Find<'p, 'l> {
        ledger: LedgerView<'l>,
        worker: &'p str,
        binding: &'p str,
        found: Option<String>,
    }
    impl KindVisitor for Find<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            if self.found.is_none() {
                self.found = self
                    .ledger
                    .recorded::<K>(self.worker, self.binding)
                    .and_then(K::bound_id)
                    .map(str::to_string);
            }
        }
    }
    let mut find = Find {
        ledger,
        worker,
        binding,
        found: None,
    };
    for_each_kind(&mut find);
    find.found
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

/// Provision each of the declared instances of `K` at `site`, in order, writing the ledger the
/// moment one is recorded, before the next is made (ADR 0180: an interrupted run never makes a
/// second). The `(binding, id)`s the push binds.
pub(crate) fn provision_kind<K: ResourceKind>(
    site: Site<'_>,
    declared: &Resources,
    lock: &mut DeployLock,
    lock_path: &Path,
    live: &K::Live,
) -> Result<BTreeMap<String, String>, DeployFailure> {
    let mut bound = BTreeMap::new();
    for decl in K::declared(declared) {
        let done = K::provision(
            &mut ApplyCx {
                environment: site.environment,
                worker: site.worker,
                project: site.project,
                environments: &mut lock.environments,
                live,
                tools: site.tools,
            },
            decl,
        )?;
        if let Some(what) = done.recorded {
            write_lock(lock_path, lock).map_err(|e| {
                DeployFailure::driver(format!(
                    "created {what} but could not record it in {}: {e}",
                    lock_path.display()
                ))
            })?;
        }
        if let Some((binding, id)) = done.bound {
            bound.insert(binding, id);
        }
    }
    Ok(bound)
}

/// Delete each kind's orphans `--prune` deletes, writing the ledger as each is forgotten, so a
/// prune interrupted half-way is resumed rather than repeated.
pub(crate) fn prune_kind_orphans(
    tools: Tools<'_>,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    orphans: &Orphans,
) -> Result<(), DeployFailure> {
    struct Prune<'p> {
        tools: Tools<'p>,
        lock: &'p mut DeployLock,
        lock_path: &'p Path,
        environment: &'p str,
        orphans: &'p Orphans,
        result: Result<(), DeployFailure>,
    }
    impl Prune<'_> {
        fn prune<K: ResourceKind>(&mut self) -> Result<(), DeployFailure> {
            let project = BTreeMap::new();
            let live = K::Live::default();
            for orphan in K::orphans_in(self.orphans)
                .iter()
                .filter(|o| K::prunable(o))
            {
                let forgot = K::prune(
                    &mut ApplyCx {
                        environment: self.environment,
                        worker: K::orphan_worker(orphan),
                        project: &project,
                        environments: &mut self.lock.environments,
                        live: &live,
                        tools: self.tools,
                    },
                    orphan,
                )?;
                if let Some(what) = forgot {
                    write_lock(self.lock_path, self.lock).map_err(|e| {
                        DeployFailure::driver(format!(
                            "deleted {what} but could not record it in {}: {e}",
                            self.lock_path.display()
                        ))
                    })?;
                }
            }
            Ok(())
        }
    }
    impl KindVisitor for Prune<'_> {
        fn visit<K: ResourceKind>(&mut self) {
            if self.result.is_ok() {
                self.result = self.prune::<K>();
            }
        }
    }
    let mut prune = Prune {
        tools,
        lock,
        lock_path,
        environment,
        orphans,
        result: Ok(()),
    };
    for_each_kind(&mut prune);
    prune.result
}
