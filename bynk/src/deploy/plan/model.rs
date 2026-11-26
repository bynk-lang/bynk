use super::run::DeployFormat;
use super::run::DeployOptions;
use crate::deploy::agent_move::agent_state_note;
use crate::deploy::config::DurableObject;
use crate::deploy::config::Resources;
use crate::deploy::config::analytics_dataset_name;
use crate::deploy::config::deployed_worker_name;
use crate::deploy::hyperdrive;
use crate::deploy::kind::LedgerView;
use crate::deploy::kind::Lives;
use crate::deploy::kind::PlanAiSearch;
use crate::deploy::kind::PlanContainer;
use crate::deploy::kind::PlanCx;
use crate::deploy::kind::PlanD1;
use crate::deploy::kind::PlanHyperdrive;
use crate::deploy::kind::PlanKv;
use crate::deploy::kind::PlanMtlsCertificate;
use crate::deploy::kind::PlanQueue;
use crate::deploy::kind::PlanR2;
use crate::deploy::kind::PlanRateLimit;
use crate::deploy::kind::PlanStoredSecret;
use crate::deploy::kind::PlanVectorize;
use crate::deploy::kind::ProjectCx;
use crate::deploy::kind::Slot;
use crate::deploy::kind::WorkerSteps;
use crate::deploy::kind::check_kind_lines;
use crate::deploy::kind::fill_kind_lines;
use crate::deploy::kind::kind_listings;
use crate::deploy::kind::kind_name_problem;
use crate::deploy::kind::kind_unrecordable_in_ci;
use crate::deploy::kind::short_kind_lines;
use crate::deploy::kind::short_kind_orphans;
use crate::deploy::ledger::DeployLock;
use crate::deploy::ledger::Orphans;
use crate::deploy::ledger::UNRESTRICTED_AGENT;
use crate::deploy::ledger::confirm_deletion_prompt;
use crate::deploy::ledger::find_orphans;
use crate::deploy::secrets::Origin;
use crate::deploy::secrets::SecretSource;
use crate::deploy::secrets::WantedSecret;
use crate::deploy::secrets::edge_keys_of;
use crate::deploy::secrets::project_edges;
use crate::deploy::secrets::wanted_secrets;
use bynk_driver::emsg;
use serde::Serialize;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::io;
use std::io::{IsTerminal, Write};
use std::path::Path;

/// The whole-project plan (slice 2). `order` is the upload order, dependencies
/// first; `contexts` carries it with each context's own actions. Slice 0's
/// single-worker `Plan` is the one-element case of this.
#[derive(Debug, Serialize)]
pub(crate) struct Plan<'a> {
    environment: &'a str,
    /// #3797: why the run cannot go ahead, as entries a client reads without matching stderr. A
    /// plan that is printed with entries here exits non-zero. Always present, so a client reads
    /// one shape.
    refusals: Vec<super::Entry>,
    /// #3797: what the run says about itself but goes ahead with.
    warnings: Vec<super::Entry>,
    /// Slice 5: every ledger entry this environment no longer declares, by
    /// its ledger key, which is what `--prune` deletes by. Not serialised:
    /// the plan shows [`Plan::shown_orphans`].
    #[serde(skip)]
    pub(crate) orphans: Orphans,
    /// The same orphans as the account names them (#2028): a queue or Worker
    /// under its [`env_qualify`](crate::deploy::config::env_qualify)d name. Printed before the per-context
    /// breakdown, so an orphan is seen before what's actually being pushed.
    #[serde(rename = "orphans")]
    pub(super) shown_orphans: Orphans,
    /// The resolved upload order — the plan's headline, since Cloudflare
    /// rejects a Worker uploaded before its binding target. Each Worker by
    /// its [`deployed_worker_name`] (#2028).
    pub(super) order: Vec<String>,
    pub(crate) contexts: Vec<ContextPlan<'a>>,
    /// #3494: what checking the plan against the account found to say, printed
    /// after it.
    #[serde(skip)]
    account_notes: Vec<String>,
    /// #3943: what a named environment takes from `default` (its `env(…)`
    /// constants left to `[vars]` or a default, and `Service @worker` and `Email`
    /// values equal to `default`'s). Advisory; present in the JSON only when
    /// there is any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    shared_with_default: Vec<super::shared::SharedWithDefault>,
}

