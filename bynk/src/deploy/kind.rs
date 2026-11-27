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
//! pruning return what they would record or forget as [`Commit`]s, and the apply's one
//! [`LedgerWriter`] (`apply.rs`, 1D.2a) writes each before the kind's next account call, so a
//! record still lands before the push (ADR 0180).
//!
//! Converted so far: KV (1D.1a), R2 and D1 (1D.1b), Vectorize and Hyperdrive (1D.1c), rate limiters
//! and queues (1D.1d). The linked kinds and container images follow in 1D.1e, and until then keep
//! their own code after the loops, which is where they sit in the plan's order. The orphan report's
//! Worker block, which is not a kind, has a [`Slot`] in the list.
//!
//! `--prune` follows the list's order too, as the maintainer ruled for 1D.1c (22 November 2026):
//! its notices and its deletes used to come in orders of their own.
//!
//! 1D.2b (#4001): a kind's apply is its typed [`ResourceKind::Step`]s, decided at plan time from
//! the declarations and the ledger and held in the context's [`WorkerSteps`], and its
//! [`ResourceKind::apply`] of one. Where a kind's plan line is a view of its step (KV, R2, D1,
//! Hyperdrive, Vectorize), the line is rendered from the step, so the plan and the apply share one
//! decision. The apply's order is [`for_each_apply_phase`], not [`for_each_kind`].

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

/// The tools a kind's account calls go through: the account operations ([`CloudflareApi`], 1D.3)
/// and the project root.
#[derive(Clone, Copy)]
pub(crate) struct Tools<'a> {
    /// The account operations (1D.3, #4043): every call the apply makes to Cloudflare.
    pub(crate) api: &'a dyn CloudflareApi,
    pub(crate) project_root: &'a Path,
}

/// What a kind's provisioning and pruning act in: one Worker of one environment, the ledger as it
/// stands (and, for an account-wide name, across environments), the account state the run read
/// once, and the tools.
///
/// The ledger is read-only here (1D.2a, #4001): a kind says what it would record or forget as
/// [`Commit`]s, and the apply's one [`LedgerWriter`] writes each before the kind's next account
/// call, so a record still lands before the push, and before anything that could fail after it
/// (ADR 0180).
pub(crate) struct StepCx<'a, 'l, K: ResourceKind> {
    pub(crate) environment: &'a str,
    pub(crate) worker: &'a str,
    /// The whole project, for a check across it that a create re-runs.
    pub(crate) project: &'a BTreeMap<String, Resources>,
    pub(crate) lock: &'a DeployLock,
    pub(crate) live: K::Live<'l>,
    pub(crate) tools: Tools<'a>,
}

impl<K: ResourceKind> StepCx<'_, '_, K> {
    /// The ledger as it stands, for reading.
    pub(crate) fn ledger(&self) -> LedgerView<'_> {
        LedgerView::of(self.lock, self.environment)
    }

    /// The commit that records `record` for this Worker's `binding`. `done` says what was done to
    /// the account, for the message when the write fails: "{done} but could not record it in …".
    pub(crate) fn recording(
        &self,
        binding: &str,
        record: K::Record,
        done: &str,
    ) -> Commit<TableChange<K::Table>> {
        Commit::recorded(TableChange::insert(self.worker, binding, record), done)
    }
}

/// One change to one environment of the ledger, which the apply's [`LedgerWriter`] makes.
pub(crate) trait LedgerChange {
    /// Make the change in `environment` of `lock`. Whether the ledger is to be written for it: a
    /// queue already recorded as it would be is not (a re-run that provisions nothing writes
    /// nothing).
    fn apply(self, lock: &mut DeployLock, environment: &str) -> bool;
}

/// A record put in, or taken out of, a kind's [`LedgerTable`].
pub(crate) enum TableChange<T: LedgerTable> {
    /// Record `record` for `worker`'s `binding`, replacing any, making the environment if the
    /// ledger has none yet.
    Insert {
        worker: String,
        binding: String,
        record: T::Record,
    },
    /// Forget `worker`'s `binding`, if the ledger has the environment.
    Remove { worker: String, binding: String },
}

impl<T: LedgerTable> TableChange<T> {
    pub(crate) fn insert(worker: &str, binding: &str, record: T::Record) -> Self {
        TableChange::Insert {
            worker: worker.to_string(),
            binding: binding.to_string(),
            record,
        }
    }

