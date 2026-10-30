use super::*;

/// #1662: the diagnostic code a contract skew is reported under — a literal so
/// the registry test sees it, and `bynk explain` knows it.
const CONTRACT_SKEW: &str = "bynk.deploy.contract_skew";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeployFormat {
    #[default]
    Short,
    Json,
}

#[derive(Debug, Clone, Default)]
pub struct DeployOptions {
    pub dry_run: bool,
    pub format: DeployFormat,
    pub yes: bool,
    /// `--context NAME` — deploy this context alone, assuming the contexts it
    /// consumes are already live (slice 2, D4). Absent deploys the whole
    /// project in dependency order.
    pub context: Option<String>,
    /// `--env NAME` — slice 4. Selects the `bynk.deploy.lock` section; for any
    /// value other than `"default"` also drives synthesis of an environment-
    /// scoped Wrangler config section, since Cloudflare does not inherit
    /// bindings into a named environment (confirmed against Cloudflare's own
    /// docs — see #835). `DeployOptions::default()`'s empty string is a
    /// test-only artefact of `#[derive(Default)]`; every real invocation goes
    /// through the CLI, whose `default_value = "default"` guarantees this is
    /// never empty.
    pub environment: String,
    /// `--secrets-file` — a dotenv-style source of `NAME=value` pairs. Supplies
    /// **names and values** (slice 3, ADR 0195 D3).
    pub secrets_file: Option<std::path::PathBuf>,
    /// `--secret NAME` — a name whose *value* comes from the environment or a
    /// prompt. The environment is never scanned for names, so this is how a
    /// `bynk.Secrets` name reaches `deploy` without a file.
    pub secrets: Vec<String>,
    /// `--force` — overwrite a secret already set, rather than skipping it.
    pub force: bool,
    /// `--prune` (slice 5) — delete every reported KV/queue orphan, behind
    /// its own confirmation. Never deletes a Worker (DECISION C). Defaults
    /// to report-only, matching the track's "report or prevent, never
    /// silently share/destroy" posture (§6).
    pub prune: bool,
    /// State-migrations S3 (#2248): `--confirm-delete <context>.<Class>` —
    /// each Durable Object class whose data a `removed agent` deletes, named
    /// back. `--yes` never stands in for it (Decision E).
    pub confirm_delete: Vec<String>,
    /// #2554: `--accept-compatible-skew` — with `--context`, deploy a callee
    /// ahead of live callers still on an older contract when each such change
    /// is compatible for them. Records the older contracts in
    /// `bynk.contracts.lock` and rebuilds, so the callee answers them.
    pub accept_compatible_skew: bool,
    pub wrangler_args: Vec<String>,
}

/// The whole-project plan (slice 2). `order` is the upload order, dependencies
/// first; `contexts` carries it with each context's own actions. Slice 0's
/// single-worker `Plan` is the one-element case of this.
#[derive(Debug, Serialize)]
pub(crate) struct Plan<'a> {
    environment: &'a str,
    /// Slice 5: every ledger entry this environment no longer declares, by
    /// its ledger key, which is what `--prune` deletes by. Not serialised:
    /// the plan shows [`Plan::shown_orphans`].
    #[serde(skip)]
    orphans: Orphans,
    /// The same orphans as the account names them (#2028): a queue or Worker
    /// under its [`env_qualify`]d name. Printed before the per-context
    /// breakdown, so an orphan is seen before what's actually being pushed.
    #[serde(rename = "orphans")]
    shown_orphans: Orphans,
    /// The resolved upload order — the plan's headline, since Cloudflare
    /// rejects a Worker uploaded before its binding target. Each Worker by
    /// its [`deployed_worker_name`] (#2028).
    order: Vec<String>,
    contexts: Vec<ContextPlan<'a>>,
}

#[derive(Debug, Serialize)]
struct ContextPlan<'a> {
    /// The Worker this context deploys as: its [`deployed_worker_name`] in
    /// this environment, the name the push hands Wrangler (#2028).
    worker: String,
    /// One line per KV namespace the Worker binds: the bare `KV` first, then
    /// each derived instance in binding order (S4b, #1967). Empty for a
    /// context with no KV.
    kv: Vec<PlanKv<'a>>,
    /// P5 (#2323): one line per R2 bucket the Worker binds, in binding order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    r2: Vec<PlanR2<'a>>,
    /// P3 (#2342): one line per rate limiter the Worker binds, in binding
    /// order.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    ratelimits: Vec<PlanRateLimit<'a>>,
    /// One line per queue this context consumes or (P2, #2578) sends to, in
    /// name order.
    queues: Vec<PlanQueue>,
    /// P3b (#2582): one line per Analytics Engine dataset the Worker writes
    /// to, in binding order. No action: the platform creates a dataset on its
    /// first write, and Bynk never deletes one.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    datasets: Vec<PlanDataset<'a>>,
    /// #1796: one line per Durable Object class the push declares in
    /// `exports`, in class-name order. Empty for a context with no agent and
    /// no events fan-out.
    durable_objects: Vec<PlanDurableObject<'a>>,
    /// State-migrations S3 (#2248): each class an `agent New replaces Old`
    /// renames — the push moves its stored data. In old-name order.
    #[serde(
        rename = "durable_objects_renamed",
        skip_serializing_if = "Vec::is_empty"
    )]
    renamed_durable_objects: Vec<PlanDurableObjectRename<'a>>,
    /// State-migrations S3 (#2248): each class a `removed agent` deletes, as
    /// `<context>.<Class>` — the name `--confirm-delete` takes. Irreversible.
    #[serde(
        rename = "durable_objects_deleted",
        skip_serializing_if = "Vec::is_empty"
    )]
    deleted_durable_objects: Vec<PlanDeletion>,
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
    action: &'static str,
    /// The workers this one binds to, in the emitted config, each by its
    /// [`deployed_worker_name`] — the target the push's Service Binding names.
    binds_to: Vec<String>,
    /// Interop I5 (#1972): its Service Bindings to Workers outside the
    /// project, in binding order. Bound, never provisioned.
    foreign_services: Vec<PlanForeignService<'a>>,
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

#[derive(Debug, Serialize)]
struct PlanKv<'a> {
    /// `KV` for the bare `Kv`, `KV_<SNAKE>` for a derived instance.
    binding: &'a str,
    action: &'static str,
    /// The namespace's account-facing title ([`kv_title`]).
    namespace: String,
}

/// P3 (#2342): one rate limiter. Its `namespace_id` is the ledger's to
/// allocate, so the plan can only say whether one is recorded; the push
/// allocates and records the missing ones.
#[derive(Debug, Serialize)]
struct PlanRateLimit<'a> {
    /// `RATELIMIT_<SNAKE>`.
    binding: &'a str,
    /// `allocate` or `reuse`.
    action: &'static str,
    /// The recorded id, when `reuse`.
    #[serde(skip_serializing_if = "Option::is_none")]
    namespace_id: Option<String>,
}

/// P5 (#2323): one R2 bucket. The name is the identity, so there is no id to
/// mint and `reuse` is a plain statement of the ledger.
#[derive(Debug, Serialize)]
struct PlanR2<'a> {
    /// `R2_<SNAKE>`.
    binding: &'a str,
    /// `create`, `reuse`, or `recreate`: a changed `@jurisdiction` makes a new
    /// bucket, and the objects of the old one do not move (see [`r2_action`]).
    action: &'static str,
    /// The bucket's account-facing name, [`r2_bucket_name`]d.
    bucket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    jurisdiction: Option<&'a str>,
}

/// P3b (#2582): one Analytics Engine dataset.
#[derive(Debug, Serialize)]
struct PlanDataset<'a> {
    /// `ANALYTICS_<SNAKE>`.
    binding: &'a str,
    /// The dataset's account-facing name, [`analytics_dataset_name`]d.
    dataset: String,
}

#[derive(Debug, Serialize)]
struct PlanQueue {
    /// `create` when this project has never made the queue, `reuse` when the
    /// ledger has it. Either way the provision step attempts the create and
    /// treats an existing queue as success, so `reuse` is a forecast — "expect
    /// nothing new" — not a promise to stay silent (ADR 0194 D2).
    action: &'static str,
    /// The queue's account name, [`env_qualify`]d as the provision step
    /// creates it (#2028).
    queue: String,
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
struct PlanDurableObject<'a> {
    class: &'a str,
    /// The backend the config declares (`sqlite`), read from the file rather
    /// than assumed, so the plan and the upload can't disagree.
    storage: &'a str,
    /// Always `Cloudflare`, and that is the point: the field names an owner
    /// other than `bynk`, which is the whole content of the advisory (ADR 0194
    /// D1's principle). A consumer reading the plan learns that this line is
    /// not a claim that the namespace exists or doesn't, without having to
    /// know the ADR.
    reconciled_by: &'static str,
}

/// State-migrations S3 (#2248): one Durable Object class rename the push
/// applies through an `exports` tombstone. `already_applied` when an earlier
/// push in this environment carried it (#2276 Decision D): the data has moved,
/// and the declaration is waiting for Cloudflare's `stale_tombstone`.
#[derive(Debug, Serialize)]
struct PlanDurableObjectRename<'a> {
    from: &'a str,
    to: &'a str,
    already_applied: bool,
}

/// State-migrations S3 (#2248): one `deleted` tombstone the push carries.
/// `already_applied` when an earlier push in this environment carried it (the
/// ledger says so, #2253 review): the declaration is waiting for Cloudflare's
/// `stale_tombstone`, nothing more is destroyed, and no confirmation is asked.
#[derive(Debug, Serialize)]
struct PlanDeletion {
    class: String,
    already_applied: bool,
}

/// State-migrations S3 (#2248): a class's name as `--confirm-delete` takes it,
/// `<context>.<Class>`. A Worker's directory name is its context's qualified
/// name with `.` replaced by `-` (`worker_dir_name`), and an identifier holds
/// no `-`, so the context name is recovered exactly.
fn qualified_class(worker: &str, class: &str) -> String {
    format!("{}.{class}", worker.replace('-', "."))
}

/// The `exports` tombstones a successful push of `worker` carried, recorded
/// so later plans show them as applied: its deletions (State-migrations S3,
/// #2248, #2253 review) and its renames (#2276 Decision D). Each replaces what
/// was recorded for the Worker.
fn record_pushed_tombstones(
    lock: &mut DeployLock,
    environment: &str,
    worker: &str,
    pushed: &Resources,
) {
    lock.record_deleted_classes(
        environment,
        worker,
        pushed.deleted_durable_objects.iter().cloned().collect(),
    );
    lock.record_renamed_classes(
        environment,
        worker,
        pushed.renamed_durable_objects.iter().cloned().collect(),
    );
}

/// State-migrations S3 (#2248, Decision E): every class this plan deletes must
/// be confirmed by name — by `--confirm-delete`, or by typing it at an
/// interactive prompt. `--yes` never stands in, and a confirmation for a class
/// the plan does not delete is refused, so a stale CI flag cannot approve a
/// different deletion.
fn confirm_deletions(plan: &Plan<'_>, confirmed: &[String], lock: &DeployLock) -> bool {
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
            eprintln!("bynk: --confirm-delete {name} names no class this plan deletes; refusing");
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
        eprintln!(
            "bynk: this deploy deletes stored agent data; confirm each class with {}",
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
            eprintln!("bynk: `{name}` not confirmed; nothing was deployed");
            return false;
        }
    }
    true
}

/// The `--` passthrough argument that conflicts with the driver's own
/// `--env`, if any (slice 4, DECISION E) — bare or `=`-joined, mirroring
/// `dev.rs`'s `passthrough_has` matching rule for the same class of clash
/// (`--port`/`--inspector-port` there). Returns the matched literal, not just
/// a bool, so the error can name what it conflicts with. Pure, so the rule is
/// tested without touching `DeployOptions`.
///
/// `pub(crate)`: `dev.rs` reuses this for the identical clash between its own
/// `--env` (which environment's ledger section `--remote` reads) and a
/// `-- --env`/`-- --environment` passthrough to `wrangler dev` (which
/// environment Wrangler actually connects to) — the same "two explicit,
/// conflicting environment selections, one of them silent" shape, just
/// without a value `dev` forwards to wrangler itself.
pub(crate) fn conflicting_env_passthrough(wrangler_args: &[String]) -> Option<&str> {
    wrangler_args
        .iter()
        .find(|arg| {
            ["--env", "--environment"]
                .iter()
                .any(|flag| arg.as_str() == *flag || arg.starts_with(&format!("{flag}=")))
        })
        .map(String::as_str)
}

/// #2055: the refusal for a `--secrets-file` passed through after `--` when
/// the run includes a first deploy. That deploy uploads the Worker's secrets
/// with its own `wrangler deploy --secrets-file`, so the passthrough would be
/// a second, contradictory one. Matched bare or `=`-joined, as
/// [`conflicting_env_passthrough`] matches. `None` when there is no such
/// argument or every Worker in `order` has been deployed in `environment`
/// (a redeploy passes nothing of its own). Pure, so the wording is tested.
pub(crate) fn secrets_file_passthrough_refusal(
    wrangler_args: &[String],
    order: &[String],
    lock: &DeployLock,
    environment: &str,
) -> Option<String> {
    let arg = wrangler_args
        .iter()
        .find(|arg| arg.as_str() == "--secrets-file" || arg.starts_with("--secrets-file="))?;
    let first: Vec<String> = order
        .iter()
        .filter(|worker| !lock.is_deployed(environment, worker))
        .map(|worker| format!("`{}`", deployed_worker_name(environment, worker)))
        .collect();
    if first.is_empty() {
        return None;
    }
    Some(format!(
        "bynk: `{arg}` after `--` conflicts with the secrets file `bynk deploy` uploads with a new Worker's first version ({}).\n  \
         Pass it to bynk instead, before `--`: `bynk deploy --secrets-file <file>`.\n",
        first.join(", ")
    ))
}

