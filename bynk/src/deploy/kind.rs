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
//! Converted so far: KV (1D.1a), R2 and D1 (1D.1b), Vectorize and Hyperdrive (1D.1c), rate limiters
//! and queues (1D.1d). The linked kinds and container images follow in 1D.1e, and until then keep
//! their own code after the loops, which is where they sit in the plan's order. The orphan report's
//! Worker block, which is not a kind, has a [`Slot`] in the list.
//!
//! `--prune` follows the list's order too, as the maintainer ruled for 1D.1c (22 November 2026):
//! its notices and its deletes used to come in orders of their own.

use super::*;
use bynk_driver::emsg;

mod ai_search;
mod container_image;
mod d1;
mod hyperdrive;
mod kv;
mod mtls_certificate;
mod queue;
mod r2;
mod ratelimit;
mod stored_secret;
mod vectorize;

pub(crate) use ai_search::*;
pub(crate) use container_image::*;
pub(crate) use d1::*;
pub(crate) use hyperdrive::*;
pub(crate) use kv::*;
pub(crate) use mtls_certificate::*;
pub(crate) use queue::*;
pub(crate) use r2::*;
pub(crate) use ratelimit::*;
pub(crate) use stored_secret::*;
pub(crate) use vectorize::*;

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
    Hyperdrive,
    RateLimit,
    StoredSecret,
    AiSearch,
    MtlsCertificate,
    ContainerImage,
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
            KindId::Hyperdrive => "Hyperdrive configs",
            KindId::RateLimit => "rate limiters",
            KindId::StoredSecret => "Secrets Store secrets",
            KindId::AiSearch => "AI Search instances",
            KindId::MtlsCertificate => "mTLS certificates",
            KindId::ContainerImage => "container images",
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

    /// Every environment the ledger knows, this one included, in name order.
    pub(crate) fn all(&self) -> impl Iterator<Item = (&'l str, &'l Environment)> + use<'l> {
        self.environments
            .iter()
            .map(|(name, env)| (name.as_str(), env))
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
///
/// A kind changes the ledger only through [`ApplyCx::record`] and [`ApplyCx::change`], which write
/// it at once, before the kind's next account call: so a record still lands before the push, and
/// before anything that could fail after it (ADR 0180). This is the one write site 1D.2 takes over.
pub(crate) struct ApplyCx<'a, 'l, K: ResourceKind> {
    pub(crate) environment: &'a str,
    pub(crate) worker: &'a str,
    /// The whole project, for a check across it that a create re-runs.
    pub(crate) project: &'a BTreeMap<String, Resources>,
    pub(crate) lock: &'a mut DeployLock,
    pub(crate) lock_path: &'a Path,
    pub(crate) live: K::Live<'l>,
    pub(crate) tools: Tools<'a>,
}

impl<K: ResourceKind> ApplyCx<'_, '_, K> {
    /// The ledger as it stands, for reading.
    pub(crate) fn ledger(&self) -> LedgerView<'_> {
        LedgerView::of(self.lock, self.environment)
    }

    /// Record `record` for this Worker's `binding`, and write the ledger. `done` says what was
    /// done to the account, for the message when the write fails: "{done} but could not record
    /// it in …".
    pub(crate) fn record(
        &mut self,
        binding: &str,
        record: K::Record,
        done: &str,
    ) -> Result<(), DeployFailure> {
        let worker = self.worker;
        K::Table::insert(self.env_mut(), worker, binding, record);
        self.write(done)
    }

    /// This environment's records, made if the ledger has none yet, to change before a
    /// [`ApplyCx::commit`]: for a kind that writes several changes at once (rate limiters).
    pub(crate) fn env_mut(&mut self) -> &mut Environment {
        self.lock
            .environments
            .entry(self.environment.to_string())
            .or_default()
    }

    /// Write the ledger. `failure` is the message when the write fails, before " in …".
    pub(crate) fn commit(&mut self, failure: &str) -> Result<(), DeployFailure> {
        write_lock(self.lock_path, self.lock).map_err(|e| {
            DeployFailure::driver(format!("{failure} in {}: {e}", self.lock_path.display()))
        })
    }

    /// Change this environment's records with `f`, if the ledger has the environment, and write
    /// the ledger, as [`ApplyCx::record`] does.
    pub(crate) fn change(
        &mut self,
        f: impl FnOnce(&mut Environment),
        done: &str,
    ) -> Result<(), DeployFailure> {
        if let Some(env) = self.lock.environments.get_mut(self.environment) {
            f(env);
        }
        self.write(done)
    }

    fn write(&mut self, done: &str) -> Result<(), DeployFailure> {
        self.commit(&format!("{done} but could not record it"))
    }
}