    pub(crate) fn remove(worker: &str, binding: &str) -> Self {
        TableChange::Remove {
            worker: worker.to_string(),
            binding: binding.to_string(),
        }
    }
}

impl<T: LedgerTable> LedgerChange for TableChange<T> {
    fn apply(self, lock: &mut DeployLock, environment: &str) -> bool {
        match self {
            TableChange::Insert {
                worker,
                binding,
                record,
            } => T::insert(
                lock.environments
                    .entry(environment.to_string())
                    .or_default(),
                &worker,
                &binding,
                record,
            ),
            TableChange::Remove { worker, binding } => {
                if let Some(env) = lock.environments.get_mut(environment) {
                    T::remove(env, &worker, &binding);
                }
            }
        }
        true
    }
}

/// What one write of the ledger covers: its changes, made in order, and the message when the write
/// fails, before " in {path}: {error}".
pub(crate) struct Commit<C> {
    pub(crate) changes: Vec<C>,
    pub(crate) failure: String,
}

impl<C> Commit<C> {
    /// `changes` in one write, failing with `failure`.
    pub(crate) fn of(changes: Vec<C>, failure: String) -> Self {
        Commit { changes, failure }
    }

    /// One change, for what `done` did to the account: "{done} but could not record it".
    pub(crate) fn recorded(change: C, done: &str) -> Self {
        Commit::of(vec![change], format!("{done} but could not record it"))
    }
}

/// The links a Worker's instances of a linked kind (stored secrets, AI Search, mTLS certificates)
/// make, recorded in one write only when one of them changes what the ledger holds.
pub(crate) struct Links<K: ResourceKind> {
    pending: BTreeMap<String, K::Record>,
    changed: bool,
}

impl<K: ResourceKind> Links<K>
where
    K::Record: PartialEq,
    K::Table: LedgerTable<Record = K::Record>,
{
    pub(crate) fn new() -> Self {
        Links {
            pending: BTreeMap::new(),
            changed: false,
        }
    }

    /// Record `link` for this Worker's `binding`, unless it is what the ledger (or an earlier link
    /// of this Worker) already holds.
    pub(crate) fn link(&mut self, cx: &StepCx<'_, '_, K>, binding: &str, link: K::Record) {
        let held = self
            .pending
            .get(binding)
            .or_else(|| cx.ledger().recorded::<K>(cx.worker, binding));
        if held == Some(&link) {
            return;
        }
        self.pending.insert(binding.to_string(), link);
        self.changed = true;
    }

    /// The one write of `worker`'s links when any changed, failing with "{failure} in …".
    pub(crate) fn commit(
        self,
        worker: &str,
        failure: String,
    ) -> Option<Commit<TableChange<K::Table>>> {
        self.changed.then(|| {
            Commit::of(
                self.pending
                    .into_iter()
                    .map(|(binding, link)| TableChange::insert(worker, &binding, link))
                    .collect(),
                failure,
            )
        })
    }
}

/// What applying one step did (1D.2b, #4001 Decision A): what the push binds, and what to
/// record, each [`Commit`] written in order before the next step is applied.
pub(crate) struct Applied<K: ResourceKind> {
    /// What the push binds, by binding, written into the generated configuration.
    pub(crate) bound: Vec<(String, K::Bound)>,
    /// What to record, each written in order before anything else is asked of the account.
    pub(crate) commits: Vec<Commit<K::Change>>,
    /// #4602: what to tell the user the step did that its plan line did not forecast (a namespace
    /// made again, a queue adopted), printed once its commits are written.
    pub(crate) notes: Vec<Notice>,
}