#[derive(Debug, Serialize)]
pub(crate) struct ContextPlan<'a> {
    /// The Worker this context deploys as: its [`deployed_worker_name`] in
    /// this environment, the name the push hands Wrangler (#2028).
    pub(crate) worker: String,
    /// One line per KV namespace the Worker binds: the bare `KV` first, then
    /// each derived instance in binding order (S4b, #1967). Empty for a
    /// context with no KV. Filled by [`Kv`](crate::deploy::kind::Kv) (1D.1a, #3929).
    pub(crate) kv: Vec<PlanKv<'a>>,
    /// P5 (#2323): one line per R2 bucket the Worker binds, in binding order. Filled by [`R2`](crate::deploy::kind::R2)
    /// (1D.1b, #3929).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) r2: Vec<PlanR2<'a>>,
    /// #3479: one line per Vectorize index the Worker binds, in binding order. Filled by
    /// [`Vectorize`](crate::deploy::kind::Vectorize) (1D.1c, #3929).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) vectorize: Vec<PlanVectorize<'a>>,
    /// P4a (#2602): one line per D1 database the Worker binds, in binding
    /// order. Filled by [`D1`](crate::deploy::kind::D1) (1D.1b, #3929).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) d1: Vec<PlanD1<'a>>,
    /// #3319: one line per Hyperdrive config the Worker binds (a `Postgres`
    /// instance), in binding order. Never the connection string. Filled by [`Hyperdrive`](crate::deploy::kind::Hyperdrive)
    /// (1D.1c, #3929).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) hyperdrive: Vec<PlanHyperdrive<'a>>,
    /// P3 (#2342): one line per rate limiter the Worker binds, in binding
    /// order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) ratelimits: Vec<PlanRateLimit<'a>>,
    /// One line per queue this context consumes or (P2, #2578) sends to, in
    /// name order.
    pub(crate) queues: Vec<PlanQueue>,
    /// P3b (#2582): one line per Analytics Engine dataset the Worker writes
    /// to, in binding order. No action: the platform creates a dataset on its
    /// first write, and Bynk never deletes one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(super) datasets: Vec<PlanDataset<'a>>,
    /// #3301: one line per Secrets Store secret the Worker reads, in binding
    /// order. Linked: found on the account before the push, never created,
    /// never deleted. Whether it exists is asked after authenticating, so the
    /// plan, which stays offline under `--dry-run`, does not claim it. Filled by
    /// [`StoredSecret`](crate::deploy::kind::StoredSecret) (1D.1e, #3929).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) stored_secrets: Vec<PlanStoredSecret<'a>>,
    /// #3480: one line per AI Search instance the Worker queries, in binding
    /// order. Linked: found on the account before the push, never created,
    /// never deleted, and asked after authenticating, so the plan does not
    /// claim it exists. Filled by [`AiSearch`](crate::deploy::kind::AiSearch) (1D.1e, #3929).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) ai_search: Vec<PlanAiSearch<'a>>,
    /// #3513 A: one line per mTLS certificate the Worker presents, in binding order. Linked: found
    /// on the account before the push and never uploaded, and asked after authenticating, so the
    /// plan, which stays offline under `--dry-run`, does not claim it exists. Filled by
    /// [`MtlsCertificate`](crate::deploy::kind::MtlsCertificate) (1D.1e, #3929).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) mtls_certificates: Vec<PlanMtlsCertificate<'a>>,
    /// #1796: one line per Durable Object class the push declares in
    /// `exports`, in class-name order. Empty for a context with no agent and
    /// no events fan-out.
    pub(super) durable_objects: Vec<PlanDurableObject<'a>>,
    /// #3477: one line per container, in class-name order: where its image comes
    /// from and the instance type Bynk passes when it starts. The application
    /// itself is `wrangler deploy`'s to reconcile, which the line says. Filled by
    /// [`ContainerImage`](crate::deploy::kind::ContainerImage) (1D.1e, #3929).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) containers: Vec<PlanContainer<'a>>,
    /// State-migrations S3 (#2248): each class an `agent New replaces Old`
    /// renames — the push moves its stored data. In old-name order.
    #[serde(
        rename = "durable_objects_renamed",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(super) renamed_durable_objects: Vec<PlanDurableObjectRename<'a>>,
    /// State-migrations S3 (#2248): each class a `removed agent` deletes, as
    /// `<context>.<Class>` — the name `--confirm-delete` takes. Irreversible.
    #[serde(
        rename = "durable_objects_deleted",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(super) deleted_durable_objects: Vec<PlanDeletion>,
    /// #3819: each agent class whose `@jurisdiction` differs from the one it was last pushed
    /// under. Its Durable Object ids change, so every existing instance is orphaned:
    /// destructive, and refused unless `--confirm-jurisdiction` names it.
    #[serde(
        rename = "durable_objects_jurisdiction_changed",
        skip_serializing_if = "Vec::is_empty"
    )]
    pub(super) jurisdiction_changes: Vec<PlanJurisdictionChange>,
    /// One line per secret this run will set on this context, in name order.
    secrets: Vec<PlanSecret>,
    /// False when this context reads at least one secret the compiler cannot
    /// name — a computed `Secrets.get` argument, or an adapter binding `given`
    /// `bynk.Secrets` (#1976) — so `secrets` is **not** everything it reads
    /// (ADR 0196 D2).
    ///
    /// Carried in the machine surface as well as the human one, because this is
    /// the field that stops a CI job trusting a short list — the failure the
    /// whole increment exists to prevent is a reader taking silence for absence.
    secrets_complete: bool,
    /// `deploy` first time, `redeploy` when the ledger has pushed it before —
    /// the honest word, since a re-run re-pushes rather than skipping.
    pub(super) action: &'static str,
    /// The workers this one binds to, in the emitted config, each by its
    /// [`deployed_worker_name`] — the target the push's Service Binding names.
    pub(super) binds_to: Vec<String>,
    /// Interop I5 (#1972): its Service Bindings to Workers outside the
    /// project, in binding order. Bound, never provisioned.
    foreign_services: Vec<PlanForeignService<'a>>,
    /// #3328: the addresses the Worker's `from email` services claim, in address
    /// order. Advisory: wrangler reconciles the Email Routing rules to this list
    /// on the push, so one no longer listed loses its rule.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    email_addresses: Vec<&'a str>,
    /// #3328: addresses the last push carried that this one no longer lists.
    /// Wrangler deletes their rules, so mail to them would bounce: the run is
    /// refused unless `--prune` confirms it ([`confirm_address_removals`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed_email_addresses: Vec<String>,
    /// #3883: the named environment this Worker has `from email` addresses in the source for
    /// but claims none in, because `[deploy.email]` maps it none. Its push leaves
    /// `addresses` out, which Wrangler reads as "change no Email Routing rule".
    #[serde(skip_serializing_if = "Option::is_none")]
    email_unclaimed: Option<&'a str>,
    /// #3883: the addresses the last push of an environment that now claims none carried.
    /// Their Email Routing rules are still live (an omitted `addresses` changes no rule) and
    /// still point at this Worker, which no longer accepts mail to them. Kept in the ledger.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    email_left_in_place: Vec<String>,
    /// 1D.2b (#4001): what the apply executes for this context. Never printed.
    #[serde(skip)]
    pub(crate) steps: WorkerSteps<'a>,
}

impl<'a> ContextPlan<'a> {
    /// The steps the apply executes for this context ([`apply_worker`](crate::deploy::apply::apply_worker)).
    pub(crate) fn steps(&self) -> &WorkerSteps<'a> {
        &self.steps
    }
}