/// What provisioning one declared instance did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Provisioned<B> {
    /// The binding and what the push binds it to, written into the generated configuration.
    pub(crate) bound: Option<(String, B)>,
}

/// The account state a run reads once, for every kind: each kind takes its own with
/// [`ResourceKind::live_in`]. The default is a run that read nothing (a plan, or a test).
#[derive(Clone, Copy, Default)]
pub(crate) struct Lives<'a> {
    /// The account's KV namespace ids, when they were read ([`Kv::read_live`]).
    pub(crate) kv: Option<&'a BTreeSet<String>>,
    /// What each `Postgres` binding wants, and the API to ask.
    pub(crate) hyperdrive: super::hyperdrive::HyperdriveRun<'a>,
    /// The queues this run has already attempted to make, across Workers: a queue two contexts
    /// consume is one queue, made once per run (ADR 0194 D2). `None` attempts every queue.
    pub(crate) queues: Option<&'a std::cell::RefCell<BTreeSet<String>>>,
    /// The ids each stored secret's Secrets Store and each mTLS certificate was linked to before
    /// the run confirmed, by `(worker, binding)` ([`link_for_run`]). `None` links nothing.
    pub(crate) linked: Option<&'a BTreeMap<(String, String), String>>,
}

impl<'a> Lives<'a> {
    /// What a plan has read before the run authenticates: what each `Postgres` binding wants,
    /// and no API.
    pub(crate) fn for_plan(wants: &'a super::hyperdrive::Wants) -> Self {
        Lives {
            hyperdrive: super::hyperdrive::HyperdriveRun::new(wants, None),
            ..Lives::default()
        }
    }
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
    /// Account state the run reads once, before any Worker is provisioned, as the kind's stages
    /// see it.
    type Live<'a>: Copy;
    /// What the push binds one instance to, written into the generated configuration (KV: its
    /// id; D1: its name and id; R2, bound by name already: nothing).
    type Bound;

    /// The Worker's declared instances, in the order they are planned and provisioned.
    fn declared(r: &Resources) -> Vec<Self::Decl<'_>>;

    /// The binding `decl` is bound under: its ledger key within its Worker.
    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str;

    /// The kind's lines in a context's plan.
    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [Self::Line<'r>];
    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<Self::Line<'r>>;

    /// This kind's part of `lives`.
    fn live_in<'a>(lives: Lives<'a>) -> Self::Live<'a>;

    /// The plan line for `decl`.
    fn plan_line<'r>(cx: &PlanCx<'_>, decl: Self::Decl<'r>, live: Self::Live<'_>)
    -> Self::Line<'r>;

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

    /// The kind's orphans in a plan's orphan report. None for a kind that reports none.
    fn orphans_in(_orphans: &Orphans) -> &[Self::Orphan] {
        &[]
    }

    /// Put `found` in the kind's place in an orphan report.
    fn set_orphans(_orphans: &mut Orphans, _found: Vec<Self::Orphan>) {}

    /// Every recorded instance `cx.resources` no longer declares.
    fn orphans(_cx: &OrphanCx<'_>) -> Vec<Self::Orphan> {
        Vec::new()
    }

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

    /// Delete `orphan` from the account and forget it, through [`ApplyCx::change`]. Only called
    /// for a [`ResourceKind::prunable`] orphan; report-only by default.
    fn prune(_cx: &mut ApplyCx<'_, '_, Self>, _orphan: &Self::Orphan) -> Result<(), DeployFailure> {
        Ok(())
    }

    /// Create or reuse `decl` on the account, recording what it made through
    /// [`ApplyCx::record`].
    fn provision(
        cx: &mut ApplyCx<'_, '_, Self>,
        decl: Self::Decl<'_>,
    ) -> Result<Provisioned<Self::Bound>, DeployFailure>;

    /// Provision each of the Worker's declared instances, in order: one [`ResourceKind::provision`]
    /// each, unless the kind works on them together (queues make every one before recording any;
    /// rate limiters record their ids in one write). What the push binds, by binding.
    fn provision_all(
        cx: &mut ApplyCx<'_, '_, Self>,
        declared: &Resources,
    ) -> Result<BTreeMap<String, Self::Bound>, DeployFailure> {
        let mut bound = BTreeMap::new();
        for decl in Self::declared(declared) {
            if let Some((binding, id)) = Self::provision(cx, decl)?.bound {
                bound.insert(binding, id);
            }
        }
        Ok(bound)
    }

    /// Change the ledger once the Worker's push has succeeded: a rate limiter the source no
    /// longer declares is free again only then.
    fn after_push(_env: &mut Environment, _worker: &str, _declared: &Resources) {}

    /// Write the ids provisioning bound, by binding, into the generated configuration.
    fn materialise(
        _config: &Path,
        _worker: &str,
        _bound: &BTreeMap<String, Self::Bound>,
    ) -> Result<(), DeployFailure> {
        Ok(())
    }

    /// The id a pushed version binds for `record`: what its deploy facts keep, and what
    /// `bynk revert` compares the ledger with.
    fn bound_id(_record: &Self::Record) -> Option<&str> {
        None
    }
}