impl<K: ResourceKind> Applied<K> {
    /// Nothing bound and nothing to record.
    pub(crate) fn nothing() -> Self {
        Applied {
            bound: Vec::new(),
            commits: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// `binding` bound to `bound`, with nothing to record.
    pub(crate) fn bound(binding: &str, bound: K::Bound) -> Self {
        Applied {
            bound: vec![(binding.to_string(), bound)],
            commits: Vec::new(),
            notes: Vec::new(),
        }
    }

    /// And `note` to print after the step's commits are written (#4602).
    pub(crate) fn noting(mut self, note: Notice) -> Self {
        self.notes.push(note);
        self
    }

    /// And `commit` to write.
    pub(crate) fn with(mut self, commit: Commit<K::Change>) -> Self {
        self.commits.push(commit);
        self
    }

    /// And `commit` to write, if there is one.
    pub(crate) fn with_any(self, commit: Option<Commit<K::Change>>) -> Self {
        match commit {
            Some(commit) => self.with(commit),
            None => self,
        }
    }
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
    /// the run confirmed, by `(worker, binding)` ([`link_for_run`]). With `None`, every declared
    /// stored secret or certificate is reported as not linked.
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
    type Bound: 'static;
    /// One change provisioning or pruning makes to the kind's records, which the apply writes
    /// (most kinds: a [`TableChange`] of their table).
    type Change: LedgerChange;
    /// One unit of the apply (1D.2b, #4001 Decision A): what one write of the ledger covers, so
    /// the write timing cannot move. Decided at plan time from the declarations and the ledger,
    /// never from the account, and held in the context's [`WorkerSteps`].
    type Step<'r>: std::fmt::Debug;

    /// What the apply may still ask of the account on top of what a step decided (#4001
    /// Decision B), each named. Pinned by a test, so a new one is a reviewed diff; nothing else
    /// reads it.
    #[cfg_attr(not(test), allow(dead_code))]
    const GUARDS: &'static [&'static str] = &[];

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

    /// Delete `orphan` from the account, and what forgetting it changes. Only called for a
    /// [`ResourceKind::prunable`] orphan; report-only by default.
    fn prune(
        _cx: &StepCx<'_, '_, Self>,
        _orphan: &Self::Orphan,
    ) -> Result<Option<Commit<Self::Change>>, DeployFailure> {
        Ok(None)
    }

    /// The Worker's steps, in the order they are applied, decided from its declarations and the
    /// ledger.
    fn steps<'r>(
        cx: &PlanCx<'_>,
        declared: &'r Resources,
        live: Self::Live<'_>,
    ) -> Vec<Self::Step<'r>>;

    /// The kind's steps in a context's [`WorkerSteps`].
    fn steps_of<'p, 'r>(steps: &'p WorkerSteps<'r>) -> &'p [Self::Step<'r>];
    fn steps_mut<'p, 'r>(steps: &'p mut WorkerSteps<'r>) -> &'p mut Vec<Self::Step<'r>>;

    /// Do `step` against the account. The ledger is read-only here; what to record is returned.
    fn apply(
        cx: &StepCx<'_, '_, Self>,
        step: &Self::Step<'_>,
    ) -> Result<Applied<Self>, DeployFailure>;

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

/// A context's steps (1D.2b, #4001): one list per kind, and the Worker the other apply phases
/// work on. Held in its [`ContextPlan`], never printed: the plan's two formats cannot see them.
#[derive(Debug, Default)]
pub(crate) struct WorkerSteps<'r> {
    pub(crate) kv: Vec<KvStep<'r>>,
    pub(crate) r2: Vec<R2Step<'r>>,
    pub(crate) vectorize: Vec<VectorizeStep<'r>>,
    pub(crate) d1: Vec<D1Step<'r>>,
    pub(crate) hyperdrive: Vec<HyperdriveStep<'r>>,
    pub(crate) ratelimits: Vec<RateLimitStep<'r>>,
    pub(crate) queues: Vec<QueueStep<'r>>,
    pub(crate) stored_secrets: Vec<StoredSecretStep<'r>>,
    pub(crate) ai_search: Vec<AiSearchStep<'r>>,
    pub(crate) mtls_certificates: Vec<MtlsCertificateStep<'r>>,
    pub(crate) containers: Vec<ContainerImageStep<'r>>,
    /// The Worker these steps are for, by its name in the project, and what it declared: the
    /// non-kind phases ([`ApplySlot`]) read it, since the push's secrets and deploy facts, the
    /// `[env.<name>]` block and the migrations take a declaration (Decision D).
    pub(crate) worker: Option<(&'r str, &'r Resources)>,
}

/// A phase of a Worker's apply that is not a kind's step ([`for_each_apply_phase`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ApplySlot {
    /// A named environment's `[env.<name>]` block, synthesised into the generated configuration.
    EnvironmentBlock,
    /// An adopted Vectorize index's account name, written into the configuration.
    AdoptedVectorize,
    /// An adopted container image, written into the configuration.
    AdoptedContainers,
    /// Each D1 database's pending migrations.
    Migrations,
    /// The secrets, then `wrangler deploy`.
    Push,
}