/// Interop I5 (#1972): one Service Binding to a foreign Worker. No action:
/// `bynk deploy` neither creates nor changes the Worker. Whether it exists is
/// asked of the account after authenticating, so `--dry-run` stays offline
/// and the plan never claims it.
#[derive(Debug, Serialize)]
struct PlanForeignService<'a> {
    binding: &'a str,
    worker: &'a str,
}

/// P3b (#2582): one Analytics Engine dataset.
#[derive(Debug, Serialize)]
pub(super) struct PlanDataset<'a> {
    /// `ANALYTICS_<SNAKE>`.
    binding: &'a str,
    /// The dataset's account-facing name, [`analytics_dataset_name`]d.
    pub(super) dataset: String,
}

/// One secret the run intends to set on one context.
///
/// There is deliberately **no presence field**. Presence is a live question
/// (`wrangler secret list`), and the plan is derived before `deploy`
/// authenticates — which is what keeps `--dry-run` working offline. So the plan
/// says what it will *try*, and the run reports the skip when a secret turns out
/// to be there. The ledger cannot help: it records no secret at all, because a
/// recorded presence could only ever be a stale one (ADR 0195 D1/D4).
#[derive(Debug, Serialize)]
struct PlanSecret {
    /// Owned: the name set is derived (declared ∪ supplied) rather than
    /// borrowed from any one source.
    name: String,
    /// `declared` — the compiler proved a handler reads it. `supplied` — the
    /// user named it. The mark is the floor-not-census contract made legible:
    /// no `declared` line for a `bynk.Secrets` name does **not** mean the
    /// context needs none (ADR 0195 D2).
    origin: Origin,
    /// `set`, or `overwrite` under `--force`. A `set` line may still report a
    /// skip at wire time — see the type's note.
    action: &'static str,
}

/// One Durable Object class the push declares (#1796). It replaces the
/// migration-tag line ADR 0194 D1 described: with `exports` there is no tag,
/// only a declared set that Cloudflare reconciles on every deploy.
#[derive(Debug, Serialize)]
pub(super) struct PlanDurableObject<'a> {
    pub(super) class: &'a str,
    /// The backend the config declares (`sqlite`), read from the file rather
    /// than assumed, so the plan and the upload can't disagree.
    pub(super) storage: &'a str,
    /// #3819: the `@jurisdiction` the agent's ids are minted under, when it has one. Not in the
    /// config the push reads: the Worker reaches the namespace through
    /// `namespace.jurisdiction(…)`, and `deploy` records it per class.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) jurisdiction: Option<&'a str>,
    /// Always `Cloudflare`, and that is the point: the field names an owner
    /// other than `bynk`, which is the whole content of the advisory (ADR 0194
    /// D1's principle). A consumer reading the plan learns that this line is
    /// not a claim that the namespace exists or doesn't, without having to
    /// know the ADR.
    pub(super) reconciled_by: &'static str,
}

/// State-migrations S3 (#2248): one Durable Object class rename the push
/// applies through an `exports` tombstone. `already_applied` when an earlier
/// push in this environment carried it (#2276 Decision D): the data has moved,
/// and the declaration is waiting for Cloudflare's `stale_tombstone`.
#[derive(Debug, Serialize)]
pub(super) struct PlanDurableObjectRename<'a> {
    from: &'a str,
    to: &'a str,
    pub(super) already_applied: bool,
}

/// State-migrations S3 (#2248): one `deleted` tombstone the push carries.
/// `already_applied` when an earlier push in this environment carried it (the
/// ledger says so, #2253 review): the declaration is waiting for Cloudflare's
/// `stale_tombstone`, nothing more is destroyed, and no confirmation is asked.
#[derive(Debug, Serialize)]
pub(super) struct PlanDeletion {
    pub(super) class: String,
    pub(super) already_applied: bool,
}

/// #3819: one agent class whose `@jurisdiction` changed since its last push: its name as
/// `--confirm-jurisdiction` takes it (`<context>.<Class>`), and the jurisdiction before and
/// after (`none` for the unrestricted namespace).
#[derive(Debug, Serialize)]
pub(super) struct PlanJurisdictionChange {
    pub(super) class: String,
    pub(super) from: String,
    pub(super) to: String,
}

/// State-migrations S3 (#2248): a class's name as `--confirm-delete` takes it,
/// `<context>.<Class>`. A Worker's directory name is its context's qualified
/// name with `.` replaced by `-` (`worker_dir_name`), and an identifier holds
/// no `-`, so the context name is recovered exactly.
fn qualified_class(worker: &str, class: &str) -> String {
    format!("{}.{class}", worker.replace('-', "."))
}

/// What the plan would destroy must be confirmed before anything is pushed: a
/// Durable Object class by name (`--confirm-delete`), and an Email Routing rule
/// by `--prune`.
pub(super) fn confirm_destructive(
    plan: &Plan<'_>,
    opts: &DeployOptions,
    lock: &DeployLock,
) -> bool {
    confirm_deletions(plan, &opts.confirm_delete, lock)
        && confirm_jurisdiction_changes(plan, &opts.confirm_jurisdiction, lock)
        && confirm_address_removals(plan, opts.prune, opts.yes)
}

/// #3328 (ADR 0805 F): wrangler deletes the Email Routing rule of an address a
/// Worker no longer lists, and mail to it then bounces. So a removal is
/// refused, before anything is pushed, unless `--prune` confirms it through the
/// ordinary prune prompt (`--yes` answers it). A first deploy has no record, so
/// removes nothing; an added address asks nothing.
pub(super) fn confirm_address_removals(plan: &Plan<'_>, prune: bool, yes: bool) -> bool {
    let removed: Vec<(&str, &str)> = plan
        .contexts
        .iter()
        .flat_map(|c| {
            c.removed_email_addresses
                .iter()
                .map(move |a| (c.worker.as_str(), a.as_str()))
        })
        .collect();
    if removed.is_empty() {
        return true;
    }
    if !prune {
        let list = removed
            .iter()
            .map(|(worker, address)| format!("`{address}` (`{worker}`)"))
            .collect::<Vec<_>>()
            .join(", ");
        emsg!(
            "bynk: error: this deploy would remove the Email Routing rule for {list}, and mail to it would bounce; nothing was pushed. Pass --prune to confirm the removal, or put the address back"
        );
        return false;
    }
    for (worker, address) in &removed {
        emsg!("bynk: will remove the Email Routing rule for `{address}` (`{worker}`)");
    }
    confirm_deletion_prompt(yes, removed.len())
}