/// A place in the list that is not a kind, which a pass that prints fills where it sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Slot {
    /// The orphan report's Workers, which it lists after Hyperdrive's orphans and before the
    /// queues.
    WorkerOrphans,
    /// The plan's Analytics Engine datasets, after the queues and before the stored secrets.
    Datasets,
    /// The plan's foreign Service Bindings, email addresses and Durable Object classes, after the
    /// mTLS certificates and before the containers.
    BindingsAndObjects,
}

/// An orphan of a kind that reports none (a rate limiter is forgotten after the push instead):
/// it cannot be made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) enum NoOrphan {}

/// A pass over every kind: one stage of the deploy, generic in the kind.
pub(crate) trait KindVisitor {
    fn visit<K: ResourceKind>(&mut self);

    /// A place in the list that is not a kind.
    fn slot(&mut self, _slot: Slot) {}
}

/// The one list of kinds, in the plan's line order (the same in both formats).
pub(crate) fn for_each_kind(v: &mut impl KindVisitor) {
    v.visit::<Kv>();
    v.visit::<R2>();
    v.visit::<Vectorize>();
    v.visit::<D1>();
    v.visit::<Hyperdrive>();
    v.visit::<RateLimit>();
    v.slot(Slot::WorkerOrphans);
    v.visit::<Queue>();
    v.slot(Slot::Datasets);
    v.visit::<StoredSecret>();
    v.visit::<AiSearch>();
    v.visit::<MtlsCertificate>();
    v.slot(Slot::BindingsAndObjects);
    v.visit::<ContainerImage>();
}

/// Fill each kind's lines of `context`, for Worker `cx.worker`'s declarations `declared`, with what
/// the run has read in `lives`.
pub(crate) fn fill_kind_lines<'r>(
    context: &mut ContextPlan<'r>,
    declared: &'r Resources,
    cx: &PlanCx<'_>,
    lives: Lives<'_>,
) {
    struct Fill<'p, 'c, 'r, 'v> {
        context: &'p mut ContextPlan<'r>,
        declared: &'r Resources,
        cx: &'p PlanCx<'c>,
        lives: Lives<'v>,
    }
    impl KindVisitor for Fill<'_, '_, '_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            let live = K::live_in(self.lives);
            let lines = K::declared(self.declared)
                .into_iter()
                .map(|decl| K::plan_line(self.cx, decl, live))
                .collect();
            *K::lines_mut(self.context) = lines;
        }
    }
    for_each_kind(&mut Fill {
        context,
        declared,
        cx,
        lives,
    });
}

/// Each kind's lines of `context` in the short format, with `slots` printing the plan's other
/// lines at each [`Slot`].
pub(crate) fn short_kind_lines(
    context: &ContextPlan<'_>,
    out: &mut String,
    slots: &mut dyn FnMut(Slot, &mut String),
) {
    struct Short<'p, 'r> {
        context: &'p ContextPlan<'r>,
        out: &'p mut String,
        slots: &'p mut dyn FnMut(Slot, &mut String),
    }
    impl KindVisitor for Short<'_, '_> {
        fn visit<K: ResourceKind>(&mut self) {
            for line in K::lines(self.context) {
                K::short_line(line, self.out);
            }
        }
        fn slot(&mut self, slot: Slot) {
            (self.slots)(slot, self.out);
        }
    }
    for_each_kind(&mut Short {
        context,
        out,
        slots,
    });
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
            K::set_orphans(self.orphans, K::orphans(self.cx));
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
            let shown = K::orphans_in(self.from)
                .iter()
                .map(|orphan| K::shown_orphan(orphan, self.environment))
                .collect();
            K::set_orphans(self.to, shown);
        }
    }
    for_each_kind(&mut Shown {
        from,
        environment,
        to,
    });
}