/// A pass over one Worker's apply, in [`for_each_apply_phase`]'s order.
pub(crate) trait ApplyVisitor {
    /// Apply each of the kind's steps.
    fn apply<K: ResourceKind>(&mut self);
    /// Write what the kind's steps bound into the generated configuration.
    fn materialise<K: ResourceKind>(&mut self);
    /// A phase that is not a kind's.
    fn slot(&mut self, slot: ApplySlot);
}

/// The one order a Worker is applied in (#4001 Decision D), which is not [`for_each_kind`]'s:
/// everything the push needs exists first, the linked kinds before the rate limiters and queues,
/// and each kind's materialisation where it was (KV's after the rate limiters'). Which resources
/// exist after a failure follows from it, so changing it is a ruled change, as `--prune`'s order
/// was for 1D.1c. Pinned by a test.
pub(crate) fn for_each_apply_phase(v: &mut impl ApplyVisitor) {
    v.apply::<Kv>();
    v.apply::<R2>();
    v.apply::<Vectorize>();
    v.apply::<D1>();
    v.materialise::<D1>();
    v.apply::<Hyperdrive>();
    v.materialise::<Hyperdrive>();
    v.apply::<StoredSecret>();
    v.materialise::<StoredSecret>();
    v.apply::<MtlsCertificate>();
    v.materialise::<MtlsCertificate>();
    v.apply::<AiSearch>();
    v.apply::<RateLimit>();
    v.materialise::<RateLimit>();
    v.materialise::<Kv>();
    v.apply::<Queue>();
    v.slot(ApplySlot::EnvironmentBlock);
    v.slot(ApplySlot::AdoptedVectorize);
    v.slot(ApplySlot::AdoptedContainers);
    v.apply::<ContainerImage>();
    v.slot(ApplySlot::Migrations);
    v.slot(ApplySlot::Push);
}

/// Fill each kind's lines and steps of `context`, for Worker `cx.worker`'s declarations
/// `declared`, with what the run has read in `lives`.
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
            *K::steps_mut(&mut self.context.steps) = K::steps(self.cx, self.declared, live);
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

    /// The apply's order (#4001 Decision D), today's `deploy_one` order exactly: the linked kinds
    /// before the rate limiters and queues, mTLS before AI Search, KV materialised after the rate
    /// limiters, and the push last. A change here changes what exists after a failure.
    #[test]
    fn the_apply_order_is_pinned() {
        #[derive(Default)]
        struct Order(Vec<String>);
        impl ApplyVisitor for Order {
            fn apply<K: ResourceKind>(&mut self) {
                self.0.push(format!("{:?}", K::ID));
            }
            fn materialise<K: ResourceKind>(&mut self) {
                self.0.push(format!("Materialise({:?})", K::ID));
            }
            fn slot(&mut self, slot: ApplySlot) {
                self.0.push(format!("{slot:?}"));
            }
        }
        let mut order = Order::default();
        for_each_apply_phase(&mut order);
        assert_eq!(
            order.0,
            [
                "Kv",
                "R2",
                "Vectorize",
                "D1",
                "Materialise(D1)",
                "Hyperdrive",
                "Materialise(Hyperdrive)",
                "StoredSecret",
                "Materialise(StoredSecret)",
                "MtlsCertificate",
                "Materialise(MtlsCertificate)",
                "AiSearch",
                "RateLimit",
                "Materialise(RateLimit)",
                "Materialise(Kv)",
                "Queue",
                "EnvironmentBlock",
                "AdoptedVectorize",
                "AdoptedContainers",
                "ContainerImage",
                "Migrations",
                "Push",
            ]
        );
    }

    /// Where the apply still asks the account on top of what a step decided (#4001 Decision B):
    /// a new guard is a change to this list, so review sees it.
    #[test]
    fn the_apply_time_guards_are_named() {
        #[derive(Default)]
        struct Guards(Vec<(String, &'static str)>);
        impl KindVisitor for Guards {
            fn visit<K: ResourceKind>(&mut self) {
                for guard in K::GUARDS {
                    self.0.push((format!("{:?}", K::ID), guard));
                }
            }
        }
        let mut guards = Guards::default();
        for_each_kind(&mut guards);
        let kinds: Vec<&str> = guards.0.iter().map(|(kind, _)| kind.as_str()).collect();
        assert_eq!(
            kinds,
            [
                "Kv",
                "Vectorize",
                "Vectorize",
                "Vectorize",
                "RateLimit",
                "Queue",
                "Queue"
            ],
            "{:#?}",
            guards.0
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