/// State-migrations S3 (#2248, Decision E): every class this plan deletes must
/// be confirmed by name — by `--confirm-delete`, or by typing it at an
/// interactive prompt. `--yes` never stands in, and a confirmation for a class
/// the plan does not delete is refused, so a stale CI flag cannot approve a
/// different deletion.
pub(super) fn confirm_deletions(plan: &Plan<'_>, confirmed: &[String], lock: &DeployLock) -> bool {
    let all: Vec<&PlanDeletion> = plan
        .contexts
        .iter()
        .flat_map(|c| c.deleted_durable_objects.iter())
        .collect();
    // Only a pending deletion destroys anything, so only it is confirmed.
    let deletions: Vec<&String> = all
        .iter()
        .filter(|d| !d.already_applied)
        .map(|d| &d.class)
        .collect();
    for name in confirmed {
        // A confirmation for a deletion already applied — in this plan, or
        // recorded by the ledger after its declaration was removed — is
        // harmless, so a CI flag left in place doesn't fail the next deploy.
        let applied = all.iter().any(|d| &d.class == name)
            || name.rsplit_once('.').is_some_and(|(context, class)| {
                lock.has_deleted_class(plan.environment, &context.replace('.', "-"), class)
            });
        if !deletions.contains(&name) && !applied {
            emsg!(
                "bynk: error: --confirm-delete {name} names no class this plan deletes; refusing"
            );
            return false;
        }
    }
    let unconfirmed: Vec<&&String> = deletions
        .iter()
        .filter(|d| !confirmed.iter().any(|c| c == **d))
        .collect();
    if unconfirmed.is_empty() {
        return true;
    }
    if !io::stdin().is_terminal() {
        emsg!(
            "bynk: error: this deploy deletes stored agent data; confirm each class with {}",
            unconfirmed
                .iter()
                .map(|d| format!("--confirm-delete {d}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
        return false;
    }
    for name in unconfirmed {
        eprint!(
            "Deleting `{name}` destroys all of its stored data, irreversibly. Type `{name}` to \
             confirm: "
        );
        let _ = io::stderr().flush();
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err() || answer.trim() != name.as_str() {
            emsg!("bynk: error: `{name}` not confirmed; nothing was deployed");
            return false;
        }
    }
    true
}

/// #3819: each agent class of `worker` whose declared `@jurisdiction` differs from the one it was
/// last pushed under. The ledger's record is what the last push hosted. A Worker that has been
/// deployed but whose ledger holds no record of its classes at all (one from before `@jurisdiction`)
/// was pushed unrestricted, since every agent was. Once the Worker has a record, a class missing
/// from it is new, and so is a Worker never pushed: neither has instances to orphan. A class that
/// is the target of a rename (`agent New replaces Old`) keeps its namespace and its instances, so
/// it is compared with the old class's record, unless it has been pushed under its new name since.
fn jurisdiction_changes(
    lock: &DeployLock,
    environment: &str,
    worker: &str,
    objects: &[DurableObject],
    renames: &[(String, String)],
) -> Vec<PlanJurisdictionChange> {
    let deployed = lock.is_deployed(environment, worker);
    let recorded_any = lock.has_agent_jurisdictions(environment, worker);
    objects
        .iter()
        .filter_map(|object| {
            let now = object.jurisdiction.as_deref().unwrap_or(UNRESTRICTED_AGENT);
            let before = match lock.recorded_agent_jurisdiction(environment, worker, &object.class)
            {
                Some(recorded) => recorded,
                None => match renames.iter().find(|(_, to)| *to == object.class) {
                    // The rename's old class was deployed, so it was recorded, or, in a ledger
                    // from before `@jurisdiction`, was unrestricted.
                    Some((from, _)) => lock
                        .recorded_agent_jurisdiction(environment, worker, from)
                        .or(deployed.then_some(UNRESTRICTED_AGENT))?,
                    None if deployed && !recorded_any => UNRESTRICTED_AGENT,
                    None => return None,
                },
            };
            (before != now).then(|| PlanJurisdictionChange {
                class: qualified_class(worker, &object.class),
                from: before.to_string(),
                to: now.to_string(),
            })
        })
        .collect()
}

/// #3819: a changed `@jurisdiction` orphans every instance of the class, so it must be confirmed
/// by name — by `--confirm-jurisdiction`, or by typing it at an interactive prompt. `--yes` never
/// stands in. A confirmation for a class this plan does not change is refused, so a stale CI flag
/// cannot approve a different change, unless the ledger already records the class under this Worker:
/// the change was applied by an earlier deploy (or a deploy that stopped part-way), which is harmless,
/// as `--confirm-delete` treats a deletion already applied.
pub(super) fn confirm_jurisdiction_changes(
    plan: &Plan<'_>,
    confirmed: &[String],
    lock: &DeployLock,
) -> bool {
    let changes: Vec<&PlanJurisdictionChange> = plan
        .contexts
        .iter()
        .flat_map(|c| c.jurisdiction_changes.iter())
        .collect();
    for name in confirmed {
        let applied = name.rsplit_once('.').is_some_and(|(context, class)| {
            lock.recorded_agent_jurisdiction(plan.environment, &context.replace('.', "-"), class)
                .is_some()
        });
        if !changes.iter().any(|c| &c.class == name) && !applied {
            emsg!(
                "bynk: error: --confirm-jurisdiction {name} names no class whose jurisdiction this plan changes; refusing"
            );
            return false;
        }
    }
    let unconfirmed: Vec<&&PlanJurisdictionChange> = changes
        .iter()
        .filter(|c| !confirmed.iter().any(|name| name == &c.class))
        .collect();
    if unconfirmed.is_empty() {
        return true;
    }
    if !io::stdin().is_terminal() {
        emsg!(
            "bynk: error: this deploy changes the jurisdiction of a stored agent, which mints different Durable Object ids and orphans every existing instance; confirm each class with {}",
            unconfirmed
                .iter()
                .map(|c| format!("--confirm-jurisdiction {}", c.class))
                .collect::<Vec<_>>()
                .join(" ")
        );
        return false;
    }
    for change in unconfirmed {
        eprint!(
            "Changing `{}` from {} to {} orphans all of its existing instances, irreversibly. Type `{}` to confirm: ",
            change.class, change.from, change.to, change.class
        );
        let _ = io::stderr().flush();
        let mut answer = String::new();
        if io::stdin().read_line(&mut answer).is_err() || answer.trim() != change.class {
            emsg!(
                "bynk: error: `{}` not confirmed; nothing was deployed",
                change.class
            );
            return false;
        }
    }
    true
}

/// The project's Worker groups (#3562): which Worker directory a context is in,
/// and which contexts a Worker holds. What `deploy` keys the ledger by, so a
/// dependency's contract is looked up under the Worker that provides it.
pub(crate) struct GroupNames {
    layout: bynk_project::worker_layout::WorkerLayout,
}

impl GroupNames {
    pub(crate) fn of(project_root: &Path) -> Self {
        Self {
            layout: bynk_project::worker_layout::WorkerLayout::for_project(
                project_root,
                &std::collections::HashMap::new(),
            ),
        }
    }

    /// The directory of the Worker `context` is in: its own, or its group's.
    pub(crate) fn dir(&self, context: &str) -> String {
        self.layout.dir(context)
    }

    /// The Worker that holds the group member named `context`, dotted
    /// (`shop.a`) or dashed (`shop-a`) as `--context` takes it; `None` for a
    /// context on its own Worker, or no context.
    pub(crate) fn holding(&self, context: &str) -> Option<String> {
        self.layout.groups().find_map(|(dir, members)| {
            members
                .iter()
                .any(|m| m == context || m.replace('.', "-") == context)
                .then(|| dir.to_string())
        })
    }

    /// The name of `worker`'s script on Cloudflare: its group's `name` or key, or
    /// the Worker's own name for a context on its own.
    pub(crate) fn script(&self, worker: &str) -> String {
        self.layout
            .group_members(worker)
            .and_then(<[String]>::first)
            .map_or_else(
                || worker.to_string(),
                |first| self.layout.script_name(first),
            )
    }

    /// The contexts of the group whose directory is `worker`; none for a
    /// context's own Worker.
    pub(crate) fn members(&self, worker: &str) -> Vec<String> {
        self.layout
            .group_members(worker)
            .map(<[String]>::to_vec)
            .unwrap_or_default()
    }
}

/// Render the plan exactly as the user sees it. Pure, so the output surface the
/// deploy guide documents is goldened rather than described — `print_plan` is
/// the transport.
pub(crate) fn plan_report(plan: &Plan<'_>, format: DeployFormat) -> String {
    match format {
        DeployFormat::Short => {
            let mut out = String::new();
            // Before the per-context section, deliberately: an orphan is a
            // fact about the account regardless of what this run is about to
            // do, and a reader should see it before the noise of what's being
            // pushed (slice 5).
            // The Worker block sits in the list between Hyperdrive's orphans and the queues.
            short_kind_orphans(&plan.shown_orphans, &mut out, &mut |out| {
                for worker in &plan.shown_orphans.workers {
                    // #3562: a removed group's members, from the ledger.
                    let members = plan.shown_orphans.worker_contexts.get(worker);
                    match members {
                        Some(contexts) => out.push_str(&format!(
                            "orphan worker {worker} (contexts {})\n",
                            contexts.join(", ")
                        )),
                        None => out.push_str(&format!("orphan worker {worker}\n")),
                    }
                    // #3562 (G5): it is not empty: it still holds an agent's state.
                    if let Some(state) = plan.shown_orphans.worker_agent_state.get(worker) {
                        out.push_str(&agent_state_note(worker, state));
                    }
                }
            });
            for context in &plan.contexts {
                // Each kind's lines, with the datasets after the queues and the bindings and
                // Durable Object classes between the linked kinds and the containers.
                short_kind_lines(context, &mut out, &mut |slot, out| match slot {
                    Slot::Datasets => {
                        for dataset in &context.datasets {
                            out.push_str(&format!(
                                "dataset {} ({}; created on first write)\n",
                                dataset.dataset, dataset.binding
                            ));
                        }
                    }
                    Slot::BindingsAndObjects => {
                        // Interop I5 (#1972): bound, not provisioned — the Worker is
                        // not Bynk's, so the line says so rather than an action.
                        for service in &context.foreign_services {
                            out.push_str(&format!(
                                "service {} {} (foreign Worker; not provisioned)\n",
                                service.binding, service.worker
                            ));
                        }
                        // #3328: Email Routing rules, reconciled by the push.
                        for address in &context.email_addresses {
                            out.push_str(&format!(
                                "email address {address} (Email Routing rule; advisory — wrangler reconciles it)\n"
                            ));
                        }
                        if let Some(environment) = context.email_unclaimed {
                            out.push_str(&format!(
                                "email: none (no mapping for `{environment}` in [deploy.email]; the Worker claims no address, so production's Email Routing rules are left alone)\n"
                            ));
                        }
                        for address in &context.email_left_in_place {
                            out.push_str(&format!(
                                "email address {address} no longer accepted (its Email Routing rule is left in place and still points at this Worker; remove it in the dashboard)\n"
                            ));
                        }
                        for address in &context.removed_email_addresses {
                            out.push_str(&format!(
                                "email address {address} removed (its Email Routing rule is deleted by the push; needs --prune)\n"
                            ));
                        }
                        // Between the provisioning lines and the push, because that is
                        // where it happens: the `exports` declaration rides the config
                        // `wrangler deploy` reads rather than being a step of its own.
                        // Flagged advisory in place — a reader must not take it for a
                        // claim that the namespace does or doesn't exist yet (#1796,
                        // ADR 0194 D1's principle).
                        for object in &context.durable_objects {
                            // #3819: a pinned agent says where its ids are minted.
                            let pinned = object
                                .jurisdiction
                                .map(|j| format!(", jurisdiction {j}"))
                                .unwrap_or_default();
                            out.push_str(&format!(
                                "durable object {} ({}{pinned}; advisory — {} reconciles it)\n",
                                object.class, object.storage, object.reconciled_by
                            ));
                        }
                    }
                    // Before the queues, in the orphan report only.
                    Slot::WorkerOrphans => {}
                });
                // State-migrations S3 (#2248): the declared class renames and
                // deletions, stated for what they do to stored data.
                for rename in &context.renamed_durable_objects {
                    out.push_str(&if rename.already_applied {
                        format!(
                            "rename durable object {} → {} (already applied by an earlier deploy; the tombstone stays until Cloudflare reports it stale)\n",
                            rename.from, rename.to
                        )
                    } else {
                        format!(
                            "rename durable object {} → {} (moves its stored data)\n",
                            rename.from, rename.to
                        )
                    });
                }
                // #3819: a changed `@jurisdiction` mints different ids, so it strands the instances.
                for change in &context.jurisdiction_changes {
                    out.push_str(&format!(
                        "change jurisdiction of durable object {} ({} → {}; its existing instances are orphaned, irreversibly; needs --confirm-jurisdiction {})\n",
                        change.class, change.from, change.to, change.class
                    ));
                }
                for deletion in &context.deleted_durable_objects {
                    out.push_str(&if deletion.already_applied {
                        format!(
                            "delete durable object {} (already deleted by an earlier deploy; the tombstone stays until Cloudflare reports it stale)\n",
                            deletion.class
                        )
                    } else {
                        format!(
                            "delete durable object {} (all stored data; irreversible)\n",
                            deletion.class
                        )
                    });
                }
                // Before the lines it qualifies, not after: a reader who takes
                // the list for the whole story is the failure this increment
                // exists to prevent (ADR 0196 D2).
                if !context.secrets_complete {
                    out.push_str(&format!(
                        "secrets incomplete {} (reads at least one name the compiler cannot see)\n",
                        context.worker
                    ));
                }
                // Names only, never values (ADR 0195 D1). The origin rides each
                // line because the three are not equally known: `declared` is
                // required, `read` is advisory, `supplied` is the user's word.
                for secret in &context.secrets {
                    out.push_str(&format!(
                        "secret {} {} ({})\n",
                        secret.action,
                        secret.name,
                        secret.origin.label()
                    ));
                }
                out.push_str(&format!("{} {}\n", context.action, context.worker));
            }
            // The order is the plan's load-bearing claim once there is more
            // than one context, so state it rather than leaving it implied by
            // the line order above.
            if plan.order.len() > 1 {
                out.push_str(&format!("order {}\n", plan.order.join(" → ")));
            }
            // #3943: advisory, after the plan it qualifies.
            for shared in &plan.shared_with_default {
                out.push_str(&shared.line(plan.environment));
                out.push('\n');
            }
            out
        }
        DeployFormat::Json => {
            format!(
                "{}\n",
                serde_json::to_string_pretty(plan).expect("plan serialises")
            )
        }
    }
}

pub(in crate::deploy) fn print_plan(plan: &Plan<'_>, format: DeployFormat) {
    print!("{}", plan_report(plan, format));
}

/// What CI must not make: a resource whose id Cloudflare mints or the ledger
/// allocates, which a CI job that cannot commit `bynk.deploy.lock` would lose,
/// leaving an orphan nobody can find again. Printed, and `true` when there is
/// one. A queue's name comes from the source, so CI creating one loses nothing
/// (ADR 0194 D2), and a bucket's, database's or dataset's likewise.
///
/// - A KV namespace's id is minted (S4b, #1967: each derived instance's too).
/// - A rate limiter's id is allocated by the ledger (P3, #2342): one allocated
///   and never committed would be allocated again next run, possibly another
///   number, resetting the limiter's counters.
/// - A Hyperdrive config's id is minted (#3319); a team that deploys from CI
///   adopts one (`config_id`) instead.
pub(crate) fn unrecordable_in_ci(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
    hyperdrive: &hyperdrive::Wants,
) -> bool {
    if !is_ci() {
        return false;
    }
    if let Some(message) = kind_unrecordable_in_ci(&ProjectCx {
        ledger: LedgerView::of(lock, environment),
        resources,
        order,
    }) {
        emsg!("{message}");
        return true;
    }
    hyperdrive::refused_in_ci(order, resources, lock, environment, hyperdrive)
}

/// The first collision among the names the account scopes: a KV namespace's
/// title (a derived instance's joins two names with the separator Worker names
/// already use), an R2 bucket's, a D1 database's and an Analytics dataset's.
pub(super) fn account_name_problem(
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
) -> Option<String> {
    kind_name_problem(&ProjectCx {
        ledger: LedgerView::of(lock, environment),
        resources,
        order: &[],
    })
    .or_else(|| analytics_dataset_problem(resources, lock, environment))
}

impl<'a> Plan<'a> {
    /// #3494: the listings the account is asked for: one per kind (and R2
    /// jurisdiction) of which this plan would create something.
    pub(crate) fn account_listings(&self) -> BTreeSet<super::on_account::Listing> {
        let mut wanted = BTreeSet::new();
        for c in &self.contexts {
            kind_listings(c, &mut wanted);
        }
        wanted
    }

    /// #3494: this plan with what `on` says the account already holds: a
    /// resource it would create that is already there is `reuse`, since the
    /// deploy adopts it (R2, D1, Vectorize) or reuses it (a queue), and one it
    /// would only create if absent (a queue sent to) is `create` when it is not.
    /// A KV namespace is noted instead: Cloudflare refuses a second namespace of
    /// one title, so the deploy cannot create it.
    pub(crate) fn checked_against(mut self, on: &super::on_account::OnAccount) -> Self {
        self.account_notes.extend(on.notes.iter().cloned());
        for c in &mut self.contexts {
            check_kind_lines(c, on, &mut self.account_notes);
        }
        self
    }

    /// #3494: [`Plan::checked_against`] the account, when
    /// `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` say which and allow
    /// it to be read; a plan with nothing to create asks nothing. Without them
    /// the plan is the ledger's, and a note says so.
    pub(crate) fn checked_against_account(self) -> Self {
        let wanted = self.account_listings();
        if wanted.is_empty() {
            return self;
        }
        match super::on_account::RestListings::from_env() {
            Ok(listings) => {
                let on = super::on_account::OnAccount::read(&wanted, &listings);
                self.checked_against(&on)
            }
            Err(why) => {
                let mut plan = self;
                plan.account_notes.push(format!(
                    "bynk: note: `create` is what `bynk.deploy.lock` records, not checked against \
                     the account; {why}"
                ));
                plan
            }
        }
    }

    /// #3494: what checking the plan against the account found to say.
    pub(crate) fn account_notes(&self) -> &[String] {
        &self.account_notes
    }

    /// #3797: the refusals and warnings the run found so far, which the JSON plan lists.
    pub(crate) fn with_report(
        mut self,
        refusals: Vec<super::Entry>,
        warnings: Vec<super::Entry>,
    ) -> Self {
        self.refusals = refusals;
        self.warnings = warnings;
        self
    }

    /// #3797: one more refusal for the plan's `refusals`, for a refusal made once the plan is
    /// derived: the plan is still printed, with it in the document.
    pub(crate) fn with_refusal(mut self, refusal: super::Entry) -> Self {
        self.refusals.push(refusal);
        self
    }

    /// #3943: what the environment takes from `default`, listed after the plan.
    pub(crate) fn with_shared(mut self, shared: Vec<super::shared::SharedWithDefault>) -> Self {
        self.shared_with_default = shared;
        self
    }

    /// Each converted kind's lines again, with what the run has read in `lives`: for
    /// Hyperdrive (#3319), each config as `bynk.toml` and the environment say it will be handled
    /// (a `sync` from the variable, an `adopt`), once the deploy has read them.
    pub(crate) fn with_lives(
        mut self,
        order: &[String],
        resources: &'a BTreeMap<String, Resources>,
        lock: &DeployLock,
        lives: Lives<'_>,
    ) -> Self {
        for (worker, context) in order.iter().zip(self.contexts.iter_mut()) {
            let cx = PlanCx {
                ledger: LedgerView::of(lock, self.environment),
                worker,
            };
            fill_kind_lines(context, &resources[worker], &cx, lives);
        }
        self
    }
}

/// Derive the plan over the resolved order. Pure, so the per-context breakdown
/// and the ordering claim are unit-tested without a Cloudflare account.
///
/// Indexes `resources` rather than defending a miss: `order` ⊆ the workers it
/// was read for, and the deploy loop indexes the same map anyway — so a
/// tolerated miss here would only understate the plan a moment before the run
/// panicked on it regardless.
pub(crate) fn derive_plan<'a>(
    order: &'a [String],
    resources: &'a BTreeMap<String, Resources>,
    lock: &DeployLock,
    // Not borrowed into the plan: a `PlanSecret` owns its name, because the set
    // is derived (declared ∪ supplied) rather than taken from either source.
    secrets: &SecretSource,
    force: bool,
    environment: &'a str,
) -> Plan<'a> {
    let edges = project_edges(resources);
    let foreign_secrets: BTreeSet<String> = resources
        .values()
        .flat_map(|r| r.foreign_callers.iter().map(|f| f.binding.clone()))
        .collect();
    let orphans = find_orphans(lock, environment, resources);
    Plan {
        environment,
        refusals: Vec::new(),
        warnings: Vec::new(),
        account_notes: Vec::new(),
        shared_with_default: Vec::new(),
        shown_orphans: orphans.as_named_in(environment),
        orphans,
        order: order
            .iter()
            .map(|worker| deployed_worker_name(environment, worker))
            .collect(),
        contexts: order
            .iter()
            .map(|worker| {
                let declared = &resources[worker];
                let mut context = ContextPlan {
                    worker: deployed_worker_name(environment, worker),
                    kv: Vec::new(),
                    r2: Vec::new(),
                    vectorize: Vec::new(),
                    d1: Vec::new(),
                    hyperdrive: Vec::new(),
                    ratelimits: Vec::new(),
                    queues: Vec::new(),
                    datasets: declared
                        .analytics_datasets
                        .iter()
                        .map(|dataset| PlanDataset {
                            binding: &dataset.binding,
                            dataset: analytics_dataset_name(environment, &dataset.name),
                        })
                        .collect(),
                    stored_secrets: Vec::new(),
                    ai_search: Vec::new(),
                    mtls_certificates: Vec::new(),
                    durable_objects: declared
                        .durable_objects
                        .iter()
                        .map(|object| PlanDurableObject {
                            class: &object.class,
                            storage: &object.storage,
                            jurisdiction: object.jurisdiction.as_deref(),
                            reconciled_by: "Cloudflare",
                        })
                        .collect(),
                    containers: Vec::new(),
                    renamed_durable_objects: declared
                        .renamed_durable_objects
                        .iter()
                        .map(|(from, to)| PlanDurableObjectRename {
                            from,
                            to,
                            already_applied: lock.has_renamed_class(environment, worker, from, to),
                        })
                        .collect(),
                    jurisdiction_changes: jurisdiction_changes(
                        lock,
                        environment,
                        worker,
                        &declared.durable_objects,
                        &declared.renamed_durable_objects,
                    ),
                    deleted_durable_objects: declared
                        .deleted_durable_objects
                        .iter()
                        .map(|class| PlanDeletion {
                            class: qualified_class(worker, class),
                            already_applied: lock.has_deleted_class(environment, worker, class),
                        })
                        .collect(),
                    secrets: wanted_secrets(
                        &declared.declared_secrets,
                        &declared.read_secrets,
                        secrets,
                    )
                    .into_iter()
                    // I3 (#1971): a foreign caller's secret is never spread.
                    .filter(|w| !foreign_secrets.contains(&w.name))
                    // I2 (#1971): the key for each edge this Worker is on.
                    .chain(
                        edge_keys_of(worker, &edges)
                            .into_iter()
                            .map(|name| WantedSecret {
                                name,
                                origin: Origin::Managed,
                            }),
                    )
                    // I3 (#1971): and the secret of each foreign caller it
                    // admits, which `deploy` mints and hands off.
                    .chain(declared.foreign_callers.iter().map(|f| WantedSecret {
                        name: f.binding.clone(),
                        origin: Origin::Managed,
                    }))
                    // #3386: and each `SignedCookie` actor's signing key, which `deploy`
                    // mints when it is unset.
                    .chain(declared.session_keys.iter().map(|k| WantedSecret {
                        name: k.binding.clone(),
                        origin: Origin::Managed,
                    }))
                    .map(|want| PlanSecret {
                        name: want.name,
                        origin: want.origin,
                        // Presence is not knowable here — the plan runs
                        // before auth so `--dry-run` stays offline — so the
                        // action is what the run will attempt.
                        action: if force { "overwrite" } else { "set" },
                    })
                    .collect(),
                    secrets_complete: declared.reads_complete,
                    action: if lock.is_deployed(environment, worker) {
                        "redeploy"
                    } else {
                        "deploy"
                    },
                    binds_to: declared
                        .binds_to
                        .iter()
                        .map(|target| deployed_worker_name(environment, target))
                        .collect(),
                    foreign_services: declared
                        .foreign_services
                        .iter()
                        .map(|f| PlanForeignService {
                            binding: &f.binding,
                            worker: &f.worker,
                        })
                        .collect(),
                    email_addresses: declared
                        .email_addresses
                        .iter()
                        .map(String::as_str)
                        .collect(),
                    email_unclaimed: declared.email_unclaimed(environment).then_some(environment),
                    email_left_in_place: if declared.email_unclaimed(environment) {
                        lock.removed_email_addresses(environment, worker, &[])
                    } else {
                        Vec::new()
                    },
                    // #3883: an environment that claims none leaves `addresses` out, and
                    // Wrangler changes no rule for an omitted key, so there is nothing to prune.
                    removed_email_addresses: if declared.email_unclaimed(environment) {
                        Vec::new()
                    } else {
                        lock.removed_email_addresses(environment, worker, &declared.email_addresses)
                    },
                    steps: WorkerSteps {
                        worker: Some((worker, declared)),
                        ..WorkerSteps::default()
                    },
                };
                // 1D.1 (#3929): each converted kind's lines.
                let cx = PlanCx {
                    ledger: LedgerView::of(lock, environment),
                    worker,
                };
                fill_kind_lines(&mut context, declared, &cx, Lives::default());
                context
            })
            .collect(),
    }
}

/// P5 (#2323): why a plan with a `recreate` bucket may not proceed without
/// `--yes`, or `None`. The only thing between a changed `@jurisdiction` and a
/// silent move to a new, empty bucket, so `run` consults it before anything is
/// provisioned.
pub(crate) fn recreate_refusal(plan: &Plan<'_>, yes: bool) -> Option<&'static str> {
    (!yes && plan.contexts.iter().any(|c| c.r2.iter().any(|b| b.action == "recreate")))
        .then_some("a changed `@jurisdiction` makes a new R2 bucket and leaves the old one, with its objects, where it is (the plan reports it as an orphan); move the data, then re-run with --yes")
}

/// P3b (#2582): why the datasets the build declares would mix, or `None`.
/// The derived name flattens both a context's `.` and a capability's word
/// breaks to `_`, so `shop.cart`'s `Signups` and `shop`'s `CartSignups` are
/// both `shop_cart_signups`; and `_<env>` meets a name that ends the same way,
/// so `shop`'s `SignupsStaging` by default is `shop`'s `Signups` under
/// `--env staging`. Datasets have no ledger record, so the check is against
/// the build's own instances: in `environment`, and in every other
/// environment the ledger has deployed to.
pub(crate) fn analytics_dataset_problem(
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
) -> Option<String> {
    let mut seen: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for (worker, declared) in resources {
        for dataset in &declared.analytics_datasets {
            let name = analytics_dataset_name(environment, &dataset.name);
            if let Some((other_worker, other_binding)) =
                seen.insert(name.clone(), (worker, &dataset.binding))
            {
                return Some(format!(
                    "`{}` of `{worker}` and `{other_binding}` of `{other_worker}` would both write to the Analytics Engine dataset `{name}`; rename one capability",
                    dataset.binding
                ));
            }
        }
    }
    for other_env in lock
        .environments
        .keys()
        .filter(|e| e.as_str() != environment)
    {
        for (worker, declared) in resources {
            for dataset in &declared.analytics_datasets {
                let there = analytics_dataset_name(other_env, &dataset.name);
                if let Some((other_worker, other_binding)) = seen.get(&there)
                    && (*other_worker != worker.as_str() || *other_binding != dataset.binding)
                {
                    return Some(format!(
                        "`{other_binding}` of `{other_worker}` would write to the Analytics Engine dataset `{there}`, which `{}` of `{worker}` writes to in environment `{other_env}`; rename one capability",
                        dataset.binding
                    ));
                }
            }
        }
    }
    None
}

pub(crate) fn requires_interactive_confirmation(yes: bool, stdin_is_terminal: bool) -> bool {
    !yes && stdin_is_terminal
}

pub(crate) fn is_ci() -> bool {
    std::env::var_os("CI").is_some_and(|value| value != "false")
}

pub(super) fn confirm(yes: bool) -> bool {
    if yes {
        return true;
    }
    if !requires_interactive_confirmation(yes, io::stdin().is_terminal()) {
        emsg!("bynk: error: refusing to mutate in a non-interactive session without --yes");
        return false;
    }
    eprint!("Deploy to Cloudflare? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