/// Each kind's orphans of `orphans` in the short format, with `workers` printing the Worker
/// block at its [`Slot`].
pub(crate) fn short_kind_orphans(
    orphans: &Orphans,
    out: &mut String,
    workers: &mut dyn FnMut(&mut String),
) {
    struct Short<'p> {
        orphans: &'p Orphans,
        out: &'p mut String,
        workers: &'p mut dyn FnMut(&mut String),
    }
    impl KindVisitor for Short<'_> {
        fn visit<K: ResourceKind>(&mut self) {
            for orphan in K::orphans_in(self.orphans) {
                K::short_orphan(orphan, self.out);
            }
        }
        fn slot(&mut self, slot: Slot) {
            match slot {
                Slot::WorkerOrphans => (self.workers)(self.out),
                // The plan's lines, not its orphans.
                Slot::Datasets | Slot::BindingsAndObjects => {}
            }
        }
    }
    for_each_kind(&mut Short {
        orphans,
        out,
        workers,
    });
}

/// How many of each kind's orphans `--prune` deletes, printing what it says of each.
pub(crate) fn announce_prunable_kinds(orphans: &Orphans, environment: &str) -> usize {
    let (notices, count) = prune_notices(orphans, environment);
    for notice in &notices {
        notice.print();
    }
    count
}

/// What `--prune` says before it asks, in the list's order (ruled for 1D.1c), and how many
/// orphans it deletes.
pub(crate) fn prune_notices(orphans: &Orphans, environment: &str) -> (Vec<Notice>, usize) {
    struct Announce<'p> {
        orphans: &'p Orphans,
        environment: &'p str,
        notices: Vec<Notice>,
        count: usize,
    }
    impl KindVisitor for Announce<'_> {
        fn visit<K: ResourceKind>(&mut self) {
            for orphan in K::orphans_in(self.orphans) {
                if let Some(notice) = K::prune_notice(orphan, self.environment) {
                    self.notices.push(notice);
                }
                self.count += usize::from(K::prunable(orphan));
            }
        }
    }
    let mut announce = Announce {
        orphans,
        environment,
        notices: Vec::new(),
        count: 0,
    };
    for_each_kind(&mut announce);
    (announce.notices, announce.count)
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

/// Provision each of the declared instances of `K` at `site`, in order. Each records what it
/// made, and the ledger is written, before the next is made (ADR 0180: an interrupted run never
/// makes a second). What the push binds, by binding.
pub(crate) fn provision_kind<K: ResourceKind>(
    site: Site<'_>,
    declared: &Resources,
    lock: &mut DeployLock,
    lock_path: &Path,
    lives: Lives<'_>,
) -> Result<BTreeMap<String, K::Bound>, DeployFailure> {
    K::provision_all(
        &mut ApplyCx {
            environment: site.environment,
            worker: site.worker,
            project: site.project,
            lock,
            lock_path,
            live: K::live_in(lives),
            tools: site.tools,
        },
        declared,
    )
}

/// Each kind's change to the ledger once `worker`'s push has succeeded
/// ([`ResourceKind::after_push`]).
pub(crate) fn kinds_after_push(
    lock: &mut DeployLock,
    environment: &str,
    worker: &str,
    declared: &Resources,
) {
    struct After<'p> {
        env: Option<&'p mut Environment>,
        worker: &'p str,
        declared: &'p Resources,
    }
    impl KindVisitor for After<'_> {
        fn visit<K: ResourceKind>(&mut self) {
            if let Some(env) = self.env.as_deref_mut() {
                K::after_push(env, self.worker, self.declared);
            }
        }
    }
    for_each_kind(&mut After {
        env: lock.environments.get_mut(environment),
        worker,
        declared,
    });
}