/// Run the slice-0 single-context deployment pipeline.
pub fn run(
    tb: &dyn Toolbox,
    compiler: &Compiler,
    project_root: &Path,
    node_floor: u32,
    opts: &DeployOptions,
) -> ExitCode {
    // Slice 4 (DECISION E), and the first check of all: once `--env` is a
    // real, driver-curated concept, a conflicting `-- --env`/`-- --environment`
    // would otherwise reach `wrangler deploy` as a second, contradictory flag —
    // Wrangler's own last-wins parsing deciding silently which one actually
    // deploys, while the ledger records the driver's choice regardless. Reject
    // before any other work, rather than pick a winner between two explicit,
    // conflicting inputs.
    if let Some(conflict) = conflicting_env_passthrough(&opts.wrangler_args) {
        eprintln!(
            "bynk: `--env {}` conflicts with `{conflict}` after `--` — pass one or the other, not both",
            opts.environment
        );
        return ExitCode::FAILURE;
    }

    let preflight_opts = DoctorOptions {
        only: Some(Capability::Deploy),
        strict: false,
    };
    let report = doctor::diagnose(
        tb,
        compiler,
        &Context {
            project_root: Some(project_root.to_path_buf()),
            in_repo: false,
            node_floor,
        },
        &preflight_opts,
    );
    if report.exit_nonzero(&preflight_opts) {
        eprint!("{}", preflight_failure_message(&report));
        return ExitCode::FAILURE;
    }

    let build_dir = project_root.join(".bynk").join("deploy");
    if let Err(e) = workers::prepare_build_dir(project_root, &build_dir) {
        eprintln!("bynk: could not prepare build directory: {e}");
        return ExitCode::FAILURE;
    }
    if !workers::compile_once(compiler, project_root, &build_dir, true) {
        return ExitCode::FAILURE;
    }
    // Slice 2: every context, ordered — not the one context slice 0 demanded.
    let workers_dir = build_dir.join("workers");
    let available = workers::discover_workers(&workers_dir);
    let selected = match workers::select_contexts(&available, opts.context.as_slice()) {
        Ok(selected) => selected,
        Err(e) => {
            eprintln!("bynk: {e}");
            return ExitCode::FAILURE;
        }
    };
    // Read spans the *whole* project even under `--context`: D4 needs the
    // selected context's binding targets to check they are live, and they are
    // by definition outside the selection.
    let resources = match project_resources(&workers_dir, &available) {
        Ok(resources) => resources,
        Err(e) => {
            eprintln!("bynk: {e}");
            return ExitCode::FAILURE;
        }
    };
    let graph = service_graph(&resources);
    // Read before the plan: a malformed `--secrets-file` is the user's typo, and
    // it should surface as one now rather than as a missing-secret failure
    // partway through a run that has already pushed a Worker.
    let secret_source = match SecretSource::read(opts) {
        Ok(source) => source,
        Err(e) => {
            eprintln!("bynk: {e}");
            return ExitCode::FAILURE;
        }
    };
    let lock_path = project_root.join(LOCK_FILE);
    let mut lock = match read_lock(&lock_path) {
        Ok(lock) => lock,
        Err(e) => {
            eprintln!("bynk: could not read {}: {e}", lock_path.display());
            return ExitCode::FAILURE;
        }
    };

    // (D4) `--context` does not deploy a dependency closure. A binding to a
    // Worker that has never been pushed fails at upload, so say which one
    // rather than letting Cloudflare's own error carry it.
    if opts.context.is_some()
        && let [worker] = selected.as_slice()
    {
        let absent = absent_dependencies(worker, &graph, &lock, &opts.environment);
        if !absent.is_empty() {
            eprint!(
                "{}",
                absent_dependencies_refusal(&opts.environment, worker, &absent)
            );
            return ExitCode::FAILURE;
        }

        // v0.177 (#643): the other half of D4. The dependency exists — but does
        // it still provide the contract this worker was compiled against?
        // Without this, the push succeeds and production discovers the skew by
        // 409ing. `--context` is precisely the flag that makes this reachable.
        let mut manifest = match read_contracts_manifest(&workers_dir.join(worker)) {
            Ok(m) => m,
            Err(e) => {
                eprintln!(
                    "bynk: could not read `{worker}`'s {}: {e}",
                    bynk_emit::emitter::contracts::CONTRACTS_MANIFEST
                );
                return ExitCode::FAILURE;
            }
        };
        let mut skews = contract_skews(
            &manifest.expects,
            &lock,
            bynk_emit::project::worker_dir_name,
            &opts.environment,
        );
        classify_contract_skews(
            &mut skews,
            &manifest.expects_shapes,
            &lock,
            bynk_emit::project::worker_dir_name,
            &opts.environment,
        );
        if !skews.is_empty() {
            eprint!(
                "{}",
                contract_skew_refusal(&opts.environment, worker, &skews)
            );
            return ExitCode::FAILURE;
        }

        // #2077: and the callee half. Does this build still provide what every
        // *other* live Worker was compiled against? Without this, pushing a
        // callee whose contract changed answers each live caller 409.
        let mut callee_skews = callee_contract_skews(
            worker,
            &manifest.provides,
            &lock,
            bynk_emit::project::worker_dir_name,
            &opts.environment,
        );
        classify_callee_skews(
            &mut callee_skews,
            &manifest.provides_shapes,
            &lock,
            &opts.environment,
        );
        // #2554: a callee ahead of its callers may answer the older contract
        // they were compiled against, when the change is compatible for them.
        // The older contracts are recorded in `bynk.contracts.lock` and the
        // callee rebuilt to carry them.
        let mut recorded = false;
        if opts.accept_compatible_skew {
            let wanted =
                shapes_to_accept(&callee_skews, &manifest.accepts, &lock, &opts.environment);
            if !wanted.is_empty() && opts.dry_run {
                // A dry run writes nothing and builds nothing: it reports what
                // the real run would record, and treats it as accepted.
                for (_, service, hash, _) in &wanted {
                    manifest
                        .accepts
                        .entry(service.clone())
                        .or_default()
                        .push(hash.clone());
                }
            } else if !wanted.is_empty() {
                match record_accepted(project_root, &wanted) {
                    Ok(()) => recorded = true,
                    Err(e) => {
                        eprintln!(
                            "bynk: could not record the older contracts in {}: {e}",
                            bynk_driver::contracts_lock::lock_path(project_root).display()
                        );
                        return ExitCode::FAILURE;
                    }
                }
                // The build reads the lock, so the callee carries what was just
                // recorded.
                if !workers::compile_once(compiler, project_root, &build_dir, true) {
                    eprintln!(
                        "bynk: the older contracts were recorded in {}, but the rebuild failed; nothing was deployed. The entries are safe to keep (every build re-checks them), or revert the file.",
                        bynk_driver::contracts_lock::lock_path(project_root).display()
                    );
                    return ExitCode::FAILURE;
                }
                manifest = match read_contracts_manifest(&workers_dir.join(worker)) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!(
                            "bynk: could not read `{worker}`'s {}: {e}",
                            bynk_emit::emitter::contracts::CONTRACTS_MANIFEST
                        );
                        return ExitCode::FAILURE;
                    }
                };
                callee_skews = callee_contract_skews(
                    worker,
                    &manifest.provides,
                    &lock,
                    bynk_emit::project::worker_dir_name,
                    &opts.environment,
                );
                classify_callee_skews(
                    &mut callee_skews,
                    &manifest.provides_shapes,
                    &lock,
                    &opts.environment,
                );
            }
        }
        let callee_skews = if opts.accept_compatible_skew {
            let (accepted, refused) = partition_accepted(callee_skews, &manifest.accepts);
            if !accepted.is_empty() {
                eprint!(
                    "{}",
                    accepted_skew_notice(
                        &opts.environment,
                        worker,
                        &accepted,
                        recorded,
                        opts.dry_run
                    )
                );
            }
            refused
        } else {
            callee_skews
        };
        if !callee_skews.is_empty() {
            // A caller this build no longer has gets its own remedy: a
            // whole-project deploy would not move it.
            let (built, orphaned) = split_orphaned_callers(callee_skews, &graph);
            if !built.is_empty() {
                eprint!(
                    "{}",
                    callee_contract_skew_refusal(&opts.environment, worker, &built)
                );
            }
            if !orphaned.is_empty() {
                eprint!(
                    "{}",
                    orphaned_caller_skew_refusal(&opts.environment, worker, &orphaned)
                );
            }
            return ExitCode::FAILURE;
        }
        // #2554: older contracts the lock still accepts that no live caller
        // was compiled against any more are noise; say so.
        if let Ok(existing) = bynk_driver::contracts_lock::read(project_root)
            && let Ok(contracts) =
                bynk_project::contracts_lock::parse(existing.as_deref(), project_root)
        {
            let unused = unused_accepted(&contracts, worker, &lock, &opts.environment);
            if !unused.is_empty() {
                eprint!("{}", unused_accepted_notice(&unused));
            }
        }
        for caller in unrecorded_callers(worker, &graph, &lock, &opts.environment) {
            eprint!(
                "{}",
                unrecorded_caller_warning(&opts.environment, worker, &caller)
            );
        }
    }

    let order = match deploy_order(&selected, &graph) {
        Ok(order) => order
            .into_iter()
            // A whole-project run orders every worker; `--context` orders only
            // the selection, but the DFS reaches its (already-live) targets —
            // drop them, D4 having already checked them.
            .filter(|worker| selected.contains(worker))
            .collect::<Vec<_>>(),
        Err(e) => {
            eprintln!("bynk: {e}");
            let OrderError::Cycle(path) = &e;
            let publishers = cycle_publishers(path, &resources);
            if !publishers.is_empty() {
                eprintln!(
                    "  The cycle runs through event delivery: {} {} events, so {} to each subscriber, and a subscriber on this cycle also calls back into {} services.",
                    publishers
                        .iter()
                        .map(|p| format!("`{p}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    if publishers.len() == 1 {
                        "publishes"
                    } else {
                        "publish"
                    },
                    if publishers.len() == 1 {
                        "it binds"
                    } else {
                        "each binds"
                    },
                    if publishers.len() == 1 {
                        "its"
                    } else {
                        "their"
                    },
                );
                eprintln!(
                    "  A subscriber that calls its publisher cannot be deployed yet; move the services it calls out of the publishing context."
                );
            }
            return ExitCode::FAILURE;
        }
    };

    // I4 (#1970, track §11 Q6): a declared foreign caller's pin. Refused
    // before the plan, `--dry-run` included, as a Bynk caller's skew is: the
    // foreign caller's client would be refused on every call.
    let pins = match read_pins(project_root) {
        Ok(pins) => pins,
        Err(e) => {
            eprintln!("bynk: {e}");
            return ExitCode::FAILURE;
        }
    };
    let mut built_foreign: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>> =
        BTreeMap::new();
    for worker in &order {
        match read_contracts_manifest(&workers_dir.join(worker)) {
            Ok(m) if !m.foreign.is_empty() => {
                built_foreign.insert(context_of(worker), m.foreign);
            }
            Ok(_) => {}
            Err(e) => {
                eprintln!(
                    "bynk: could not read `{worker}`'s {}: {e}",
                    bynk_emit::emitter::contracts::CONTRACTS_MANIFEST
                );
                return ExitCode::FAILURE;
            }
        }
    }
    let in_run: BTreeSet<String> = order.iter().map(|w| context_of(w)).collect();
    let (broken, unpinned) = check_pins(&pins, &built_foreign, &in_run, opts.context.is_none());
    if !broken.is_empty() {
        eprint!("{}", refusal_message(&broken));
        return ExitCode::FAILURE;
    }
    if !unpinned.is_empty() {
        eprint!("{}", unpinned_message(&unpinned));
    }

    // Review of #1974: namespace titles are account-wide, and a derived
    // instance's joins two names with the separator Worker names already use,
    // so two can coincide. Refused before the plan, `--dry-run` included,
    // rather than met as wrangler's own error partway through a deploy.
    if let Some(message) = kv_title_collision(&resources, &lock, &opts.environment) {
        eprintln!("bynk: {message}");
        return ExitCode::FAILURE;
    }
    // P5 (#2323): likewise a bucket's derived name, which R2 scopes to the
    // account and holds to its own character rules.
    if let Some(message) = r2_bucket_problem(&resources, &lock, &opts.environment) {
        eprintln!("bynk: {message}");
        return ExitCode::FAILURE;
    }
    // #2055: a first deploy hands `wrangler deploy` Bynk's own
    // `--secrets-file`, so a passthrough one would be a second, with
    // wrangler deciding which wins. Refused before the plan, as `--env`'s
    // clash is.
    if let Some(refusal) =
        secrets_file_passthrough_refusal(&opts.wrangler_args, &order, &lock, &opts.environment)
    {
        eprint!("{refusal}");
        return ExitCode::FAILURE;
    }

    let plan = derive_plan(
        &order,
        &resources,
        &lock,
        &secret_source,
        opts.force,
        &opts.environment,
    );
    print_plan(&plan, opts.format);
    if opts.dry_run {
        return ExitCode::SUCCESS;
    }
    // P5 (#2323): a changed `@jurisdiction` is a new bucket beside the old,
    // whose objects do not move, so it is only made on an explicit `--yes`.
    if let Some(refusal) = recreate_refusal(&plan, opts.yes) {
        eprintln!("bynk: {refusal}");
        return ExitCode::FAILURE;
    }

    // The CI gate is KV's alone, deliberately. A namespace id is *minted* by
    // Cloudflare, so a CI job that creates one and cannot commit the result
    // leaves an orphan nobody can find again. A queue's name comes from the
    // source, so CI creating one loses nothing: the next run derives the same
    // name and finds the same queue (ADR 0194 D2).
    // Per namespace (S4b, #1967): each derived instance's id is minted too.
    if is_ci()
        && let Some((worker, binding)) =
            first_unrecorded_kv(&order, &resources, &lock, &opts.environment)
    {
        eprintln!(
            "bynk: {} is unrecorded; provision locally first and commit {LOCK_FILE}",
            kv_namespace_noun(worker, binding)
        );
        return ExitCode::FAILURE;
    }
    // P3 (#2342): a rate limiter's id is allocated by the ledger, so one
    // allocated in CI and never committed would be allocated again next run —
    // possibly another number, resetting the limiter's counters.
    if is_ci()
        && let Some((worker, binding)) =
            first_unrecorded_ratelimit(&order, &resources, &lock, &opts.environment)
    {
        eprintln!(
            "bynk: the rate limiter `{binding}` of `{worker}` is unrecorded; deploy locally first and commit {LOCK_FILE}"
        );
        return ExitCode::FAILURE;
    }
    let probe = probe::detect(
        tb,
        "wrangler",
        DetectOpts {
            project_root: Some(project_root),
            allow_npx: true,
        },
    );
    // #1796: before authenticating, so the refusal costs no network call.
    if let Some(refusal) = wrangler_floor_refusal(&probe, &order, &resources) {
        eprintln!("{refusal}");
        return ExitCode::FAILURE;
    }
    if !whoami(&probe.provenance) {
        eprintln!(
            "bynk: Cloudflare authentication is unavailable; run `wrangler login` or set CLOUDFLARE_API_TOKEN"
        );
        return ExitCode::FAILURE;
    }
    // Interop I5 (#1972, track §11 Q8): a foreign Worker a Service Binding
    // names is not Bynk's to create, so a missing one is a warning, never a
    // refusal from the driver. Asked once per Worker, before the
    // confirmation, so the user decides with it in view.
    for warning in foreign_worker_warnings(&order, &resources, |name| {
        worker_presence(&probe.provenance, name, project_root)
    }) {
        eprintln!("{warning}");
    }
    // State-migrations S3 (#2248): a deletion is confirmed by name, apart
    // from — and before — the ordinary confirmation.
    if !confirm_deletions(&plan, &opts.confirm_delete, &lock) {
        return ExitCode::FAILURE;
    }
    if !confirm(opts.yes) {
        return ExitCode::FAILURE;
    }

    // Slice 5, DECISION B: once per run, not once per context — an
    // account-wide list, fetched only when it could actually change an
    // outcome (a context needs KV and already has a recorded id worth
    // checking; a first deploy has nothing to check drift against). `None`
    // (no wrangler, or the call failed) falls back to trusting the ledger
    // unconditionally, exactly as every deploy before this slice did.
    let live_kv_ids = if order.iter().any(|w| {
        kv_bindings(&resources[w])
            .any(|b| recorded_kv_binding(&lock, w, b, &opts.environment).is_some())
    }) {
        live_kv_namespace_ids(&probe.provenance, project_root)
    } else {
        None
    };

    // Provision → wire → push, per context, in dependency order. Each context's
    // state is written to the ledger as it lands (ADR 0180's incremental
    // posture), so an interrupted multi-context run is resumable rather than
    // restartable — and never rolled back (D2): a half-deployed project is a
    // real state the next plan will show, not an error to unwind.
    // Shared across the loop: a queue two contexts consume is one queue, so it
    // wants one create attempt per run, not one per consumer (ADR 0194 D2).
    let mut attempted_queues = BTreeSet::new();
    // I2 (#1971): each edge's key is decided over both its ends at once, so
    // each Worker's `secret list` is asked here, before anything is pushed,
    // and the per-context step reuses the answer. That covers every Worker
    // this run deploys and the far end of each edge it touches, which a
    // `--context` run does not deploy. Asked before the KV ids are
    // materialised and any `[env.<name>]` table is synthesised: wrangler loads
    // that config (a placeholder id included) and addresses the same Worker
    // either way, since the `name` the synthesised table sets (#2028) is the
    // one Wrangler's `--env` default gives a table without one.
    let in_run: BTreeSet<&str> = order.iter().map(String::as_str).collect();
    let edges: Vec<Edge> = project_edges(&resources)
        .into_iter()
        .filter(|e| in_run.contains(e.caller.as_str()) || in_run.contains(e.callee.as_str()))
        .collect();
    let mut asked: BTreeSet<&str> = in_run.clone();
    for e in &edges {
        asked.insert(&e.caller);
        asked.insert(&e.callee);
    }
    let listed: BTreeMap<String, Option<BTreeSet<String>>> = asked
        .iter()
        .map(|worker| {
            let answer = list_secrets(
                &probe.provenance,
                &workers_dir.join(worker),
                &opts.environment,
            );
            (worker.to_string(), answer)
        })
        .collect();
    let mut edge_keys: BTreeMap<Edge, EdgeKey> = BTreeMap::new();
    for edge in &edges {
        let ends = match edge.ends() {
            Ok(ends) => ends,
            Err(e) => {
                eprintln!("bynk: {e}");
                return ExitCode::FAILURE;
            }
        };
        let [callee, caller] = ends.map(|(worker, key)| {
            let p = presence_of(
                &key,
                listed[worker].as_ref(),
                !lock.is_deployed(&opts.environment, worker),
            );
            // #2004 review: name the Workers whose answer could not be read,
            // since the decision counts them as holding the key.
            if p == Presence::Unknown {
                eprint!(
                    "{}",
                    unknown_edge_key_notice(&opts.environment, worker, &key, edge)
                );
            }
            p
        });
        let both_in_run =
            in_run.contains(edge.caller.as_str()) && in_run.contains(edge.callee.as_str());
        match decide_edge_key(
            edge,
            caller,
            callee,
            both_in_run,
            opts.force,
            &mut mint_edge_key,
            &opts.environment,
        ) {
            Ok(decision) => {
                edge_keys.insert(edge.clone(), decision);
            }
            Err(e) => {
                eprintln!("bynk: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    // I3 (#1971): each foreign caller's secret, on the context in the run that
    // admits it. Only that end is ours, so a `--context` run decides it too.
    let foreign_secrets: BTreeSet<String> = resources
        .values()
        .flat_map(|r| r.foreign_callers.iter().map(|f| f.binding.clone()))
        .collect();
    let mut foreign_keys: BTreeMap<(String, String), ForeignKey> = BTreeMap::new();
    for worker in &order {
        for caller in &resources[worker].foreign_callers {
            let presence = presence_of(
                &caller.binding,
                listed[worker].as_ref(),
                !lock.is_deployed(&opts.environment, worker),
            );
            let supplied = value_from_source(&caller.binding, &secret_source);
            match decide_foreign_key(supplied, presence, opts.force, &mut mint_edge_key) {
                Ok(decision) => {
                    foreign_keys.insert((worker.clone(), caller.binding.clone()), decision);
                }
                Err(e) => {
                    eprint!(
                        "{}",
                        foreign_binding_refusal(&opts.environment, worker, caller, &e)
                    );
                    return ExitCode::FAILURE;
                }
            }
        }
    }
    // Shared across the loop so two contexts wanting the same secret prompt
    // once. Dropped with the run — nothing here is ever written (ADR 0195 D1).
    let mut resolved_secrets = BTreeMap::new();
    // #2055: a first deploy's secrets file that a SIGKILLed run left behind
    // goes before this run might write one of its own.
    let stale_removed = sweep_stale_upload_dirs(&std::env::temp_dir());
    if stale_removed > 0 {
        eprintln!(
            "bynk: removed {stale_removed} secrets file(s) an interrupted deploy left in the temp directory"
        );
    }
    for (i, worker) in order.iter().enumerate() {
        if order.len() > 1 {
            eprint!(
                "{}",
                deploying_notice(&opts.environment, worker, i + 1, order.len())
            );
        }
        match deploy_one(
            &probe.provenance,
            project_root,
            &workers_dir,
            worker,
            &resources[worker],
            &resources,
            &mut lock,
            &lock_path,
            &mut attempted_queues,
            &mut Secrets {
                source: &secret_source,
                force: opts.force,
                edge_keys: &edge_keys,
                foreign_keys: &foreign_keys,
                foreign_secrets: &foreign_secrets,
                listed: &listed,
                resolved: &mut resolved_secrets,
                with_upload: secrets_file_supported(&probe),
            },
            &opts.wrangler_args,
            &opts.environment,
            live_kv_ids.as_ref(),
        ) {
            Ok(Pushed::Ok) => {
                // v0.177 (#643): record what this Worker now *provides*, so a
                // later `--context` push of one of its callers can be refused
                // before it ships a caller that would 409.
                // `Some` even when empty: this build *knows* what the Worker
                // provides, and "knows it provides nothing" must not read as
                // "no record" at the next gate.
                let provided = read_contracts_manifest(&workers_dir.join(worker))
                    .map(|m| Some(m.provides))
                    .unwrap_or(None);
                lock.record_deployed(&opts.environment, worker, provided);
                record_pushed_tombstones(&mut lock, &opts.environment, worker, &resources[worker]);
                // P3 (#2342): the pushed Worker no longer binds a rate limiter
                // its source dropped, so that id is free again.
                lock.forget_undeclared_ratelimits(
                    &opts.environment,
                    worker,
                    &resources[worker].ratelimits,
                );
                // #2077: and what it was compiled against, so a later
                // `--context` push of one of its callees can be refused
                // before it ships a callee this Worker would 409 against.
                if let Ok(m) = read_contracts_manifest(&workers_dir.join(worker)) {
                    lock.record_expects(&opts.environment, worker, m.expects);
                    // #2485: and the structure behind both, so the next gate
                    // can say whether a skew is compatible.
                    lock.record_shapes(
                        &opts.environment,
                        worker,
                        m.provides_shapes,
                        m.expects_shapes,
                    );
                }
                // I4 (#1970): and what its declared foreign callers may call
                // it at, so the ledger knows the consumers no manifest of
                // their own records.
                lock.record_foreign(
                    &opts.environment,
                    worker,
                    read_contracts_manifest(&workers_dir.join(worker))
                        .map(|m| m.foreign)
                        .unwrap_or_default(),
                );
                // I3 (#1971): the secrets just minted for this context's
                // foreign callers, handed to their operators.
                for caller in &resources[worker].foreign_callers {
                    if let Some(ForeignKey::Set {
                        value,
                        minted: true,
                    }) = foreign_keys.get(&(worker.clone(), caller.binding.clone()))
                    {
                        eprint!(
                            "{}",
                            foreign_hand_off(&opts.environment, worker, caller, value)
                        );
                    }
                }
                // #1883, slice 1b: and the static files it now serves.
                lock.record_assets(
                    &opts.environment,
                    worker,
                    served_assets(&workers_dir.join(worker)),
                );
                if let Err(e) = write_lock(&lock_path, &lock) {
                    eprint!(
                        "{}",
                        unrecorded_deploy_failure(
                            &opts.environment,
                            worker,
                            &lock_path.display().to_string(),
                            &e.to_string()
                        )
                    );
                    return ExitCode::FAILURE;
                }
            }
            // A shared Ctrl-C. Stop, but report nothing and exit cleanly: the
            // user asked for this, and the terminal signalled us too. `worker`
            // is deliberately *not* recorded as deployed — the push was cut
            // short, so whether it landed is unknown, and the ledger only ever
            // claims what it watched succeed.
            Ok(Pushed::Interrupted) => return ExitCode::SUCCESS,
            Err(f) => {
                eprintln!("bynk: {}", f.message);
                // Stop rather than push on: everything left in the order either
                // binds to what just failed or would be uploaded into a
                // topology that is not what the plan described. `worker` itself
                // is excluded — the line above already named it as the failure,
                // and listing it here again as "not deployed" would double-count
                // it against the number.
                eprint!("{}", stopped_report(&opts.environment, &order[i + 1..]));
                // #2004 review: a run that re-keyed an edge and stopped between
                // its two ends leaves them refusing each other. The next plain
                // run sees a key on each end and keeps both, so say how to
                // repair it.
                if let Some(hint) =
                    interrupted_rotation_hint(&opts.environment, &edge_keys, &order[..i])
                {
                    eprint!("{hint}");
                }
                // I3 (#1971): a secret minted for this context's foreign
                // caller may already be set, and the next run would keep it
                // without ever showing it, so hand it off now.
                for caller in &resources[worker].foreign_callers {
                    if let Some(ForeignKey::Set {
                        value,
                        minted: true,
                    }) = foreign_keys.get(&(worker.clone(), caller.binding.clone()))
                    {
                        eprint!(
                            "{}",
                            foreign_binding_maybe_set_notice(&opts.environment, worker, caller)
                        );
                        eprint!(
                            "{}",
                            foreign_hand_off(&opts.environment, worker, caller, value)
                        );
                    }
                }
                // Wrangler's own code, not a flat 1 (slice 0's contract).
                return ExitCode::from(f.code);
            }
        }
    }

    // Slice 5: pruning is project-wide, independent of `--context` — the
    // orphan report already is (DECISION A), so pruning follows it. Runs
    // only after every selected context has pushed cleanly: a failed deploy
    // above already returned, so a mid-flight ledger never reaches this.
    if opts.prune && plan.orphans.has_prunable() {
        if !confirm_prune(opts.yes, &plan.orphans, &opts.environment) {
            return ExitCode::FAILURE;
        }
        if let Err(e) = prune_orphans(
            &probe.provenance,
            project_root,
            &mut lock,
            &lock_path,
            &opts.environment,
            &plan.orphans,
        ) {
            eprintln!("bynk: {}", e.message);
            return ExitCode::from(e.code);
        }
    }
    ExitCode::SUCCESS
}

/// Interop I5 (#1972): a warning for each foreign Worker that the pushed
/// contexts bind to and `presence` does not find, each Worker asked once,
/// in name order. Cloudflare resolves a Service Binding at upload, so a push
/// binding a missing Worker is rejected there: the warning says so ahead of
/// it, and the driver itself refuses nothing (a Worker created between now
/// and the push is fine).
pub(crate) fn foreign_worker_warnings(
    order: &[String],
    resources: &BTreeMap<String, Resources>,
    mut presence: impl FnMut(&str) -> WorkerPresence,
) -> Vec<String> {
    let mut bound: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for worker in order {
        for f in &resources[worker].foreign_services {
            bound.entry(f.worker.as_str()).or_default().push(worker);
        }
    }
    let mut out = Vec::new();
    for (name, by) in bound {
        let by = by
            .iter()
            .map(|w| format!("`{w}`"))
            .collect::<Vec<_>>()
            .join(", ");
        match presence(name) {
            WorkerPresence::Present => {}
            WorkerPresence::Missing => out.push(format!(
                "bynk: warning: {by} binds to Worker `{name}`, which this account does not have — \
                 Cloudflare rejects a Service Binding to a missing Worker at upload, so deploy `{name}` first."
            )),
            WorkerPresence::Unknown(said) => out.push(format!(
                "bynk: warning: could not check that Worker `{name}` (bound by {by}) exists: {said}"
            )),
        }
    }
    out
}

/// #2004 review, I2 (#1971): what to tell the user when a run that re-keyed
/// edges stopped before every end of them was deployed. `completed` is the
/// Workers that finished (not the one that failed, which may or may not hold
/// its new keys). An edge whose ends had no key before cannot be split: the
/// next run finds an end without one and re-keys it. One that was rotating
/// can: each end may hold a different key, and a plain rerun keeps both.
/// `None` when no such edge was left incomplete.
pub(crate) fn interrupted_rotation_hint(
    environment: &str,
    edge_keys: &BTreeMap<Edge, EdgeKey>,
    completed: &[String],
) -> Option<String> {
    let done = |w: &String| completed.contains(w);
    let split: Vec<String> = edge_keys
        .iter()
        .filter(|(edge, key)| {
            matches!(key, EdgeKey::Set { rotating: true, .. })
                && !(done(&edge.caller) && done(&edge.callee))
        })
        .map(|(edge, _)| edge.describe(environment))
        .collect();
    if split.is_empty() {
        return None;
    }
    Some(format!(
        "bynk: this run was setting new keys on {} and stopped before both ends of each were deployed, so each end may hold a different key and calls along it are refused.\n  \
         Rerun `{} --force` (the whole project) to set one key on both ends; a plain rerun would keep both.\n",
        split.join(", "),
        deploy_command(environment)
    ))
}

/// #2485: the reasons behind one skew's verdict, indented under its line.
fn push_compat(out: &mut String, compat: Option<&Compatibility>) {
    if let Some(c) = compat {
        for line in c.lines() {
            out.push_str(&format!("      {line}\n"));
        }
    }
}

/// Which side of a call a refusal is for.
#[derive(Clone, Copy)]
enum SkewSide {
    /// The pushed Worker is the caller, ahead of its live callee.
    Caller,
    /// The pushed Worker is the callee, ahead of or behind its live callers.
    Callee,
}

/// #2485, #2554: when some skew is only a compatible change, say what that
/// means for this side.
///
/// A callee can be deployed ahead of its callers that stay compatible: it is
/// built to answer the older contract too (`--accept-compatible-skew`). A
/// caller ahead of its live callee cannot: the live callee refuses a hash it
/// has never heard of, whatever the verdict, so the callee goes first.
fn push_compat_note<'a>(
    out: &mut String,
    verdicts: impl Iterator<Item = Option<&'a Compatibility>>,
    side: SkewSide,
    dependencies: &[&str],
) {
    use bynk_check::contract_compat::Verdict;
    let compatible = verdicts
        .into_iter()
        .any(|c| c.is_some_and(|c| c.verdict == Verdict::Compatible));
    if !compatible {
        return;
    }
    match side {
        SkewSide::Caller => {
            let mut first: Vec<&str> = dependencies.to_vec();
            first.sort_unstable();
            first.dedup();
            out.push_str(
                "  A compatible change is still refused here: the live callee refuses a contract it was not built to accept, so the callee goes first.\n",
            );
            out.push_str(&format!(
                "  Deploy {} ahead of this caller with `--accept-compatible-skew`.\n",
                first
                    .iter()
                    .map(|d| format!("`{d}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        SkewSide::Callee => out.push_str(
            "  A compatible change can be accepted: `--accept-compatible-skew` records the older contract in bynk.contracts.lock, and this callee then answers its live callers until they are redeployed.\n",
        ),
    }
}

/// #2554: the callee-ahead skews the pushed build answers: compatible for the
/// caller, and the caller's recorded hash is one the build accepts
/// (`bynk-contracts.json`'s `accepts`). Returns `(accepted, refused)`.
pub(crate) fn partition_accepted(
    skews: Vec<CalleeSkew>,
    accepts: &BTreeMap<String, Vec<String>>,
) -> (Vec<CalleeSkew>, Vec<CalleeSkew>) {
    skews
        .into_iter()
        .partition(|s| skew_is_accepted(s, accepts))
}

fn skew_is_accepted(s: &CalleeSkew, accepts: &BTreeMap<String, Vec<String>>) -> bool {
    use bynk_check::contract_compat::Verdict;
    s.compat
        .as_ref()
        .is_some_and(|c| c.verdict == Verdict::Compatible)
        && accepts
            .get(&s.service)
            .is_some_and(|hashes| hashes.contains(&s.expected))
}

/// #2554: the older contracts `bynk.contracts.lock` must gain for the pushed
/// build to answer its compatible live callers: `(context, service, hash,
/// shape text)`, one per distinct contract. A skew with no verdict, a breaking
/// one, and one the build already accepts add nothing.
pub(crate) fn shapes_to_accept(
    skews: &[CalleeSkew],
    accepts: &BTreeMap<String, Vec<String>>,
    lock: &DeployLock,
    environment: &str,
) -> Vec<(String, String, String, String)> {
    use bynk_check::contract_compat::Verdict;
    let mut out: Vec<(String, String, String, String)> = Vec::new();
    for s in skews {
        let compatible = s
            .compat
            .as_ref()
            .is_some_and(|c| c.verdict == Verdict::Compatible);
        if !compatible || skew_is_accepted(s, accepts) {
            continue;
        }
        let Some(shape) = lock.expected_shape(environment, &s.caller, &s.dependency, &s.service)
        else {
            continue;
        };
        let entry = (
            s.dependency.clone(),
            s.service.clone(),
            s.expected.clone(),
            shape.to_text(),
        );
        if !out.contains(&entry) {
            out.push(entry);
        }
    }
    out
}

/// #2554: add `wanted` (from [`shapes_to_accept`]) to `bynk.contracts.lock`.
fn record_accepted(
    project_root: &Path,
    wanted: &[(String, String, String, String)],
) -> Result<(), String> {
    let existing = bynk_driver::contracts_lock::read(project_root).map_err(|e| e.to_string())?;
    let mut lock = bynk_project::contracts_lock::parse(existing.as_deref(), project_root)?;
    for (context, service, _, shape) in wanted {
        lock.accept(context, service, shape);
    }
    bynk_driver::contracts_lock::write(
        project_root,
        &bynk_project::contracts_lock::serialize(&lock),
    )
    .map_err(|e| e.to_string())
}

/// #2554: the lock entries for `worker`'s context that no live caller in
/// `environment` was compiled against: `(context, service, hash)`.
pub(crate) fn unused_accepted(
    contracts: &bynk_project::contracts_lock::ContractsLock,
    worker: &str,
    lock: &DeployLock,
    environment: &str,
) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for (context, services) in contracts.contexts() {
        if bynk_emit::project::worker_dir_name(context) != worker {
            continue;
        }
        for (service, shapes) in services {
            for text in shapes {
                let Some(shape) = ServiceShape::from_text(text) else {
                    continue;
                };
                let hash = shape.hash();
                let used = lock.live_expects(environment).any(|(caller, expects)| {
                    caller != worker
                        && expects
                            .and_then(|e| e.get(context))
                            .and_then(|m| m.get(service))
                            == Some(&hash)
                });
                if !used {
                    out.push((context.clone(), service.clone(), hash));
                }
            }
        }
    }
    out
}

/// #2554: tell the author which accepted contracts nothing live needs.
pub(crate) fn unused_accepted_notice(unused: &[(String, String, String)]) -> String {
    let mut out = format!(
        "bynk: bynk.contracts.lock accepts {} older contract{} that no live caller was compiled against:\n",
        unused.len(),
        if unused.len() == 1 { "" } else { "s" }
    );
    for (context, service, hash) in unused {
        out.push_str(&format!("  {context}.{service} — {hash}\n"));
    }
    out.push_str("  Remove them from the lock when you no longer need them.\n");
    out
}

/// #2554: said when the pushed callee will answer live callers still on an
/// older, compatible contract.
pub(crate) fn accepted_skew_notice(
    environment: &str,
    worker: &str,
    accepted: &[CalleeSkew],
    recorded: bool,
    dry_run: bool,
) -> String {
    let mut out = format!(
        "bynk: `{}` will also answer {} live caller contract{} that {} compatible:\n",
        deployed_worker_name(environment, worker),
        accepted.len(),
        if accepted.len() == 1 { "" } else { "s" },
        if accepted.len() == 1 { "is" } else { "are" },
    );
    for s in accepted {
        out.push_str(&format!(
            "  `{}` calls {}.{} — compiled against {}\n",
            deployed_worker_name(environment, &s.caller),
            s.dependency,
            s.service,
            s.expected,
        ));
        push_compat(&mut out, s.compat.as_ref());
    }
    if dry_run {
        out.push_str(
            "  A real run would record these in bynk.contracts.lock and rebuild the callee to carry them. Nothing was written.\n",
        );
    } else if recorded {
        out.push_str(
            "  Recorded in bynk.contracts.lock: commit it. The callee answers these callers until they are redeployed; remove an entry once no live caller was compiled against it.\n",
        );
    }
    out
}

/// v0.177 (#643): the `--context` refusal for a Worker compiled against a
/// contract its live dependencies no longer provide. The Worker by its
/// [`deployed_worker_name`], the remedy repeating the run's `--env` (#2047).
/// Each skew names a contract (`<context>.<service>`), not a Worker.
pub(crate) fn contract_skew_refusal(
    environment: &str,
    worker: &str,
    skews: &[ContractSkew],
) -> String {
    let mut out = format!(
        "bynk: `{}` was compiled against a contract its live dependencies no longer provide ({CONTRACT_SKEW}):\n",
        deployed_worker_name(environment, worker)
    );
    for s in skews {
        out.push_str(&format!(
            "  {}.{} — compiled against {}, live is {}\n",
            s.dependency, s.service, s.expected, s.live
        ));
        push_compat(&mut out, s.compat.as_ref());
    }
    push_compat_note(
        &mut out,
        skews.iter().map(|s| s.compat.as_ref()),
        SkewSide::Caller,
        &skews
            .iter()
            .map(|s| s.dependency.as_str())
            .collect::<Vec<_>>(),
    );
    out.push_str(
        "  Deploying this would ship a caller its callee rejects (409 ContractMismatch) on every call.\n",
    );
    out.push_str(&format!(
        "  Deploy the whole project (`{}`) so both sides move together.\n",
        deploy_command(environment)
    ));
    out
}

/// #2004 review: an end of `edge` whose secrets could not be listed, counted
/// as holding `key`. The Worker and the edge by their deployed names, the
/// remedy repeating the run's `--env` (#2047).
pub(crate) fn unknown_edge_key_notice(
    environment: &str,
    worker: &str,
    key: &str,
    edge: &Edge,
) -> String {
    format!(
        "bynk: could not ask Cloudflare whether `{}` holds {key}; assuming it does. If it does not, the edge {} refuses every call — `{} --force` sets a fresh key on every edge.\n",
        deployed_worker_name(environment, worker),
        edge.describe(environment),
        deploy_command(environment)
    )
}

/// The progress line of a multi-context run: `n` of `total`, the Worker by
/// its [`deployed_worker_name`] (#2047).
pub(crate) fn deploying_notice(environment: &str, worker: &str, n: usize, total: usize) -> String {
    format!(
        "bynk: deploying `{}` ({n}/{total})…\n",
        deployed_worker_name(environment, worker)
    )
}

/// A push that landed but could not be written to the ledger at `lock_path`.
/// The Worker by its [`deployed_worker_name`] (#2047).
pub(crate) fn unrecorded_deploy_failure(
    environment: &str,
    worker: &str,
    lock_path: &str,
    error: &str,
) -> String {
    format!(
        "bynk: deployed `{}` but could not record it in {lock_path}: {error}\n",
        deployed_worker_name(environment, worker)
    )
}

/// I3 (#1971): the run could not decide `caller`'s binding on `worker`
/// (`reason`, from `decide_foreign_key`). The Worker by its
/// [`deployed_worker_name`] (#2047).
pub(crate) fn foreign_binding_refusal(
    environment: &str,
    worker: &str,
    caller: &ForeignCaller,
    reason: &str,
) -> String {
    format!(
        "bynk: `{}` (the foreign caller `{}` of `{}`): {reason}\n",
        caller.binding,
        caller.actor,
        deployed_worker_name(environment, worker)
    )
}

/// I3 (#1971): a failed push may have set a secret minted for `caller`. The
/// Worker by its [`deployed_worker_name`], which the suggested `wrangler
/// secret list` names too, so it asks about the right Worker (#2047).
pub(crate) fn foreign_binding_maybe_set_notice(
    environment: &str,
    worker: &str,
    caller: &ForeignCaller,
) -> String {
    let worker = deployed_worker_name(environment, worker);
    format!(
        "bynk: `{worker}` may already hold the {} minted for its foreign caller `{}`. If `wrangler secret list --name {worker}` shows it, hand this off; if not, the next deploy mints another.\n",
        caller.binding, caller.actor
    )
}

/// #1796: the refusal a deploy gets when a context it pushes declares a
/// Durable Object class and the wrangler that would push it is older than
/// [`bynk_emit::WRANGLER_MIN`].
///
/// Below that floor wrangler doesn't read the `exports` map the emitter
/// declares every class in, so the push would carry Durable Object bindings
/// with no namespace behind them and fail with Cloudflare's own error, which
/// says nothing about the version. `doctor`'s wrangler row only *warns* about
/// the floor, because a project with no agent still deploys on an older
/// wrangler. This is where the project is known, so this is where the floor
/// becomes a hard requirement. `None` when nothing pushed declares a class, or
/// when the version can't be judged without running npx
/// ([`doctor::wrangler_below_min`]'s own rule).
pub(crate) fn wrangler_floor_refusal(
    probe: &probe::Probe,
    order: &[String],
    resources: &BTreeMap<String, Resources>,
) -> Option<String> {
    let version = probe
        .version
        .filter(|_| doctor::wrangler_below_min(probe))?;
    let worker = order
        .iter()
        .find(|worker| !resources[*worker].durable_objects.is_empty())?;
    Some(format!(
        "bynk: wrangler {version} is older than {}, the first that reads the Durable Object \
         `exports` `{worker}` declares; the push would fail. Upgrade with `{}`.",
        bynk_emit::WRANGLER_MIN,
        doctor::wrangler_upgrade_remedy(probe)
    ))
}

pub fn preflight_failure_message(report: &Report) -> String {
    format!(
        "bynk: environment not ready for `deploy` — see below.\n\n{}",
        report::render(report, Format::Human)
    )
}

/// The D4 refusal: `--context` selected `worker`, which binds to `absent`
/// Workers the ledger has never recorded as deployed in `environment`.
///
/// Every Worker is named by its [`deployed_worker_name`], as the plan names it
/// (#2038): under `--env staging` the missing target is `payment-staging`, not
/// the logical `payment`. Pure, so the wording is goldened rather than
/// described.
pub(crate) fn absent_dependencies_refusal(
    environment: &str,
    worker: &str,
    absent: &[String],
) -> String {
    format!(
        "bynk: `{}` binds to {}, which {} never been deployed — a Service Binding to a Worker that does not exist fails at upload.\n  Deploy the whole project once (`{}`) to bring the topology up.\n",
        deployed_worker_name(environment, worker),
        absent
            .iter()
            .map(|a| format!("`{}`", deployed_worker_name(environment, a)))
            .collect::<Vec<_>>()
            .join(", "),
        if absent.len() == 1 { "has" } else { "have" },
        deploy_command(environment)
    )
}

/// #2077: the `--context` refusal for a callee whose new build no longer
/// provides the contract its live callers were compiled against — the callee
/// half of [`contract_skew_refusal`], under the same code. Each live caller and
/// the callee by their [`deployed_worker_name`]; each finding names the
/// contract (`<context>.<service>`) and both hashes; the remedy repeats the
/// run's `--env` ([`deploy_command`]). Pure, so the wording is goldened.
pub(crate) fn callee_contract_skew_refusal(
    environment: &str,
    worker: &str,
    skews: &[CalleeSkew],
) -> String {
    let mut out = format!(
        "bynk: `{}` no longer provides the contract its live callers were compiled against ({CONTRACT_SKEW}):\n",
        deployed_worker_name(environment, worker)
    );
    for s in skews {
        out.push_str(&format!(
            "  `{}` calls {}.{} — compiled against {}, this build provides {}\n",
            deployed_worker_name(environment, &s.caller),
            s.dependency,
            s.service,
            s.expected,
            s.provided
        ));
        push_compat(&mut out, s.compat.as_ref());
    }
    push_compat_note(
        &mut out,
        skews.iter().map(|s| s.compat.as_ref()),
        SkewSide::Callee,
        &[],
    );
    out.push_str(
        "  Deploying this would ship a callee that rejects its live callers (409 ContractMismatch) on every call.\n",
    );
    out.push_str(&format!(
        "  Deploy the whole project (`{}`) so both sides move together.\n",
        deploy_command(environment)
    ));
    out
}

/// #2077 review: the refusal for a callee whose new build breaks a live caller
/// this project no longer builds — a Worker of a removed context, still
/// recorded in the ledger (the plan's `orphan worker`). A whole-project deploy
/// neither redeploys nor checks it, and `--prune` never deletes a Worker or its
/// record, so the remedy is to retire it (delete the Worker, then drop its
/// ledger entry, without which this gate keeps refusing) or to restore its
/// context and deploy the whole project ([`deploy_command`]). Pure, so the
/// wording is goldened.
pub(crate) fn orphaned_caller_skew_refusal(
    environment: &str,
    worker: &str,
    skews: &[CalleeSkew],
) -> String {
    let mut out = format!(
        "bynk: `{}` no longer provides the contract a live Worker this project no longer builds was compiled against ({CONTRACT_SKEW}):\n",
        deployed_worker_name(environment, worker)
    );
    for s in skews {
        out.push_str(&format!(
            "  `{}` (orphaned) calls {}.{} — compiled against {}, this build provides {}\n",
            deployed_worker_name(environment, &s.caller),
            s.dependency,
            s.service,
            s.expected,
            s.provided
        ));
        push_compat(&mut out, s.compat.as_ref());
    }
    push_compat_note(
        &mut out,
        skews.iter().map(|s| s.compat.as_ref()),
        SkewSide::Callee,
        &[],
    );
    out.push_str(
        "  Deploying this would ship a callee that rejects it (409 ContractMismatch) on every call, and a whole-project deploy would not move it: that reports an orphaned Worker but never redeploys or deletes it, and `--prune` never deletes a Worker either.\n",
    );
    let mut callers: Vec<&str> = skews.iter().map(|s| s.caller.as_str()).collect();
    callers.dedup();
    out.push_str(&format!(
        "  If it is unused, delete it ({}) and remove its entry ({}) from {LOCK_FILE}. If it is still needed, restore its context and deploy the whole project (`{}`).\n",
        callers
            .iter()
            .map(|c| format!(
                "`npx wrangler delete --name {}`",
                deployed_worker_name(environment, c)
            ))
            .collect::<Vec<_>>()
            .join(", "),
        callers
            .iter()
            .map(|c| format!("`[environments.{environment}.workers.{c}]`"))
            .collect::<Vec<_>>()
            .join(", "),
        deploy_command(environment)
    ));
    out
}

/// #2077: the warning for a live caller of the pushed `worker` whose ledger
/// record predates recorded `expects`, so the callee gate cannot check it.
/// Never a refusal: silence is not a match.
pub(crate) fn unrecorded_caller_warning(environment: &str, worker: &str, caller: &str) -> String {
    format!(
        "bynk: warning: could not check that `{}` still provides what the live `{}` was compiled against — its deploy record predates recorded expectations ({CONTRACT_SKEW}). A whole-project deploy (`{}`) records them.\n",
        deployed_worker_name(environment, worker),
        deployed_worker_name(environment, caller),
        deploy_command(environment)
    )
}

/// What the run did **not** get to, once a context failed. `rest` is the order
/// *beyond* the failure, so the last context failing reports nothing — there was
/// nothing left to withhold, and the failure itself has already been named.
/// Each context is named by its [`deployed_worker_name`] in `environment`, as
/// the plan's `order` line names it (#2038).
///
/// Pure, so the wording — and the count's agreement with the list — is goldened
/// rather than described.
pub(crate) fn stopped_report(environment: &str, rest: &[String]) -> String {
    if rest.is_empty() {
        return String::new();
    }
    format!(
        "bynk: stopping — {} not deployed: {}. Re-run `{}` to resume; what already landed is kept.\n",
        if rest.len() == 1 {
            "1 more context was".to_string()
        } else {
            format!("{} further contexts were", rest.len())
        },
        rest.iter()
            .map(|worker| deployed_worker_name(environment, worker))
            .collect::<Vec<_>>()
            .join(", "),
        deploy_command(environment)
    )
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
            for kv in &plan.shown_orphans.kv {
                match &kv.binding {
                    None => out.push_str(&format!("orphan kv {}\n", kv.worker)),
                    Some(binding) => {
                        out.push_str(&format!("orphan kv {} {binding}\n", kv.worker));
                    }
                }
            }
            for r2 in &plan.shown_orphans.r2 {
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
            for worker in &plan.shown_orphans.workers {
                out.push_str(&format!("orphan worker {worker}\n"));
            }
            for queue in &plan.shown_orphans.queues {
                out.push_str(&format!("orphan queue {queue}\n"));
            }
            for context in &plan.contexts {
                for kv in &context.kv {
                    // The bare line is unchanged; an instance names its binding.
                    if is_bare_kv(kv.binding) {
                        out.push_str(&format!("kv {} {}\n", kv.action, kv.namespace));
                    } else {
                        out.push_str(&format!(
                            "kv {} {} ({})\n",
                            kv.action, kv.namespace, kv.binding
                        ));
                    }
                }
                for r2 in &context.r2 {
                    out.push_str(&format!("r2 {} {} ({})", r2.action, r2.bucket, r2.binding));
                    if let Some(jurisdiction) = r2.jurisdiction {
                        out.push_str(&format!(" in jurisdiction {jurisdiction}"));
                    }
                    if r2.action == "recreate" {
                        out.push_str(" — the old bucket and its objects stay where they are");
                    }
                    out.push('\n');
                }
                for limiter in &context.ratelimits {
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
                for queue in &context.queues {
                    out.push_str(&format!("queue {} {}\n", queue.action, queue.queue));
                }
                for dataset in &context.datasets {
                    out.push_str(&format!(
                        "dataset {} ({}; created on first write)\n",
                        dataset.dataset, dataset.binding
                    ));
                }
                // Interop I5 (#1972): bound, not provisioned — the Worker is
                // not Bynk's, so the line says so rather than an action.
                for service in &context.foreign_services {
                    out.push_str(&format!(
                        "service {} {} (foreign Worker; not provisioned)\n",
                        service.binding, service.worker
                    ));
                }
                // Between the provisioning lines and the push, because that is
                // where it happens: the `exports` declaration rides the config
                // `wrangler deploy` reads rather than being a step of its own.
                // Flagged advisory in place — a reader must not take it for a
                // claim that the namespace does or doesn't exist yet (#1796,
                // ADR 0194 D1's principle).
                for object in &context.durable_objects {
                    out.push_str(&format!(
                        "durable object {} ({}; advisory — {} reconciles it)\n",
                        object.class, object.storage, object.reconciled_by
                    ));
                }
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

fn print_plan(plan: &Plan<'_>, format: DeployFormat) {
    print!("{}", plan_report(plan, format));
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
                ContextPlan {
                    worker: deployed_worker_name(environment, worker),
                    kv: kv_bindings(declared)
                        .map(|binding| PlanKv {
                            binding,
                            action: if recorded_kv_binding(lock, worker, binding, environment)
                                .is_some()
                            {
                                "reuse"
                            } else {
                                "create"
                            },
                            namespace: kv_title(worker, binding, environment),
                        })
                        .collect(),
                    r2: declared
                        .r2_buckets
                        .iter()
                        .map(|bucket| PlanR2 {
                            binding: &bucket.binding,
                            action: r2_action(lock, worker, bucket, environment),
                            bucket: r2_bucket_name(environment, &bucket.name),
                            jurisdiction: bucket.jurisdiction.as_deref(),
                        })
                        .collect(),
                    ratelimits: declared
                        .ratelimits
                        .iter()
                        .map(|binding| {
                            let recorded = recorded_ratelimit(lock, worker, binding, environment);
                            PlanRateLimit {
                                binding,
                                action: if recorded.is_some() {
                                    "reuse"
                                } else {
                                    "allocate"
                                },
                                namespace_id: recorded.map(str::to_string),
                            }
                        })
                        .collect(),
                    // P2 (#2578): the queues it sends to, with those it
                    // consumes. An unrecorded consumed queue is `create`, a
                    // forecast that the first deploy's record corrects. A
                    // queue only sent to is recorded only if Bynk made it, so
                    // an adopted one would read `create` forever: it is
                    // `create or reuse` until a record says otherwise.
                    queues: declared
                        .all_queues()
                        .iter()
                        .map(|queue| PlanQueue {
                            action: if lock.has_queue(environment, queue) {
                                "reuse"
                            } else if declared.queues.contains(queue) {
                                "create"
                            } else {
                                "create or reuse"
                            },
                            queue: env_qualify(environment, queue),
                        })
                        .collect(),
                    datasets: declared
                        .analytics_datasets
                        .iter()
                        .map(|dataset| PlanDataset {
                            binding: &dataset.binding,
                            dataset: analytics_dataset_name(environment, &dataset.name),
                        })
                        .collect(),
                    durable_objects: declared
                        .durable_objects
                        .iter()
                        .map(|object| PlanDurableObject {
                            class: &object.class,
                            storage: &object.storage,
                            reconciled_by: "Cloudflare",
                        })
                        .collect(),
                    renamed_durable_objects: declared
                        .renamed_durable_objects
                        .iter()
                        .map(|(from, to)| PlanDurableObjectRename {
                            from,
                            to,
                            already_applied: lock.has_renamed_class(environment, worker, from, to),
                        })
                        .collect(),
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
                }
            })
            .collect(),
    }
}

/// Two KV namespaces that would share one account-facing title
/// ([`kv_title`]), described for the user. `None` when there is no clash.
///
/// Cloudflare refuses a namespace whose title the account already holds, in
/// any environment ("Returns `400` if the account already owns a namespace
/// with that title"), so there are two checks:
///
/// - **Within `environment`**, every namespace the project declares: Worker
///   `shop` with `capability CartSessions` and Worker `shop-cart` with
///   `capability Sessions` both give `shop-cart-sessions`.
/// - **Across environments** (#2048), each namespace this deploy would create
///   (declared, with no id recorded for `environment`) against every other
///   environment the ledger knows: the titles of the namespaces it records,
///   and the titles its next deploy of this source would create. Worker
///   `shop`'s bare namespace under `--env staging` and its `capability
///   Staging` in the default environment are both `shop-staging`. A namespace
///   already recorded here is not created, so it cannot fail; that clash is
///   refused by the other environment's deploy instead. A recorded namespace
///   whose id has gone from the account is created again, and that is only
///   known once the deploy fetches the live ids, so `deploy_one` runs
///   [`cross_environment_title_clash`] again at that point (#2059).
///
/// Checked across the whole project, not just `--context`'s selection, since
/// titles are account-wide. The ledger records ids, not titles, so a recorded
/// namespace counts under the title Bynk would give it now; one made under
/// `--env` before #2037 is really titled with the bare Worker name.
pub(crate) fn kv_title_collision(
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
) -> Option<String> {
    let mut seen: BTreeMap<String, (&str, &str)> = BTreeMap::new();
    for (worker, declared) in resources {
        for binding in kv_bindings(declared) {
            let title = kv_title(worker, binding, environment);
            if let Some((first_worker, first_binding)) =
                seen.insert(title.clone(), (worker.as_str(), binding))
            {
                return Some(format!(
                    "{} and {} would both be titled `{title}` on the account; rename a capability or context so they differ",
                    kv_namespace_noun(first_worker, first_binding),
                    kv_namespace_noun(worker, binding)
                ));
            }
        }
    }

    // What this deploy creates: each declared namespace with no recorded id.
    let creates: Vec<(&str, &str)> = resources
        .iter()
        .flat_map(|(worker, declared)| {
            kv_bindings(declared).map(move |binding| (worker.as_str(), binding))
        })
        .filter(|(worker, binding)| {
            recorded_kv_binding(lock, worker, binding, environment).is_none()
        })
        .collect();
    cross_environment_title_clash(resources, lock, environment, &creates, None)
}

/// P5 (#2323): why a plan with a `recreate` bucket may not proceed without
/// `--yes`, or `None`. The only thing between a changed `@jurisdiction` and a
/// silent move to a new, empty bucket, so `run` consults it before anything is
/// provisioned.
pub(crate) fn recreate_refusal(plan: &Plan<'_>, yes: bool) -> Option<&'static str> {
    (!yes && plan.contexts.iter().any(|c| c.r2.iter().any(|b| b.action == "recreate")))
        .then_some("a changed `@jurisdiction` makes a new R2 bucket and leaves the old one, with its objects, where it is (the plan reports it as an orphan); move the data, then re-run with --yes")
}

/// P5 (#2323): what a deploy will do about `worker`'s R2 bucket `bucket` in
/// `environment`. The name is the identity, so a recorded bucket under the
/// same jurisdiction is `reuse`; one recorded under another is `recreate`,
/// since a bucket's jurisdiction is fixed when it is made.
pub(crate) fn r2_action(
    lock: &DeployLock,
    worker: &str,
    bucket: &R2Decl,
    environment: &str,
) -> &'static str {
    match recorded_r2(lock, worker, &bucket.binding, environment) {
        None => "create",
        Some(recorded)
            if recorded.name == r2_bucket_name(environment, &bucket.name)
                && recorded.jurisdiction == bucket.jurisdiction =>
        {
            "reuse"
        }
        Some(recorded) if recorded.name == r2_bucket_name(environment, &bucket.name) => "recreate",
        Some(_) => "create",
    }
}

/// P5 (#2323): why the buckets the build declares cannot all be made in
/// `environment`, or `None`. A derived name that breaks R2's rules is an error
/// at plan time, naming the instance, never a failed push; and two instances
/// (or one here and one another environment's) must not share a name, since
/// R2 scopes bucket names to the account.
pub(crate) fn r2_bucket_problem(
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
) -> Option<String> {
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
            for (other_env, env) in &lock.environments {
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

/// The cross-environment half of [`kv_title_collision`]: does any of
/// `creates`, the `(worker, binding)` namespaces about to be created under
/// `environment`, take a title another environment the ledger knows holds or
/// would create? The message names both namespaces and both environments.
///
/// Also run by `deploy_one` at the moment it decides to create (#2059), since
/// that is the only point that knows a recorded id has gone from the account
/// and its namespace is about to be created again. There `live_kv_ids` is the
/// account's live ids, when fetched: another environment's recorded namespace
/// whose id is not among them no longer holds its title (though one this
/// source declares there still counts, since that deploy would create it).
/// `None` counts every recorded namespace, which is always the case at plan
/// time: [`kv_title_collision`] runs before the live ids are fetched, so the
/// relaxation applies only to the deploy-time re-check.
pub(crate) fn cross_environment_title_clash(
    resources: &BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
    creates: &[(&str, &str)],
    live_kv_ids: Option<&BTreeSet<String>>,
) -> Option<String> {
    if creates.is_empty() {
        return None;
    }
    let is_live = |id: &str| live_kv_ids.is_none_or(|live| live.contains(id));
    let declared = || {
        resources.iter().flat_map(|(worker, declared)| {
            kv_bindings(declared).map(move |binding| (worker.as_str(), binding))
        })
    };
    for (other, recorded) in &lock.environments {
        if other == environment {
            continue;
        }
        // Each title `other` holds or would create, and whose it is: what its
        // ledger records first, then what this source declares.
        let held = recorded
            .kv
            .iter()
            .filter(|(_, kv)| is_live(&kv.id))
            .map(|(worker, _)| {
                (
                    worker.as_str(),
                    bynk_emit::emitter::wrangler::KV_BINDING_NAME,
                )
            })
            .chain(
                recorded
                    .kv_instances
                    .iter()
                    .flat_map(|(worker, instances)| {
                        instances
                            .iter()
                            .filter(|(_, kv)| is_live(&kv.id))
                            .map(move |(binding, _)| (worker.as_str(), binding.as_str()))
                    }),
            );
        let mut taken: BTreeMap<String, (&str, &str)> = BTreeMap::new();
        for (worker, binding) in held.chain(declared()) {
            taken
                .entry(kv_title(worker, binding, other))
                .or_insert((worker, binding));
        }
        for &(worker, binding) in creates {
            let title = kv_title(worker, binding, environment);
            if let Some(&(other_worker, other_binding)) = taken.get(&title) {
                return Some(format!(
                    "{} in {} and {} in {} would both be titled `{title}` on the account, which holds one namespace per title; rename a capability or context so they differ",
                    kv_namespace_noun(worker, binding),
                    environment_label(environment),
                    kv_namespace_noun(other_worker, other_binding),
                    environment_label(other),
                ));
            }
        }
    }
    None
}

/// How a message names an environment: `the default environment`, or
/// `` `--env staging` ``.
fn environment_label(environment: &str) -> String {
    if environment == "default" {
        "the default environment".to_string()
    } else {
        format!("`--env {environment}`")
    }
}

/// The first KV namespace in `order` with no recorded id, as `(worker,
/// binding)`: what CI refuses to mint, since an id created there and never
/// committed is an orphan nobody can find again. Every namespace counts, the
/// bare `KV` and each derived instance alike (S4b, #1967).
fn first_unrecorded_kv<'a>(
    order: &'a [String],
    resources: &'a BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
) -> Option<(&'a str, &'a str)> {
    order.iter().find_map(|worker| {
        kv_bindings(&resources[worker])
            .find(|binding| recorded_kv_binding(lock, worker, binding, environment).is_none())
            .map(|binding| (worker.as_str(), binding))
    })
}

/// P3 (#2342): the first rate limiter in `order` with no recorded
/// `namespace_id`, as `(worker, binding)`: what CI refuses to allocate.
fn first_unrecorded_ratelimit<'a>(
    order: &'a [String],
    resources: &'a BTreeMap<String, Resources>,
    lock: &DeployLock,
    environment: &str,
) -> Option<(&'a str, &'a str)> {
    order.iter().find_map(|worker| {
        resources[worker]
            .ratelimits
            .iter()
            .find(|binding| recorded_ratelimit(lock, worker, binding, environment).is_none())
            .map(|binding| (worker.as_str(), binding.as_str()))
    })
}

pub(crate) fn requires_interactive_confirmation(yes: bool, stdin_is_terminal: bool) -> bool {
    !yes && stdin_is_terminal
}

fn is_ci() -> bool {
    std::env::var_os("CI").is_some_and(|value| value != "false")
}

fn confirm(yes: bool) -> bool {
    if yes {
        return true;
    }
    if !requires_interactive_confirmation(yes, io::stdin().is_terminal()) {
        eprintln!("bynk: refusing to mutate in a non-interactive session without --yes");
        return false;
    }
    eprint!("Deploy to Cloudflare? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::deploy::config::tests::project;
    use crate::deploy::graph::tests::graph;
    use crate::deploy::ledger::tests::{lock_with_deployed, with_kv, with_queue};

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// `derive_plan` with no secret input and no `--force` — the shape every
    /// test that predates slice 3 wants, so those tests keep saying what they
    /// are about rather than restating two arguments they do not exercise.
    pub(crate) fn plan_of<'a>(
        order: &'a [String],
        resources: &'a BTreeMap<String, Resources>,
        lock: &DeployLock,
    ) -> Plan<'a> {
        derive_plan(
            order,
            resources,
            lock,
            &SecretSource::default(),
            false,
            "default",
        )
    }

    // ---- P5 (#2323): R2 buckets ----

    // ---- P3 (#2342): rate limiters ----

    #[test]
    fn the_plan_allocates_each_rate_limiter_then_reuses_its_id() {
        let declared = project(vec![(
            "shop",
            Resources::default().ratelimits(&["RATELIMIT_LOGIN"]),
        )]);
        let order = names(&["shop"]);
        let fresh = plan_of(&order, &declared, &DeployLock::default());
        let report = plan_report(&fresh, DeployFormat::Short);
        assert!(
            report.contains("ratelimit allocate RATELIMIT_LOGIN\n"),
            "{report}"
        );
        assert_eq!(
            first_unrecorded_ratelimit(&order, &declared, &DeployLock::default(), "default"),
            Some(("shop", "RATELIMIT_LOGIN")),
            "what CI refuses to allocate"
        );
        let mut lock = DeployLock::default();
        lock.allocate_ratelimit("default", "shop", "RATELIMIT_LOGIN");
        let again = plan_of(&order, &declared, &lock);
        let report = plan_report(&again, DeployFormat::Short);
        let id = crate::deploy::ledger::ratelimit_seed("default", "shop", "RATELIMIT_LOGIN");
        assert!(
            report.contains(&format!(
                "ratelimit reuse RATELIMIT_LOGIN (namespace {id})\n"
            )),
            "{report}"
        );
        assert_eq!(
            first_unrecorded_ratelimit(&order, &declared, &lock, "default"),
            None
        );
    }

    fn shop_with_buckets() -> BTreeMap<String, Resources> {
        project(vec![(
            "shop",
            Resources::default().r2_buckets(&[
                ("R2_ARCHIVES", "shop-archives", None),
                ("R2_INDEXES", "shop-indexes", Some("eu")),
            ]),
        )])
    }

    #[test]
    fn the_plan_creates_each_bucket_then_reuses_it() {
        let declared = shop_with_buckets();
        let order = names(&["shop"]);
        let fresh = plan_of(&order, &declared, &DeployLock::default());
        let r2: Vec<(&str, &str, &str)> = fresh.contexts[0]
            .r2
            .iter()
            .map(|b| (b.binding, b.action, b.bucket.as_str()))
            .collect();
        assert_eq!(
            r2,
            [
                ("R2_ARCHIVES", "create", "shop-archives"),
                ("R2_INDEXES", "create", "shop-indexes")
            ]
        );
        let mut lock = DeployLock::default();
        lock.record_r2("default", "shop", "R2_ARCHIVES", "shop-archives", None);
        lock.record_r2("default", "shop", "R2_INDEXES", "shop-indexes", Some("eu"));
        let again = plan_of(&order, &declared, &lock);
        assert!(again.contexts[0].r2.iter().all(|b| b.action == "reuse"));
    }

    #[test]
    fn a_named_environment_plans_its_own_qualified_buckets() {
        let declared = shop_with_buckets();
        let order = names(&["shop"]);
        let plan = derive_plan(
            &order,
            &declared,
            &DeployLock::default(),
            &SecretSource::default(),
            false,
            "staging",
        );
        assert_eq!(plan.contexts[0].r2[0].bucket, "shop-archives-staging");
        let report = plan_report(&plan, DeployFormat::Short);
        assert!(
            report.contains("r2 create shop-archives-staging (R2_ARCHIVES)\n"),
            "{report}"
        );
        assert!(
            report.contains("r2 create shop-indexes-staging (R2_INDEXES) in jurisdiction eu\n"),
            "{report}"
        );
    }

    #[test]
    fn a_changed_jurisdiction_is_a_recreate_that_says_the_old_bucket_stays() {
        let declared = shop_with_buckets();
        let mut lock = DeployLock::default();
        lock.record_r2("default", "shop", "R2_ARCHIVES", "shop-archives", None);
        lock.record_r2(
            "default",
            "shop",
            "R2_INDEXES",
            "shop-indexes",
            Some("fedramp"),
        );
        let order = names(&["shop"]);
        let plan = plan_of(&order, &declared, &lock);
        assert_eq!(plan.contexts[0].r2[0].action, "reuse");
        assert_eq!(plan.contexts[0].r2[1].action, "recreate");
        let report = plan_report(&plan, DeployFormat::Short);
        assert!(
            report.contains("r2 recreate shop-indexes (R2_INDEXES) in jurisdiction eu — the old bucket and its objects stay where they are"),
            "{report}"
        );
    }

    #[test]
    fn a_recreate_without_yes_is_refused_and_with_yes_is_not() {
        let declared = shop_with_buckets();
        let mut lock = DeployLock::default();
        lock.record_r2("default", "shop", "R2_ARCHIVES", "shop-archives", None);
        lock.record_r2(
            "default",
            "shop",
            "R2_INDEXES",
            "shop-indexes",
            Some("fedramp"),
        );
        let order = names(&["shop"]);
        let plan = plan_of(&order, &declared, &lock);
        assert!(recreate_refusal(&plan, false).is_some());
        assert!(recreate_refusal(&plan, true).is_none());
        // Nothing to recreate, nothing to refuse.
        let fresh = plan_of(&order, &declared, &DeployLock::default());
        assert!(recreate_refusal(&fresh, false).is_none());
    }

    #[test]
    fn a_bucket_a_changed_jurisdiction_left_behind_is_reported_as_an_orphan() {
        let declared = shop_with_buckets();
        let mut lock = DeployLock::default();
        lock.record_r2(
            "default",
            "shop",
            "R2_INDEXES",
            "shop-indexes",
            Some("fedramp"),
        );
        lock.record_r2("default", "shop", "R2_INDEXES", "shop-indexes", Some("eu"));
        let order = names(&["shop"]);
        let report = plan_report(&plan_of(&order, &declared, &lock), DeployFormat::Short);
        assert!(
            report.contains(
                "orphan r2 shop R2_INDEXES (shop-indexes, jurisdiction fedramp; not deleted)\n"
            ),
            "{report}"
        );
    }

    #[test]
    fn an_orphaned_bucket_is_shown_with_its_name_and_kept() {
        let declared = project(vec![("shop", Resources::default())]);
        let mut lock = DeployLock::default();
        lock.record_r2("default", "shop", "R2_OLD", "shop-old", None);
        let order = names(&["shop"]);
        let plan = plan_of(&order, &declared, &lock);
        let report = plan_report(&plan, DeployFormat::Short);
        assert!(
            report.contains("orphan r2 shop R2_OLD (shop-old; not deleted)\n"),
            "{report}"
        );
        assert!(!plan.orphans.has_prunable());
    }

    #[test]
    fn a_derived_name_r2_would_refuse_is_an_error_naming_the_instance() {
        let lock = DeployLock::default();
        let long = project(vec![(
            "a-very-long-context-name-that-goes-on-and-on",
            Resources::default().r2_buckets(&[(
                "R2_SOME_RATHER_LONG_CAPABILITY_NAME",
                "a-very-long-context-name-that-goes-on-and-on-some-rather-long-capability-name",
                None,
            )]),
        )]);
        let message = r2_bucket_problem(&long, &lock, "default").expect("too long");
        assert!(
            message.contains("`R2_SOME_RATHER_LONG_CAPABILITY_NAME`"),
            "{message}"
        );
        assert!(message.contains("3 to 63 characters"), "{message}");
        // The environment suffix counts toward the limit.
        let edge = project(vec![(
            "shop",
            Resources::default().r2_buckets(&[("R2_X", &"a".repeat(60), None)]),
        )]);
        assert!(r2_bucket_problem(&edge, &lock, "default").is_none());
        assert!(r2_bucket_problem(&edge, &lock, "production").is_some());
        let underscore = project(vec![(
            "shop_front",
            Resources::default().r2_buckets(&[("R2_X", "shop_front-x", None)]),
        )]);
        assert!(
            r2_bucket_problem(&underscore, &lock, "default")
                .unwrap()
                .contains("lowercase letters, digits and hyphens")
        );
    }

    #[test]
    fn two_instances_that_would_share_a_bucket_name_are_refused() {
        let colliding = project(vec![
            (
                "shop-cart",
                Resources::default().r2_buckets(&[("R2_A", "shop-cart-a", None)]),
            ),
            (
                "shop",
                Resources::default().r2_buckets(&[("R2_CART_A", "shop-cart-a", None)]),
            ),
        ]);
        let message =
            r2_bucket_problem(&colliding, &DeployLock::default(), "default").expect("a collision");
        assert!(message.contains("`shop-cart-a`"), "{message}");
        // Another environment's bucket holding the name is the same clash.
        let mut lock = DeployLock::default();
        lock.record_r2(
            "production",
            "shop",
            "R2_OTHER",
            "shop-archives-staging",
            None,
        );
        let staging = project(vec![(
            "shop",
            Resources::default().r2_buckets(&[("R2_ARCHIVES", "shop-archives", None)]),
        )]);
        assert!(r2_bucket_problem(&staging, &lock, "staging").is_some());
        assert!(r2_bucket_problem(&staging, &lock, "default").is_none());
    }

    // ---- interop I5 (#1972): Service Bindings to foreign Workers ----

    #[test]
    fn a_foreign_service_is_a_plan_line_with_no_action() {
        let resources = project(vec![(
            "shop-billing",
            Resources::default().foreign(&[("SERVICE_BILLING", "legacy-billing")]),
        )]);
        let order = names(&["shop-billing"]);
        let plan = plan_of(&order, &resources, &DeployLock::default());
        assert_eq!(
            plan_report(&plan, DeployFormat::Short),
            // I2 (#1971): a binding to a foreign Worker is no edge, so it
            // carries no edge key: the foreign Worker is not Bynk's and
            // never receives one.
            "service SERVICE_BILLING legacy-billing (foreign Worker; not provisioned)\ndeploy shop-billing\n"
        );
        let json: serde_json::Value =
            serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
        assert_eq!(
            json["contexts"][0]["foreign_services"],
            serde_json::json!([{ "binding": "SERVICE_BILLING", "worker": "legacy-billing" }])
        );
        assert_eq!(json["contexts"][0]["binds_to"], serde_json::json!([]));
    }

    /// Track §11 Q8: warn, never refuse. Each Worker is asked once however
    /// many contexts bind it; a present one says nothing, a missing one says
    /// Cloudflare will reject the upload, and an unreadable answer is a
    /// warning of its own rather than a guess either way.
    #[test]
    fn foreign_worker_warnings_ask_once_and_never_refuse() {
        let resources = project(vec![
            (
                "a",
                Resources::default().foreign(&[("SERVICE_BILLING", "legacy-billing")]),
            ),
            (
                "b",
                Resources::default().foreign(&[
                    ("SERVICE_BILLING", "legacy-billing"),
                    ("SERVICE_LEDGER", "ledger"),
                    ("SERVICE_TAX", "tax"),
                ]),
            ),
        ]);
        let order = names(&["a", "b"]);
        let mut asked = Vec::new();
        let warnings = foreign_worker_warnings(&order, &resources, |name| {
            asked.push(name.to_string());
            match name {
                "legacy-billing" => WorkerPresence::Missing,
                "ledger" => WorkerPresence::Present,
                _ => WorkerPresence::Unknown("Authentication error [code: 10000]".into()),
            }
        });
        assert_eq!(asked, ["ledger", "legacy-billing", "tax"]);
        assert_eq!(warnings.len(), 2);
        assert!(
            warnings[0].contains("`a`, `b` binds to Worker `legacy-billing`")
                && warnings[0].contains("rejects a Service Binding to a missing Worker at upload"),
            "{}",
            warnings[0]
        );
        assert!(
            warnings[1].contains("could not check that Worker `tax`")
                && warnings[1].contains("[code: 10000]"),
            "{}",
            warnings[1]
        );
    }

    /// Only Cloudflare's "Worker not found" codes read as missing.
    #[test]
    fn classify_worker_listing_reads_only_not_found_as_missing() {
        assert_eq!(classify_worker_listing(true, ""), WorkerPresence::Present);
        assert_eq!(
            classify_worker_listing(
                false,
                "A request to the Cloudflare API (/accounts/x/workers/scripts/legacy-billing/deployments) failed.\n  This Worker does not exist on your account. [code: 10007]"
            ),
            WorkerPresence::Missing
        );
        assert_eq!(
            classify_worker_listing(false, "… [code: 10090]"),
            WorkerPresence::Missing
        );
        assert!(matches!(
            classify_worker_listing(false, "fetch failed"),
            WorkerPresence::Unknown(_)
        ));
    }

    // ---- v0.177 (#643): the deploy-time contract-skew gate ----

    fn lock_with(worker: &str, contracts: &[(&str, &str)]) -> DeployLock {
        let mut lock = DeployLock::default();
        lock.record_deployed(
            "default",
            worker,
            Some(
                contracts
                    .iter()
                    .map(|(s, h)| (s.to_string(), h.to_string()))
                    .collect(),
            ),
        );
        lock
    }

    fn expects(dep: &str, svc: &str, hash: &str) -> BTreeMap<String, BTreeMap<String, String>> {
        BTreeMap::from([(
            dep.to_string(),
            BTreeMap::from([(svc.to_string(), hash.to_string())]),
        )])
    }

    #[test]
    fn conflicting_env_passthrough_finds_bare_and_equals_forms() {
        assert_eq!(
            conflicting_env_passthrough(&names(&["--env", "production"])),
            Some("--env")
        );
        assert_eq!(
            conflicting_env_passthrough(&names(&["--env=production"])),
            Some("--env=production")
        );
        assert_eq!(
            conflicting_env_passthrough(&names(&["--environment", "production"])),
            Some("--environment")
        );
        assert_eq!(
            conflicting_env_passthrough(&names(&["--minify"])),
            None,
            "an unrelated flag is not a conflict"
        );
        assert_eq!(conflicting_env_passthrough(&[]), None);
    }

    #[test]
    fn plan_creates_or_reuses_kv_from_the_ledger() {
        let order = names(&["api"]);
        let declared = project(vec![("api", Resources::default().needs_kv())]);
        let fresh = DeployLock::default();
        assert_eq!(
            plan_of(&order, &declared, &fresh).contexts[0].kv[0].action,
            "create"
        );
        assert_eq!(
            plan_of(&order, &declared, &with_kv(DeployLock::default(), "api")).contexts[0].kv[0]
                .action,
            "reuse"
        );
        assert!(
            plan_of(
                &order,
                &project(vec![("api", Resources::default())]),
                &fresh
            )
            .contexts[0]
                .kv
                .is_empty(),
            "a context declaring no KV gets no KV line"
        );
    }

    // ---- #600 slice 1: queues and Durable Objects ----------------------

    #[test]
    fn plan_creates_or_reuses_a_queue_by_its_name() {
        // Queues reconcile on the name `from queue("n")` gave them — there is
        // no id — so the ledger's whole answer is "have we made this before?"
        let order = names(&["jobs"]);
        let declared = project(vec![("jobs", Resources::default().consumes(&["intake"]))]);
        let line =
            |lock: &DeployLock| plan_of(&order, &declared, lock).contexts[0].queues[0].action;
        assert_eq!(line(&DeployLock::default()), "create");
        assert_eq!(line(&with_queue(DeployLock::default(), "intake")), "reuse");
        // The name is keyed environment-wide, not per worker: a different
        // context consuming `intake` means the same queue.
        assert!(with_queue(DeployLock::default(), "intake").has_queue("default", "intake"));
        assert!(!with_queue(DeployLock::default(), "intake").has_queue("default", "other"));
    }

    #[test]
    fn an_unrecorded_queue_only_sent_to_may_be_created_or_reused() {
        // P2 (#2578): an adopted producer-only queue is never recorded, so
        // the plan cannot claim it will be created.
        let order = names(&["shop"]);
        let declared = project(vec![("shop", Resources::default().produces(&["events"]))]);
        let line =
            |lock: &DeployLock| plan_of(&order, &declared, lock).contexts[0].queues[0].action;
        assert_eq!(line(&DeployLock::default()), "create or reuse");
        assert_eq!(line(&with_queue(DeployLock::default(), "events")), "reuse");
        // Consumed as well as sent to, it is recorded either way: `create`.
        let both = project(vec![(
            "shop",
            Resources::default()
                .consumes(&["events"])
                .produces(&["events"]),
        )]);
        assert_eq!(
            plan_of(&order, &both, &DeployLock::default()).contexts[0].queues[0].action,
            "create"
        );
    }

    #[test]
    fn a_dataset_is_listed_under_its_qualified_name_with_no_action() {
        // P3b (#2582): nothing to create or record, so the line only names it.
        let order = names(&["shop"]);
        let declared = project(vec![(
            "shop",
            Resources::default().analytics(&[("ANALYTICS_SIGNUPS", "shop_signups")]),
        )]);
        let plan = derive_plan(
            &order,
            &declared,
            &DeployLock::default(),
            &SecretSource::default(),
            false,
            "staging",
        );
        assert_eq!(plan.contexts[0].datasets[0].dataset, "shop_signups_staging");
        assert!(plan_report(&plan, DeployFormat::Short).contains(
            "dataset shop_signups_staging (ANALYTICS_SIGNUPS; created on first write)\n"
        ));
    }

    #[test]
    fn a_context_with_no_queues_gets_no_queue_lines() {
        assert!(
            plan_of(
                &names(&["api"]),
                &project(vec![("api", Resources::default())]),
                &DeployLock::default(),
            )
            .contexts[0]
                .queues
                .is_empty()
        );
    }

    #[test]
    fn a_passthrough_secrets_file_is_refused_only_when_the_run_has_a_first_deploy() {
        // #2055: a first deploy passes its own `--secrets-file`.
        let order = names(&["api", "web"]);
        let mut lock = DeployLock::default();
        lock.record_deployed("staging", "api", None);
        let args = |a: &[&str]| names(a);
        assert_eq!(
            secrets_file_passthrough_refusal(
                &args(&["--secrets-file", "s.env"]),
                &order,
                &lock,
                "staging"
            )
            .as_deref(),
            Some(
                "bynk: `--secrets-file` after `--` conflicts with the secrets file `bynk deploy` uploads with a new Worker's first version (`web-staging`).\n  \
                 Pass it to bynk instead, before `--`: `bynk deploy --secrets-file <file>`.\n"
            )
        );
        assert!(
            secrets_file_passthrough_refusal(
                &args(&["--secrets-file=s.env"]),
                &order,
                &lock,
                "staging"
            )
            .is_some_and(|r| r.contains("`--secrets-file=s.env`")),
            "the `=`-joined spelling is the same flag"
        );
        lock.record_deployed("staging", "web", None);
        assert_eq!(
            secrets_file_passthrough_refusal(
                &args(&["--secrets-file", "s.env"]),
                &order,
                &lock,
                "staging"
            ),
            None,
            "a redeploy passes no secrets file of its own"
        );
        assert_eq!(
            secrets_file_passthrough_refusal(&args(&["--minify"]), &order, &lock, "default"),
            None
        );
    }

    #[test]
    fn a_wrangler_below_the_floor_refuses_only_a_push_that_declares_a_durable_object() {
        // #1796: `exports` is read from WRANGLER_MIN on, so below it a context
        // with an agent can't deploy; one without still can.
        let min = probe::Version::parse(bynk_emit::WRANGLER_MIN).unwrap();
        let wrangler = |version: probe::Version| probe::Probe {
            tool: "wrangler".to_string(),
            version: Some(version),
            provenance: probe::Provenance::Path("/usr/bin/wrangler".into()),
        };
        // The newest version strictly below the floor, whatever the floor is.
        let old = if min.patch > 0 {
            probe::Version {
                patch: min.patch - 1,
                ..min
            }
        } else if min.minor > 0 {
            probe::Version {
                minor: min.minor - 1,
                patch: 999,
                ..min
            }
        } else {
            probe::Version {
                major: min.major - 1,
                minor: 999,
                patch: 999,
            }
        };
        let resources = project(vec![
            ("api", Resources::default()),
            ("jobs", Resources::default().exports(&["JobLedger"])),
        ]);
        let both = names(&["api", "jobs"]);

        let refusal = wrangler_floor_refusal(&wrangler(old), &both, &resources)
            .expect("an agent-bearing push below the floor is refused");
        for part in [
            format!("wrangler {old}"),
            bynk_emit::WRANGLER_MIN.to_string(),
            "`jobs`".to_string(),
            "npm install -g wrangler@4".to_string(),
        ] {
            assert!(refusal.contains(&part), "missing {part:?} in: {refusal}");
        }

        // No class in what's pushed (`--context api`), the floor itself, or an
        // unversioned npx wrangler: nothing to refuse.
        assert_eq!(
            wrangler_floor_refusal(&wrangler(old), &names(&["api"]), &resources),
            None
        );
        assert_eq!(
            wrangler_floor_refusal(&wrangler(min), &both, &resources),
            None
        );
        let npx = probe::Probe {
            tool: "wrangler".to_string(),
            version: None,
            provenance: probe::Provenance::Npx,
        };
        assert_eq!(wrangler_floor_refusal(&npx, &both, &resources), None);
    }

    /// State-migrations S3 (#2248): a rename and a deletion each get a plan
    /// line saying what happens to stored data; a deletion is named as
    /// `<context>.<Class>`, the form `--confirm-delete` takes.
    #[test]
    fn renames_and_deletions_are_planned_by_what_they_do_to_data() {
        let order = names(&["shop-orders"]);
        let declared = Resources {
            renamed_durable_objects: vec![("Cart".to_string(), "Basket".to_string())],
            deleted_durable_objects: vec!["Wishlist".to_string()],
            ..Resources::default().exports(&["Basket"])
        };
        let resources = project(vec![("shop-orders", declared)]);
        let plan = plan_of(&order, &resources, &DeployLock::default());
        let context = &plan.contexts[0];
        assert_eq!(context.deleted_durable_objects.len(), 1);
        assert_eq!(
            context.deleted_durable_objects[0].class,
            "shop.orders.Wishlist"
        );
        assert!(!context.deleted_durable_objects[0].already_applied);
        let text = plan_report(&plan, DeployFormat::Short);
        assert!(
            text.contains("rename durable object Cart → Basket (moves its stored data)"),
            "{text}"
        );
        assert!(
            text.contains(
                "delete durable object shop.orders.Wishlist (all stored data; irreversible)"
            ),
            "{text}"
        );
        let json = plan_report(&plan, DeployFormat::Json);
        assert!(json.contains("\"durable_objects_renamed\""), "{json}");
        assert!(json.contains("\"shop.orders.Wishlist\""), "{json}");
    }

    /// #2279 review: what a push records is what the next plan reads, for
    /// renames and deletions alike, and a later push without them clears both.
    #[test]
    fn a_push_records_its_tombstones_for_the_next_plan() {
        let order = names(&["shop"]);
        let declared = Resources {
            renamed_durable_objects: vec![("Box".to_string(), "Crate".to_string())],
            deleted_durable_objects: vec!["Wishlist".to_string()],
            ..Resources::default().exports(&["Crate"])
        };
        let mut lock = DeployLock::default();
        record_pushed_tombstones(&mut lock, "default", "shop", &declared);
        let resources = project(vec![("shop", declared)]);
        let plan = plan_of(&order, &resources, &lock);
        assert!(plan.contexts[0].renamed_durable_objects[0].already_applied);
        assert!(plan.contexts[0].deleted_durable_objects[0].already_applied);
        record_pushed_tombstones(&mut lock, "default", "shop", &Resources::default());
        assert!(!lock.has_renamed_class("default", "shop", "Box", "Crate"));
        assert!(!lock.has_deleted_class("default", "shop", "Wishlist"));
    }

    /// #2276 Decision D: a rename an earlier push carried reads as applied;
    /// a different target, or another environment, still reads as moving data.
    #[test]
    fn a_rename_already_pushed_reads_as_applied() {
        let order = names(&["shop"]);
        let declared = Resources {
            renamed_durable_objects: vec![("Box".to_string(), "Crate".to_string())],
            ..Resources::default().exports(&["Crate"])
        };
        let resources = project(vec![("shop", declared)]);
        let mut lock = DeployLock::default();
        lock.record_renamed_classes(
            "default",
            "shop",
            [("Box".to_string(), "Crate".to_string())].into(),
        );
        let plan = plan_of(&order, &resources, &lock);
        assert!(plan.contexts[0].renamed_durable_objects[0].already_applied);
        let text = plan_report(&plan, DeployFormat::Short);
        assert!(
            text.contains("rename durable object Box → Crate (already applied by an earlier deploy; the tombstone stays until Cloudflare reports it stale)"),
            "{text}"
        );
        let json = plan_report(&plan, DeployFormat::Json);
        assert!(json.contains("\"already_applied\": true"), "{json}");
        lock.record_renamed_classes(
            "default",
            "shop",
            [("Box".to_string(), "Bin".to_string())].into(),
        );
        let plan = plan_of(&order, &resources, &lock);
        assert!(!plan.contexts[0].renamed_durable_objects[0].already_applied);
        lock.record_renamed_classes("default", "shop", BTreeMap::new());
        assert!(!lock.has_renamed_class("default", "shop", "Box", "Bin"));
        assert!(
            !toml::to_string(&lock).unwrap().contains("renamed_classes"),
            "an empty record leaves nothing in the ledger"
        );
    }

    /// State-migrations S3 (#2248, Decision E): a deletion needs a named
    /// confirmation; `--yes` never stands in, and a confirmation for a class
    /// the plan does not delete is refused. (Tests run without a terminal, so
    /// the interactive prompt is not reached.)
    #[test]
    fn a_deletion_needs_its_own_named_confirmation() {
        let order = names(&["shop"]);
        let declared = Resources {
            deleted_durable_objects: vec!["Wishlist".to_string()],
            ..Resources::default()
        };
        let resources = project(vec![("shop", declared)]);
        let lock = DeployLock::default();
        let plan = plan_of(&order, &resources, &lock);
        assert!(
            !confirm_deletions(&plan, &[], &lock),
            "unconfirmed: refused"
        );
        assert!(confirm_deletions(&plan, &names(&["shop.Wishlist"]), &lock));
        assert!(
            !confirm_deletions(&plan, &names(&["shop.Wishlist", "shop.Other"]), &lock),
            "a confirmation for a class not deleted is refused"
        );
        let none = project(vec![("shop", Resources::default())]);
        let quiet = plan_of(&order, &none, &lock);
        assert!(
            confirm_deletions(&quiet, &[], &lock),
            "nothing to delete, nothing to confirm"
        );
        assert!(!confirm_deletions(
            &quiet,
            &names(&["shop.Wishlist"]),
            &lock
        ));
    }

    /// #2253 review: the `removed agent` declaration stays until Cloudflare
    /// reports its tombstone stale, so the deploy after the deletion carries
    /// the tombstone again. The ledger records the first push's deletion, so
    /// the second plan shows it as applied and asks nothing; a CI flag still
    /// naming it — even after the declaration is gone — is not refused.
    #[test]
    fn a_deletion_already_pushed_is_not_confirmed_again() {
        let order = names(&["shop"]);
        let declared = Resources {
            deleted_durable_objects: vec!["Wishlist".to_string()],
            ..Resources::default()
        };
        let resources = project(vec![("shop", declared)]);
        let mut lock = DeployLock::default();
        lock.record_deleted_classes("default", "shop", ["Wishlist".to_string()].into());
        let plan = plan_of(&order, &resources, &lock);
        assert!(plan.contexts[0].deleted_durable_objects[0].already_applied);
        let text = plan_report(&plan, DeployFormat::Short);
        assert!(
            text.contains("already deleted by an earlier deploy"),
            "{text}"
        );
        assert!(
            confirm_deletions(&plan, &[], &lock),
            "an applied deletion asks nothing"
        );
        assert!(confirm_deletions(&plan, &names(&["shop.Wishlist"]), &lock));
        // The declaration removed: the plan deletes nothing, and the leftover
        // flag names a deletion the ledger records, so it is not refused.
        let gone = project(vec![("shop", Resources::default())]);
        let after = plan_of(&order, &gone, &lock);
        assert!(confirm_deletions(&after, &names(&["shop.Wishlist"]), &lock));
        assert!(!confirm_deletions(&after, &names(&["shop.Other"]), &lock));
    }

    #[test]
    fn the_durable_object_lines_are_advisory_in_every_ledger_state() {
        // #1796, ADR 0194 D1's principle: Cloudflare reconciles the declared
        // `exports` against the Worker's namespaces, so the plan says what the
        // push will *declare* and never what already exists. A ledger that has
        // deployed this context before must not change the lines — there is no
        // state here for the ledger to have an opinion about.
        let order = names(&["jobs"]);
        let declared = project(vec![(
            "jobs",
            Resources::default().exports(&["JobLedger", "__EventsFanout"]),
        )]);
        for lock in [DeployLock::default(), lock_with_deployed(&["jobs"])] {
            let plan = plan_of(&order, &declared, &lock);
            let objects = &plan.contexts[0].durable_objects;
            assert_eq!(
                objects.iter().map(|o| o.class).collect::<Vec<_>>(),
                ["JobLedger", "__EventsFanout"],
                "every declared class gets a line, the fan-out class included"
            );
            for object in objects {
                assert_eq!(object.storage, "sqlite");
                assert_eq!(
                    object.reconciled_by, "Cloudflare",
                    "the plan names an owner other than bynk — that is the advisory"
                );
            }
        }
        // No agent, no Durable Object line.
        assert!(
            plan_of(
                &names(&["api"]),
                &project(vec![("api", Resources::default())]),
                &DeployLock::default(),
            )
            .contexts[0]
                .durable_objects
                .is_empty()
        );
    }

    // ---- #601 slice 2: `--context` dependency liveness (D4) ------------

    #[test]
    fn context_flag_names_a_dependency_that_was_never_deployed() {
        let g = graph(&[("orders", &["payment"]), ("payment", &[])]);
        assert_eq!(
            absent_dependencies("orders", &g, &DeployLock::default(), "default"),
            names(&["payment"]),
            "deploying orders alone would fail at upload — say which target is missing"
        );
        // Once payment is in the ledger, orders alone is fine.
        assert!(
            absent_dependencies("orders", &g, &lock_with_deployed(&["payment"]), "default")
                .is_empty()
        );
        // A worker with no bindings never has an absent dependency.
        assert!(absent_dependencies("payment", &g, &DeployLock::default(), "default").is_empty());
    }

    #[test]
    fn the_plan_distinguishes_a_first_deploy_from_a_redeploy() {
        let order = names(&["api"]);
        let declared = project(vec![("api", Resources::default())]);
        assert_eq!(
            plan_of(&order, &declared, &DeployLock::default()).contexts[0].action,
            "deploy"
        );
        assert_eq!(
            plan_of(&order, &declared, &lock_with_deployed(&["api"])).contexts[0].action,
            "redeploy",
            "a re-run re-pushes rather than skipping, so the plan must not say `deploy`"
        );
    }

    #[test]
    fn the_plan_carries_the_order_and_each_context_s_bindings() {
        let order = names(&["payment", "orders"]);
        let declared = project(vec![
            ("orders", Resources::default().binds(&["payment"])),
            ("payment", Resources::default()),
        ]);
        let plan = plan_of(&order, &declared, &DeployLock::default());
        assert_eq!(plan.order, vec!["payment", "orders"]);
        assert_eq!(plan.contexts[1].worker, "orders");
        assert_eq!(plan.contexts[1].binds_to, vec!["payment"]);
        assert!(plan.contexts[0].binds_to.is_empty());
    }

    /// #2028: under `--env staging` the plan names what the push creates —
    /// each Worker, binding target and queue `-staging` — while the ledger,
    /// keyed by logical name within the environment, still decides each
    /// action.
    #[test]
    fn under_a_named_environment_the_plan_names_what_the_push_creates() {
        let order = names(&["payment", "orders"]);
        let declared = project(vec![
            (
                "orders",
                Resources::default().binds(&["payment"]).consumes(&["jobs"]),
            ),
            ("payment", Resources::default()),
        ]);
        let mut lock = DeployLock::default();
        lock.record_deployed("staging", "payment", Some(Default::default()));
        lock.record_deployed("staging", "gone", Some(Default::default()));
        lock.record_queue("staging", "jobs");
        lock.record_queue("staging", "old-jobs");
        let plan = derive_plan(
            &order,
            &declared,
            &lock,
            &SecretSource::default(),
            false,
            "staging",
        );
        assert_eq!(plan.order, vec!["payment-staging", "orders-staging"]);
        assert_eq!(plan.contexts[0].worker, "payment-staging");
        assert_eq!(plan.contexts[0].action, "redeploy", "found by logical name");
        assert_eq!(plan.contexts[1].worker, "orders-staging");
        assert_eq!(plan.contexts[1].action, "deploy");
        assert_eq!(plan.contexts[1].binds_to, vec!["payment-staging"]);
        assert_eq!(plan.contexts[1].queues[0].queue, "jobs-staging");
        assert_eq!(plan.contexts[1].queues[0].action, "reuse");
        // Shown as the account names them; pruned by ledger key.
        assert_eq!(plan.shown_orphans.workers, vec!["gone-staging"]);
        assert_eq!(plan.shown_orphans.queues, vec!["old-jobs-staging"]);
        assert_eq!(plan.orphans.workers, vec!["gone"]);
        assert_eq!(plan.orphans.queues, vec!["old-jobs"]);

        let text = plan_report(&plan, DeployFormat::Short);
        assert!(text.contains("orphan worker gone-staging\n"), "{text}");
        assert!(text.contains("orphan queue old-jobs-staging\n"), "{text}");
        assert!(text.contains("queue reuse jobs-staging\n"), "{text}");
        assert!(text.contains("redeploy payment-staging\n"), "{text}");
        assert!(text.contains("deploy orders-staging\n"), "{text}");
        assert!(
            text.contains("order payment-staging → orders-staging\n"),
            "{text}"
        );

        let json: serde_json::Value =
            serde_json::from_str(&plan_report(&plan, DeployFormat::Json)).unwrap();
        assert_eq!(
            json["order"],
            serde_json::json!(["payment-staging", "orders-staging"])
        );
        assert_eq!(json["contexts"][1]["worker"], "orders-staging");
        assert_eq!(
            json["contexts"][1]["binds_to"],
            serde_json::json!(["payment-staging"])
        );
        assert_eq!(json["contexts"][1]["queues"][0]["queue"], "jobs-staging");
        assert_eq!(
            json["orphans"]["workers"],
            serde_json::json!(["gone-staging"])
        );
        assert_eq!(
            json["orphans"]["queues"],
            serde_json::json!(["old-jobs-staging"])
        );
    }

    /// #2028: the plan and the push get every name from the same functions.
    /// The `[env.staging]` block the push hands Wrangler is synthesised from a
    /// generated config, and its `name`, Service Binding targets and queue
    /// names are exactly the plan's `worker`, `binds_to` and `queues`.
    #[test]
    fn the_plan_and_the_push_share_one_name_function() {
        const ORDERS: &str = r#"name = "shop-orders"
main = "index.ts"

[[services]]
binding = "PAYMENT"
service = "shop-payment"

[[queues.consumers]]
queue = "jobs"
"#;
        let order = names(&["shop-payment", "shop-orders"]);
        let declared = project(vec![
            (
                "shop-orders",
                Resources::default()
                    .binds(&["shop-payment"])
                    .consumes(&["jobs"]),
            ),
            ("shop-payment", Resources::default()),
        ]);
        for environment in ["staging", "production", "default"] {
            let plan = derive_plan(
                &order,
                &declared,
                &DeployLock::default(),
                &SecretSource::default(),
                false,
                environment,
            );
            let orders = &plan.contexts[1];
            let pushed: toml::Table = synthesise_environment_block(
                ORDERS,
                environment,
                &BTreeMap::new(),
                &BTreeSet::new(),
            )
            .unwrap()
            .parse()
            .unwrap();
            let block = &pushed["env"][environment];
            assert_eq!(block["name"].as_str(), Some(orders.worker.as_str()));
            assert_eq!(
                block["services"][0]["service"].as_str(),
                Some(orders.binds_to[0].as_str())
            );
            assert_eq!(
                block["queues"]["consumers"][0]["queue"].as_str(),
                Some(orders.queues[0].queue.as_str())
            );
            assert_eq!(plan.order[1], orders.worker);
            assert_eq!(
                orders.worker,
                deployed_worker_name(environment, "shop-orders")
            );
        }
    }

    #[test]
    fn dry_run_and_ci_gates_do_not_reach_mutation() {
        assert!(
            DeployOptions {
                dry_run: true,
                ..Default::default()
            }
            .dry_run
        );
    }

    /// Review of #1974: an instance title another Worker's namespace already
    /// takes is refused, naming both; distinct titles pass.
    #[test]
    fn a_kv_title_two_namespaces_would_share_is_refused() {
        let colliding = project(vec![
            (
                "shop",
                Resources::default().kv_instances(&["KV_CART_SESSIONS"]),
            ),
            (
                "shop-cart",
                Resources::default().kv_instances(&["KV_SESSIONS"]),
            ),
        ]);
        let message =
            kv_title_collision(&colliding, &DeployLock::default(), "default").expect("a collision");
        assert!(
            message.contains("`shop-cart-sessions`")
                && message.contains("`KV_CART_SESSIONS` for `shop`")
                && message.contains("`KV_SESSIONS` for `shop-cart`"),
            "{message}"
        );
        // An instance title equal to another Worker's bare namespace title.
        let bare = project(vec![
            ("shop", Resources::default().kv_instances(&["KV_CART"])),
            ("shop-cart", Resources::default().needs_kv()),
        ]);
        assert!(kv_title_collision(&bare, &DeployLock::default(), "default").is_some());
        let distinct = project(vec![(
            "shop-cart",
            Resources::default()
                .needs_kv()
                .kv_instances(&["KV_PRICE_CACHE", "KV_SESSIONS"]),
        )]);
        assert_eq!(
            kv_title_collision(&distinct, &DeployLock::default(), "staging"),
            None
        );
    }

    /// #2048: Worker `shop` with the bare `Kv` and `capability Staging`. Its
    /// bare namespace under `--env staging` and its instance in the default
    /// environment are both titled `shop-staging`.
    fn shop_with_staging_instance() -> BTreeMap<String, Resources> {
        project(vec![(
            "shop",
            Resources::default()
                .needs_kv()
                .kv_instances(&["KV_STAGING"]),
        )])
    }

    /// #2048: a first staging deploy whose bare title the default
    /// environment's instance already holds is refused, naming both sides
    /// and both environments.
    #[test]
    fn a_title_another_environment_holds_is_refused() {
        let resources = shop_with_staging_instance();
        let mut lock = DeployLock::default();
        lock.record_kv("default", "shop", "KV", "bare-id");
        lock.record_kv("default", "shop", "KV_STAGING", "instance-id");
        let message = kv_title_collision(&resources, &lock, "staging").expect("a collision");
        assert_eq!(
            message,
            "KV namespace for `shop` in `--env staging` and KV namespace `KV_STAGING` for `shop` \
             in the default environment would both be titled `shop-staging` on the account, \
             which holds one namespace per title; rename a capability or context so they differ"
        );
    }

    /// #2048: the reverse order. A default deploy that would create a title
    /// a known staging environment would create is refused too, before
    /// either fails at `kv namespace create`.
    #[test]
    fn a_title_another_environment_would_create_is_refused() {
        let resources = shop_with_staging_instance();
        let mut lock = DeployLock::default();
        // Staging is known to the ledger, but has not provisioned KV yet.
        lock.record_deployed("staging", "other", Some(Default::default()));
        let message = kv_title_collision(&resources, &lock, "default").expect("a collision");
        assert!(
            message.contains("KV namespace `KV_STAGING` for `shop` in the default environment")
                && message.contains("KV namespace for `shop` in `--env staging`")
                && message.contains("`shop-staging`"),
            "{message}"
        );
    }

    /// #2048: a recorded namespace in another environment holds its title
    /// even when the source no longer declares it (an orphan not yet
    /// pruned).
    #[test]
    fn an_orphan_in_another_environment_still_holds_its_title() {
        let resources = project(vec![(
            "shop",
            Resources::default().kv_instances(&["KV_STAGING"]),
        )]);
        let mut lock = DeployLock::default();
        lock.record_kv("staging", "shop", "KV", "orphaned-id");
        let message = kv_title_collision(&resources, &lock, "default").expect("a collision");
        assert!(
            message.contains("KV namespace for `shop` in `--env staging`"),
            "{message}"
        );
    }

    /// #2048: only namespaces this deploy creates are checked. One already
    /// recorded here exists, so it cannot fail; the other environment's deploy
    /// is where the clash is refused.
    #[test]
    fn a_namespace_already_recorded_here_is_not_refused() {
        let resources = shop_with_staging_instance();
        let mut lock = DeployLock::default();
        lock.record_kv("default", "shop", "KV", "bare-id");
        lock.record_kv("default", "shop", "KV_STAGING", "instance-id");
        lock.record_deployed("staging", "other", Some(Default::default()));
        assert_eq!(kv_title_collision(&resources, &lock, "default"), None);
        // ...and the staging deploy is the one refused.
        assert!(kv_title_collision(&resources, &lock, "staging").is_some());
    }

    /// #2048: an environment the ledger does not know is not compared, and
    /// distinct titles across known environments pass.
    #[test]
    fn unknown_environments_and_distinct_titles_pass() {
        let resources = shop_with_staging_instance();
        assert_eq!(
            kv_title_collision(&resources, &DeployLock::default(), "staging"),
            None
        );
        let plain = project(vec![(
            "shop-cart",
            Resources::default()
                .needs_kv()
                .kv_instances(&["KV_SESSIONS"]),
        )]);
        let mut lock = DeployLock::default();
        lock.record_kv("default", "shop-cart", "KV", "bare-id");
        lock.record_kv("default", "shop-cart", "KV_SESSIONS", "sessions-id");
        lock.record_kv("production", "shop-cart", "KV", "prod-id");
        assert_eq!(kv_title_collision(&plain, &lock, "staging"), None);
    }

    /// S4b (#1967): CI refuses any unrecorded namespace, including an
    /// instance whose Worker's bare namespace is recorded.
    #[test]
    fn ci_refuses_the_first_unrecorded_namespace_bare_or_instance() {
        use crate::deploy::ledger::tests::with_kv_instance;
        let order = names(&["shop-cart"]);
        let declared = project(vec![(
            "shop-cart",
            Resources::default()
                .needs_kv()
                .kv_instances(&["KV_PRICE_CACHE", "KV_SESSIONS"]),
        )]);
        assert_eq!(
            first_unrecorded_kv(&order, &declared, &DeployLock::default(), "default"),
            Some(("shop-cart", "KV"))
        );
        let bare = with_kv(DeployLock::default(), "shop-cart");
        assert_eq!(
            first_unrecorded_kv(&order, &declared, &bare, "default"),
            Some(("shop-cart", "KV_PRICE_CACHE"))
        );
        let all = with_kv_instance(
            with_kv_instance(bare, "shop-cart", "KV_PRICE_CACHE"),
            "shop-cart",
            "KV_SESSIONS",
        );
        assert_eq!(
            first_unrecorded_kv(&order, &declared, &all, "default"),
            None
        );
        assert_eq!(
            first_unrecorded_kv(&order, &declared, &all, "staging"),
            Some(("shop-cart", "KV")),
            "recorded under another environment is unrecorded here"
        );
    }

    #[test]
    fn an_interrupted_rotation_says_how_to_repair_it() {
        let edge = |caller: &str, callee: &str| Edge {
            caller: caller.to_string(),
            callee: callee.to_string(),
        };
        let set = |rotating: bool| EdgeKey::Set {
            value: "minted-value".into(),
            rotating,
        };
        let keys: BTreeMap<Edge, EdgeKey> = [
            (edge("commerce-orders", "commerce-payment"), set(true)),
            (edge("commerce-orders", "commerce-ledger"), set(false)),
            (edge("commerce-ledger", "commerce-payment"), EdgeKey::Keep),
        ]
        .into_iter()
        .collect();
        // `commerce-payment` finished; `commerce-orders` did not.
        let hint = interrupted_rotation_hint("default", &keys, &names(&["commerce-payment"]))
            .expect("a rotation was interrupted");
        assert!(
            hint.contains("`commerce-orders` → `commerce-payment`"),
            "{hint}"
        );
        assert!(
            !hint.contains("commerce-ledger"),
            "an edge that had no key, or kept its key, cannot be split: {hint}"
        );
        assert!(hint.contains("bynk deploy --force"), "{hint}");
        assert!(!hint.contains("minted-value"), "never the value: {hint}");
        // Both ends finished, so nothing was split.
        assert_eq!(
            interrupted_rotation_hint(
                "default",
                &keys,
                &names(&["commerce-payment", "commerce-orders"])
            ),
            None
        );
    }

    #[test]
    fn non_interactive_deploy_requires_yes() {
        assert!(!requires_interactive_confirmation(false, false));
        assert!(requires_interactive_confirmation(false, true));
        assert!(!requires_interactive_confirmation(true, false));
    }

    #[test]
    fn matching_contracts_are_not_a_skew() {
        let lock = lock_with("app-b", &[("whoami", "317bdd3de84d2176")]);
        let found = contract_skews(
            &expects("app.b", "whoami", "317bdd3de84d2176"),
            &lock,
            |c| c.replace('.', "-"),
            "default",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_changed_contract_is_a_skew() {
        // The scenario the increment exists for: B was redeployed with a new
        // contract, and A still stamps the old one.
        let lock = lock_with("app-b", &[("whoami", "ffffffffffffffff")]);
        let found = contract_skews(
            &expects("app.b", "whoami", "317bdd3de84d2176"),
            &lock,
            |c| c.replace('.', "-"),
            "default",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].dependency, "app.b");
        assert_eq!(found[0].service, "whoami");
        assert_eq!(found[0].expected, "317bdd3de84d2176");
        assert_eq!(found[0].live, "ffffffffffffffff");
    }

    #[test]
    fn a_service_the_live_callee_no_longer_provides_is_a_skew() {
        let lock = lock_with("app-b", &[("somethingElse", "317bdd3de84d2176")]);
        let found = contract_skews(
            &expects("app.b", "whoami", "317bdd3de84d2176"),
            &lock,
            |c| c.replace('.', "-"),
            "default",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].live, "<absent>");
    }

    #[test]
    fn a_ledger_with_no_contract_record_yields_no_finding() {
        // Silence is not a match. A dependency deployed by a pre-v0.177 driver
        // has no contract record, and the gate must report only what it *knows*
        // is skewed — never what it merely cannot rule out. The runtime check is
        // the backstop for exactly this case, so a false accusation here would
        // block a legitimate deploy for no gain.
        let mut lock = DeployLock::default();
        lock.record_deployed("default", "app-b", None);
        let found = contract_skews(
            &expects("app.b", "whoami", "317bdd3de84d2176"),
            &lock,
            |c| c.replace('.', "-"),
            "default",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_callee_that_now_provides_nothing_is_a_total_skew() {
        // The counterpart to the rule above, and why the sentinel is `Option`
        // rather than an empty map: a callee that removed *all* its `on call`
        // services emits no manifest, so a bare-map field would record `{}` —
        // indistinguishable from "old ledger" — and the gate would wave through
        // the most complete skew there is. `Some({})` says "known to provide
        // nothing", which is a finding.
        let lock = lock_with("app-b", &[]);
        let found = contract_skews(
            &expects("app.b", "whoami", "317bdd3de84d2176"),
            &lock,
            |c| c.replace('.', "-"),
            "default",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].live, "<absent>");
    }

    #[test]
    fn a_never_deployed_dependency_yields_no_finding_here() {
        // That is D4's existing job (`absent_dependencies`), and it runs first.
        // Reporting it twice, in two vocabularies, would only confuse.
        let found = contract_skews(
            &expects("app.b", "whoami", "317bdd3de84d2176"),
            &DeployLock::default(),
            |c| c.replace('.', "-"),
            "default",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    // ---- #2077: the callee half of the contract-skew gate ----

    /// Chapter 14 of the shop, live: payment provides `authorise` at
    /// `febd15c2757b72e1`, and orders was compiled against it.
    fn live_shop(orders_expects: Option<&str>) -> DeployLock {
        let mut lock = lock_with("shop-payment", &[("authorise", "febd15c2757b72e1")]);
        lock.record_expects("default", "shop-payment", BTreeMap::new());
        lock.record_deployed("default", "shop-orders", Some(BTreeMap::new()));
        if let Some(hash) = orders_expects {
            lock.record_expects(
                "default",
                "shop-orders",
                expects("shop.payment", "authorise", hash),
            );
        }
        lock
    }

    fn provides(svc: &str, hash: &str) -> BTreeMap<String, String> {
        BTreeMap::from([(svc.to_string(), hash.to_string())])
    }

    fn worker_of(c: &str) -> String {
        c.replace('.', "-")
    }

    #[test]
    fn a_callee_whose_contract_changed_under_a_live_caller_is_refused() {
        // The issue's reproduction: `authorise`'s parameter renamed, so the
        // new build provides `da0d725f3a004040` while live orders stamps the
        // old hash.
        let lock = live_shop(Some("febd15c2757b72e1"));
        let found = callee_contract_skews(
            "shop-payment",
            &provides("authorise", "da0d725f3a004040"),
            &lock,
            worker_of,
            "default",
        );
        assert_eq!(
            found,
            [CalleeSkew {
                caller: "shop-orders".into(),
                dependency: "shop.payment".into(),
                service: "authorise".into(),
                expected: "febd15c2757b72e1".into(),
                provided: "da0d725f3a004040".into(),
                compat: None,
            }]
        );
        let refusal = callee_contract_skew_refusal("default", "shop-payment", &found);
        assert!(refusal.contains("`shop-orders` calls shop.payment.authorise"));
        assert!(refusal.contains(CONTRACT_SKEW));
    }

    /// #2077 review: a caller this build still has is advised to deploy the
    /// whole project, which moves it with the callee.
    #[test]
    fn a_built_caller_is_refused_with_the_whole_project_remedy() {
        let lock = live_shop(Some("febd15c2757b72e1"));
        let found = callee_contract_skews(
            "shop-payment",
            &provides("authorise", "da0d725f3a004040"),
            &lock,
            worker_of,
            "default",
        );
        let bindings = graph(&[("shop-orders", &["shop-payment"]), ("shop-payment", &[])]);
        let (built, orphaned) = split_orphaned_callers(found, &bindings);
        assert_eq!(built.len(), 1);
        assert!(orphaned.is_empty(), "{orphaned:?}");
        let refusal = callee_contract_skew_refusal("default", "shop-payment", &built);
        assert!(refusal.contains("Deploy the whole project (`bynk deploy`)"));
    }

    /// #2077 review: a caller of a context removed from the project is still
    /// live in the ledger, and still refused — but a whole-project deploy
    /// would neither move nor check it, so its refusal says to retire it (or
    /// restore its context), never just to deploy the whole project.
    #[test]
    fn an_orphaned_caller_is_refused_with_the_orphan_remedy() {
        let mut lock = live_shop(Some("febd15c2757b72e1"));
        lock.record_deployed("staging", "shop-payment", Some(BTreeMap::new()));
        lock.record_deployed("staging", "shop-legacy", Some(BTreeMap::new()));
        lock.record_expects(
            "staging",
            "shop-legacy",
            expects("shop.payment", "authorise", "febd15c2757b72e1"),
        );
        let found = callee_contract_skews(
            "shop-payment",
            &provides("authorise", "da0d725f3a004040"),
            &lock,
            worker_of,
            "staging",
        );
        // This build has orders and payment; `shop-legacy` is gone.
        let bindings = graph(&[("shop-orders", &["shop-payment"]), ("shop-payment", &[])]);
        let (built, orphaned) = split_orphaned_callers(found, &bindings);
        assert!(built.is_empty(), "{built:?}");
        assert_eq!(orphaned.len(), 1);
        assert_eq!(orphaned[0].caller, "shop-legacy");
        let refusal = orphaned_caller_skew_refusal("staging", "shop-payment", &orphaned);
        assert!(refusal.contains(CONTRACT_SKEW), "{refusal}");
        assert!(
            refusal.contains("`shop-legacy-staging` (orphaned) calls shop.payment.authorise"),
            "{refusal}"
        );
        assert!(
            refusal.contains("`npx wrangler delete --name shop-legacy-staging`")
                && refusal.contains("`[environments.staging.workers.shop-legacy]`")
                && refusal.contains("`bynk deploy --env staging`"),
            "{refusal}"
        );
    }

    #[test]
    fn a_callee_that_still_provides_the_live_hash_passes() {
        let lock = live_shop(Some("febd15c2757b72e1"));
        let found = callee_contract_skews(
            "shop-payment",
            &provides("authorise", "febd15c2757b72e1"),
            &lock,
            worker_of,
            "default",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_service_a_live_caller_still_calls_but_the_callee_removed_is_refused() {
        let lock = live_shop(Some("febd15c2757b72e1"));
        let found = callee_contract_skews(
            "shop-payment",
            &BTreeMap::new(),
            &lock,
            worker_of,
            "default",
        );
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].provided, "<absent>");
    }

    #[test]
    fn a_pushed_worker_is_never_held_to_its_own_stale_record() {
        // The gate is a `--context` gate, as D4 is: `run` reaches it only for
        // a single selected context, so a whole-project run (which redeploys
        // the callers beside the callee) is never refused by it — the
        // binary-driven `deploy_env` test pins that. Here: a pushed caller's
        // own recorded `expects` is being replaced, so it is no finding even
        // when the pushed Worker's contract changed.
        let mut lock = live_shop(Some("febd15c2757b72e1"));
        lock.record_expects(
            "default",
            "shop-orders",
            expects("shop.orders", "place", "0000000000000000"),
        );
        let found =
            callee_contract_skews("shop-orders", &BTreeMap::new(), &lock, worker_of, "default");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_caller_recorded_before_expects_is_not_refused_but_is_named_for_a_warning() {
        // An old lock: orders is deployed, but the ledger never recorded what
        // it expects. Silence is not a match, so no refusal; orders binds to
        // payment in this build, so it is named for the warning.
        let lock = live_shop(None);
        let found = callee_contract_skews(
            "shop-payment",
            &provides("authorise", "da0d725f3a004040"),
            &lock,
            worker_of,
            "default",
        );
        assert!(found.is_empty(), "{found:?}");
        let bindings = graph(&[("shop-orders", &["shop-payment"]), ("shop-payment", &[])]);
        assert_eq!(
            unrecorded_callers("shop-payment", &bindings, &lock, "default"),
            names(&["shop-orders"])
        );
        // A Worker that does not bind to payment is no caller of it.
        let unrelated = graph(&[("shop-orders", &[]), ("shop-payment", &[])]);
        assert!(unrecorded_callers("shop-payment", &unrelated, &lock, "default").is_empty());
        // And once orders' expectations are recorded, there is nothing to warn of.
        let recorded = live_shop(Some("febd15c2757b72e1"));
        assert!(unrecorded_callers("shop-payment", &bindings, &recorded, "default").is_empty());
    }

    #[test]
    fn a_caller_that_expects_nothing_of_the_callee_is_not_a_finding() {
        let mut lock = live_shop(None);
        lock.record_expects("default", "shop-orders", BTreeMap::new());
        let found = callee_contract_skews(
            "shop-payment",
            &BTreeMap::new(),
            &lock,
            worker_of,
            "default",
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_caller_live_in_another_environment_is_not_this_environments_caller() {
        let lock = live_shop(Some("febd15c2757b72e1"));
        let found = callee_contract_skews(
            "shop-payment",
            &provides("authorise", "da0d725f3a004040"),
            &lock,
            worker_of,
            "staging",
        );
        assert!(found.is_empty(), "{found:?}");
    }
    // ---- #2485: classifying a skew -------------------------------------------

    mod classified {
        use super::*;
        use bynk_check::contract_compat::{Field, Shape, Verdict};

        /// `charge(order: Order) -> { ok: Bool }`, with `extra` as additional
        /// fields of the response.
        fn charge(extra: &[(&str, Shape)]) -> ServiceShape {
            let mut fields = vec![Field {
                name: "ok".into(),
                ty: Shape::Base("Bool".into()),
                preds: Vec::new(),
            }];
            fields.extend(extra.iter().map(|(n, t)| Field {
                name: n.to_string(),
                ty: t.clone(),
                preds: Vec::new(),
            }));
            fields.sort_by_key(|f| f.name.clone());
            ServiceShape {
                name: "charge".into(),
                params: vec![("order".into(), Shape::Base("Int".into()))],
                ret: Shape::Record {
                    name: "Receipt".into(),
                    fields,
                },
            }
        }

        fn lock_providing(shape: &ServiceShape) -> DeployLock {
            let mut lock = DeployLock::default();
            lock.record_deployed(
                "default",
                "shop-payment",
                Some(BTreeMap::from([("charge".into(), shape.hash())])),
            );
            lock.record_shapes(
                "default",
                "shop-payment",
                BTreeMap::from([("charge".into(), shape.to_text())]),
                BTreeMap::new(),
            );
            lock
        }

        fn worker_of(dep: &str) -> String {
            dep.replace('.', "-")
        }

        #[test]
        fn a_caller_skew_with_both_shapes_is_classified() {
            let live = charge(&[]);
            let compiled = charge(&[("at", Shape::Base("Int".into()))]);
            let lock = lock_providing(&live);
            let expects = BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), compiled.hash())]),
            )]);
            let mut skews = contract_skews(&expects, &lock, worker_of, "default");
            assert_eq!(skews.len(), 1);
            let shapes = BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), compiled.to_text())]),
            )]);
            classify_contract_skews(&mut skews, &shapes, &lock, worker_of, "default");
            // The caller was built expecting an extra response field the live
            // callee does not send: it cannot read that answer.
            let c = skews[0].compat.as_ref().expect("classified");
            assert_eq!(c.verdict, Verdict::Breaking, "{c:#?}");
            let refusal = contract_skew_refusal("default", "shop-orders", &skews);
            assert!(refusal.contains("breaking response.at"), "{refusal}");
        }

        #[test]
        fn a_callee_skew_that_only_adds_a_response_field_is_compatible() {
            let old = charge(&[]);
            let new = charge(&[("at", Shape::Base("Int".into()))]);
            let mut lock = DeployLock::default();
            lock.record_deployed("default", "shop-orders", Some(BTreeMap::new()));
            lock.record_expects(
                "default",
                "shop-orders",
                BTreeMap::from([(
                    "shop.payment".to_string(),
                    BTreeMap::from([("charge".to_string(), old.hash())]),
                )]),
            );
            lock.record_shapes(
                "default",
                "shop-orders",
                BTreeMap::new(),
                BTreeMap::from([(
                    "shop.payment".to_string(),
                    BTreeMap::from([("charge".to_string(), old.to_text())]),
                )]),
            );
            let provides = BTreeMap::from([("charge".to_string(), new.hash())]);
            let mut skews =
                callee_contract_skews("shop-payment", &provides, &lock, worker_of, "default");
            assert_eq!(skews.len(), 1);
            classify_callee_skews(
                &mut skews,
                &BTreeMap::from([("charge".to_string(), new.to_text())]),
                &lock,
                "default",
            );
            let c = skews[0].compat.as_ref().expect("classified");
            assert_eq!(c.verdict, Verdict::Compatible, "{c:#?}");
            let refusal = callee_contract_skew_refusal("default", "shop-payment", &skews);
            assert!(refusal.contains("compatible response.at"), "{refusal}");
            assert!(
                refusal.contains("A compatible change can be accepted"),
                "{refusal}"
            );
        }

        // ---- #2554: accepting a compatible callee-ahead skew ----------------

        /// A live `shop-orders` compiled against `old`, and the callee's new
        /// build providing `new`: the one callee-ahead skew, classified.
        fn callee_skew(old: &ServiceShape, new: &ServiceShape) -> (DeployLock, Vec<CalleeSkew>) {
            let mut lock = DeployLock::default();
            lock.record_deployed("default", "shop-orders", Some(BTreeMap::new()));
            lock.record_expects(
                "default",
                "shop-orders",
                BTreeMap::from([(
                    "shop.payment".to_string(),
                    BTreeMap::from([("charge".to_string(), old.hash())]),
                )]),
            );
            lock.record_shapes(
                "default",
                "shop-orders",
                BTreeMap::new(),
                BTreeMap::from([(
                    "shop.payment".to_string(),
                    BTreeMap::from([("charge".to_string(), old.to_text())]),
                )]),
            );
            let provides = BTreeMap::from([("charge".to_string(), new.hash())]);
            let mut skews =
                callee_contract_skews("shop-payment", &provides, &lock, worker_of, "default");
            classify_callee_skews(
                &mut skews,
                &BTreeMap::from([("charge".to_string(), new.to_text())]),
                &lock,
                "default",
            );
            (lock, skews)
        }

        #[test]
        fn a_compatible_skew_is_accepted_only_once_the_build_carries_its_hash() {
            let old = charge(&[]);
            let new = charge(&[("at", Shape::Base("Int".into()))]);
            let (lock, skews) = callee_skew(&old, &new);

            // Before the lock is recorded the build accepts nothing: the skew
            // is refused, and the shape to record is the caller's.
            let none = BTreeMap::new();
            let wanted = shapes_to_accept(&skews, &none, &lock, "default");
            assert_eq!(
                wanted,
                [(
                    "shop.payment".to_string(),
                    "charge".to_string(),
                    old.hash(),
                    old.to_text()
                )]
            );
            let (accepted, refused) = partition_accepted(skews.clone(), &none);
            assert!((accepted.len(), refused.len()) == (0, 1));

            // Once the rebuilt callee lists the hash, it is answered, and
            // nothing more needs recording.
            let carries = BTreeMap::from([("charge".to_string(), vec![old.hash()])]);
            assert!(shapes_to_accept(&skews, &carries, &lock, "default").is_empty());
            let (accepted, refused) = partition_accepted(skews, &carries);
            assert!((accepted.len(), refused.len()) == (1, 0));
        }

        #[test]
        fn a_breaking_skew_is_never_accepted_whatever_the_build_carries() {
            let old = charge(&[("at", Shape::Base("Int".into()))]);
            let new = charge(&[]);
            let (lock, skews) = callee_skew(&old, &new);
            assert_eq!(skews[0].compat.as_ref().unwrap().verdict, Verdict::Breaking);
            // Even if a hand-edited lock listed the hash, the verdict stands.
            let carries = BTreeMap::from([("charge".to_string(), vec![old.hash()])]);
            assert!(shapes_to_accept(&skews, &carries, &lock, "default").is_empty());
            let (accepted, refused) = partition_accepted(skews, &carries);
            assert!((accepted.len(), refused.len()) == (0, 1));
        }

        #[test]
        fn an_unclassified_skew_is_never_accepted() {
            let old = charge(&[]);
            let new = charge(&[("at", Shape::Base("Int".into()))]);
            let (lock, mut skews) = callee_skew(&old, &new);
            skews[0].compat = None;
            let carries = BTreeMap::from([("charge".to_string(), vec![old.hash()])]);
            assert!(shapes_to_accept(&skews, &carries, &lock, "default").is_empty());
            assert!(partition_accepted(skews, &carries).0.is_empty());
        }

        #[test]
        fn the_notice_says_what_was_recorded_and_what_a_dry_run_did_not() {
            let old = charge(&[]);
            let new = charge(&[("at", Shape::Base("Int".into()))]);
            let (_, skews) = callee_skew(&old, &new);
            let real = accepted_skew_notice("default", "shop-payment", &skews, true, false);
            assert!(
                real.contains("will also answer 1 live caller contract that is compatible"),
                "{real}"
            );
            assert!(
                real.contains("`shop-orders` calls shop.payment.charge"),
                "{real}"
            );
            assert!(real.contains("Recorded in bynk.contracts.lock"), "{real}");
            let dry = accepted_skew_notice("default", "shop-payment", &skews, false, true);
            assert!(dry.contains("Nothing was written"), "{dry}");
            assert!(!dry.contains("Recorded in"), "{dry}");
        }

        #[test]
        fn a_caller_ahead_of_its_callee_is_told_to_deploy_the_callee_first() {
            // The new caller expects the old contract; the live callee already
            // sends more: compatible for the caller, yet the callee still
            // refuses a hash it does not know.
            let caller = charge(&[]);
            let live = charge(&[("at", Shape::Base("Int".into()))]);
            let lock = lock_providing(&live);
            let expects = BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), caller.hash())]),
            )]);
            let mut skews = contract_skews(&expects, &lock, worker_of, "default");
            classify_contract_skews(
                &mut skews,
                &BTreeMap::from([(
                    "shop.payment".to_string(),
                    BTreeMap::from([("charge".to_string(), caller.to_text())]),
                )]),
                &lock,
                worker_of,
                "default",
            );
            assert_eq!(
                skews[0].compat.as_ref().unwrap().verdict,
                Verdict::Compatible
            );
            let refusal = contract_skew_refusal("default", "shop-orders", &skews);
            assert!(
                refusal.contains("A compatible change is still refused here"),
                "{refusal}"
            );
            assert!(
                refusal.contains("Deploy `shop.payment` ahead of this caller"),
                "{refusal}"
            );
        }

        #[test]
        fn the_lock_entries_nothing_live_needs_are_reported() {
            let old = charge(&[]);
            let newer = charge(&[("at", Shape::Base("Int".into()))]);
            let unused_one = charge(&[("by", Shape::Base("Int".into()))]);
            let (lock, _) = callee_skew(&old, &newer);
            let mut contracts = bynk_project::contracts_lock::ContractsLock::new();
            contracts.accept("shop.payment", "charge", &old.to_text());
            contracts.accept("shop.payment", "charge", &unused_one.to_text());
            let unused = unused_accepted(&contracts, "shop-payment", &lock, "default");
            assert_eq!(
                unused,
                [(
                    "shop.payment".to_string(),
                    "charge".to_string(),
                    unused_one.hash()
                )],
                "the contract shop-orders was compiled against is still needed"
            );
            assert!(unused_accepted(&contracts, "shop-orders", &lock, "default").is_empty());
            assert!(unused_accepted_notice(&unused).contains("shop.payment.charge"));
        }

        #[test]
        fn a_mixed_run_accepts_the_compatible_callers_and_refuses_the_rest() {
            // Two live callers of one callee: `shop-orders` on a contract the
            // new build stays compatible with, `shop-audit` on one it breaks.
            let compatible_old = charge(&[]);
            let breaking_old = charge(&[("by", Shape::Base("Int".into()))]);
            let new = charge(&[("at", Shape::Base("Int".into()))]);
            let mut lock = DeployLock::default();
            for (caller, old) in [
                ("shop-orders", &compatible_old),
                ("shop-audit", &breaking_old),
            ] {
                lock.record_deployed("default", caller, Some(BTreeMap::new()));
                lock.record_expects(
                    "default",
                    caller,
                    BTreeMap::from([(
                        "shop.payment".to_string(),
                        BTreeMap::from([("charge".to_string(), old.hash())]),
                    )]),
                );
                lock.record_shapes(
                    "default",
                    caller,
                    BTreeMap::new(),
                    BTreeMap::from([(
                        "shop.payment".to_string(),
                        BTreeMap::from([("charge".to_string(), old.to_text())]),
                    )]),
                );
            }
            let provides = BTreeMap::from([("charge".to_string(), new.hash())]);
            let mut skews =
                callee_contract_skews("shop-payment", &provides, &lock, worker_of, "default");
            classify_callee_skews(
                &mut skews,
                &BTreeMap::from([("charge".to_string(), new.to_text())]),
                &lock,
                "default",
            );
            assert_eq!(skews.len(), 2);

            // Only the compatible caller's contract is worth recording.
            let none = BTreeMap::new();
            let wanted = shapes_to_accept(&skews, &none, &lock, "default");
            assert_eq!(
                wanted.iter().map(|w| w.2.as_str()).collect::<Vec<_>>(),
                [compatible_old.hash().as_str()]
            );

            // With the build carrying exactly that hash, one is answered and the
            // breaking one is still refused.
            let carries = BTreeMap::from([("charge".to_string(), vec![compatible_old.hash()])]);
            let (accepted, refused) = partition_accepted(skews, &carries);
            assert_eq!(
                accepted
                    .iter()
                    .map(|s| s.caller.as_str())
                    .collect::<Vec<_>>(),
                ["shop-orders"]
            );
            assert_eq!(
                refused
                    .iter()
                    .map(|s| s.caller.as_str())
                    .collect::<Vec<_>>(),
                ["shop-audit"]
            );
        }

        #[test]
        fn a_skew_whose_shapes_are_equal_is_not_accepted() {
            // Different hashes but the same structure cannot happen for a real
            // build; if a ledger ever says so, the gate refuses rather than
            // guess. `Unchanged` is not `Compatible`.
            let shape = charge(&[]);
            let (lock, mut skews) =
                callee_skew(&shape, &charge(&[("at", Shape::Base("Int".into()))]));
            skews[0].compat = Some(check_call(&shape, &shape));
            assert_eq!(
                skews[0].compat.as_ref().unwrap().verdict,
                Verdict::Unchanged
            );
            let carries = BTreeMap::from([("charge".to_string(), vec![shape.hash()])]);
            assert!(shapes_to_accept(&skews, &carries, &lock, "default").is_empty());
            assert!(partition_accepted(skews, &carries).0.is_empty());
        }

        #[test]
        fn a_missing_or_drifted_shape_leaves_the_skew_unclassified() {
            let live = charge(&[]);
            let compiled = charge(&[("at", Shape::Base("Int".into()))]);
            let lock = lock_providing(&live);
            let expects = BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), compiled.hash())]),
            )]);
            // No shapes from the build (an older compiler).
            let mut skews = contract_skews(&expects, &lock, worker_of, "default");
            classify_contract_skews(&mut skews, &BTreeMap::new(), &lock, worker_of, "default");
            assert_eq!(skews[0].compat, None);
            // A shape that does not hash to the hash it sits beside.
            let mut skews = contract_skews(&expects, &lock, worker_of, "default");
            let wrong = BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), live.to_text())]),
            )]);
            classify_contract_skews(&mut skews, &wrong, &lock, worker_of, "default");
            assert_eq!(skews[0].compat, None);
            // A ledger holding only hashes.
            let mut hashes_only = DeployLock::default();
            hashes_only.record_deployed(
                "default",
                "shop-payment",
                Some(BTreeMap::from([("charge".into(), live.hash())])),
            );
            let mut skews = contract_skews(&expects, &hashes_only, worker_of, "default");
            let shapes = BTreeMap::from([(
                "shop.payment".to_string(),
                BTreeMap::from([("charge".to_string(), compiled.to_text())]),
            )]);
            classify_contract_skews(&mut skews, &shapes, &hashes_only, worker_of, "default");
            assert_eq!(skews[0].compat, None);
            let refusal = contract_skew_refusal("default", "shop-orders", &skews);
            assert!(!refusal.contains("breaking") && !refusal.contains("compatible"));
        }
    }
}