/// Delete each kind's orphans `--prune` deletes, in the list's order, each forgotten and the
/// ledger written as it goes, so a prune interrupted half-way is resumed rather than repeated.
pub(crate) fn prune_kind_orphans(
    tools: Tools<'_>,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    orphans: &Orphans,
    lives: Lives<'_>,
) -> Result<(), DeployFailure> {
    struct Prune<'p, 'v> {
        tools: Tools<'p>,
        lock: &'p mut DeployLock,
        lock_path: &'p Path,
        environment: &'p str,
        orphans: &'p Orphans,
        lives: Lives<'v>,
        result: Result<(), DeployFailure>,
    }
    impl Prune<'_, '_> {
        fn prune<K: ResourceKind>(&mut self) -> Result<(), DeployFailure> {
            let project = BTreeMap::new();
            for orphan in K::orphans_in(self.orphans)
                .iter()
                .filter(|o| K::prunable(o))
            {
                K::prune(
                    &mut ApplyCx {
                        environment: self.environment,
                        worker: K::orphan_worker(orphan),
                        project: &project,
                        lock: self.lock,
                        lock_path: self.lock_path,
                        live: K::live_in(self.lives),
                        tools: self.tools,
                    },
                    orphan,
                )?;
            }
            Ok(())
        }
    }
    impl KindVisitor for Prune<'_, '_> {
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
        lives,
        result: Ok(()),
    };
    for_each_kind(&mut prune);
    prune.result
}

/// #3494: a plan action checked against whether the account holds the resource: a `create` of
/// one already there is `reuse`, since the deploy adopts it, and a `create or reuse` is decided
/// either way. A listing not read (`None`) changes nothing: the ledger's word stands.
pub(crate) fn held_verb(action: &'static str, held: Option<bool>) -> &'static str {
    match (action, held) {
        ("create" | "create or reuse", Some(true)) => "reuse",
        ("create or reuse", Some(false)) => "create",
        (other, _) => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one list, whose order every pass follows: the plan's line order, and (ruled for 1D.1c)
    /// `--prune`'s.
    #[test]
    fn the_kind_list_is_the_plans_order_with_the_worker_block_before_the_queues() {
        #[derive(Default)]
        struct Order(Vec<String>);
        impl KindVisitor for Order {
            fn visit<K: ResourceKind>(&mut self) {
                self.0.push(format!("{:?}", K::ID));
            }
            fn slot(&mut self, slot: Slot) {
                self.0.push(format!("{slot:?}"));
            }
        }
        let mut order = Order::default();
        for_each_kind(&mut order);
        assert_eq!(
            order.0,
            [
                "Kv",
                "R2",
                "Vectorize",
                "D1",
                "Hyperdrive",
                "RateLimit",
                "WorkerOrphans",
                "Queue",
                "Datasets",
                "StoredSecret",
                "AiSearch",
                "MtlsCertificate",
                "BindingsAndObjects",
                "ContainerImage",
            ]
        );
    }

    /// The order the maintainer ruled for `--prune` (22 November 2026): KV, Vectorize, Hyperdrive,
    /// then queues. R2 and D1 are reported, never deleted, and Workers are never pruned.
    #[test]
    fn prune_names_one_orphan_of_each_kind_in_the_ruled_order() {
        let orphans = Orphans {
            kv: vec![OrphanKv {
                worker: "gone".to_string(),
                binding: None,
            }],
            r2: vec![OrphanR2 {
                worker: "gone".to_string(),
                binding: "R2_FILES".to_string(),
                bucket: "gone-files".to_string(),
                jurisdiction: None,
            }],
            vectorize: vec![OrphanVectorize {
                worker: "gone".to_string(),
                binding: "VECTORIZE_DOCS".to_string(),
                index: "gone-docs".to_string(),
                adopted: false,
            }],
            d1: vec![OrphanD1 {
                worker: "gone".to_string(),
                binding: "D1_ORDERS".to_string(),
                database: "gone-orders".to_string(),
            }],
            hyperdrive: vec![OrphanHyperdrive {
                worker: "gone".to_string(),
                binding: "HYPERDRIVE_MAIN".to_string(),
                config: "gone-main".to_string(),
                id: "id-1".to_string(),
                adopted: false,
            }],
            workers: vec!["gone".to_string()],
            queues: vec!["jobs".to_string()],
            ..Orphans::default()
        };
        let (notices, count) = prune_notices(&orphans, "default");
        let text: Vec<&str> = notices
            .iter()
            .map(|notice| match notice {
                Notice::Styled(text) | Notice::Plain(text) => text.as_str(),
            })
            .collect();
        assert_eq!(
            text,
            [
                "bynk: will delete KV namespace for `gone`",
                "bynk: will delete Vectorize index `gone-docs` of `gone`'s `VECTORIZE_DOCS`, and every vector in it",
                "bynk: will delete Hyperdrive config `gone-main` of `gone`'s `HYPERDRIVE_MAIN`",
                "bynk: will delete queue `jobs`",
            ]
        );
        assert_eq!(count, 4);
        assert!(orphans.has_prunable());
    }
}
