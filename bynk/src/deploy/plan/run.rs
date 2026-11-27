use std::collections::BTreeSet;

use super::model::GroupNames;
use super::model::account_name_problem;
use super::model::confirm;
use super::model::confirm_destructive;
use super::model::derive_plan;
use super::model::recreate_refusal;
use super::shared::{
    SharedWithDefault, comparison_skipped, has_comparable_bindings, shared_bindings,
    shared_constants, strict_refusal, without_deliberate,
};
use super::{Entry, Stop, code, refusals, refused_report};
use crate::cloudflare::{Account, Wrangler};
use crate::deploy::LOCK_FILE;
use crate::deploy::agent_move::{AgentGate, checked_agent_moves};
use crate::deploy::apply::LedgerWriter;
use crate::deploy::apply::PlanApply;
use crate::deploy::apply::apply_plan;
use crate::deploy::apply::prune_plan;
use crate::deploy::config::Resources;
use crate::deploy::config::deployed_worker_name;
use crate::deploy::config::project_resources;
use crate::deploy::config::read_contracts_manifest;
use crate::deploy::config::script_names;
use crate::deploy::containers::apply_adoptions;
use crate::deploy::destinations::print_plan_then_forward_warnings;
use crate::deploy::gates::AccountChecked;
use crate::deploy::gates::AccountCx;
use crate::deploy::gates::ContextGate;
use crate::deploy::gates::KeysCx;
use crate::deploy::gates::account_checks;
use crate::deploy::gates::check_foreign_pins;
use crate::deploy::gates::context_gate;
use crate::deploy::gates::decide_keys;
use crate::deploy::gates::ordered;
use crate::deploy::gates::preflight;
use crate::deploy::graph::service_graph;
use crate::deploy::hyperdrive;
use crate::deploy::kind::Kv;
use crate::deploy::kind::LedgerView;
use crate::deploy::kind::Lives;
use crate::deploy::kind::ProjectCx;
use crate::deploy::kind::Tools;
use crate::deploy::ledger::DeployLock;
use crate::deploy::ledger::UNRESTRICTED_AGENT;
use crate::deploy::ledger::read_lock;
use crate::deploy::provisioning::secrets_file_supported;
use crate::deploy::provisioning::sweep_stale_upload_dirs;
use crate::deploy::secrets::SecretSource;
use crate::deploy::secrets::wrangler_runner_in;
use crate::probe::Toolbox;
use crate::workers;
use bynk_driver::{colour, emsg};
use std::path::Path;
use std::process::ExitCode;

/// Where a deploy builds, ready to write into. A real deploy keeps its build
/// under `.bynk/deploy`; a dry run builds in a scratch directory, removed when
/// the returned guard is dropped, so it writes nothing to the project (#3796).
/// `None` once the failure has been reported.
fn build_dir_for(
    project_root: &Path,
    dry_run: bool,
) -> Option<(std::path::PathBuf, Option<workers::ScratchBuildDir>)> {
    let prepared = if dry_run {
        workers::ScratchBuildDir::new().map(|s| (s.path().to_path_buf(), Some(s)))
    } else {
        let build_dir = project_root.join(".bynk").join("deploy");
        workers::prepare_build_dir(project_root, &build_dir).map(|()| (build_dir, None))
    };
    prepared
        .map_err(|e| emsg!("bynk: error: could not prepare build directory: {e}"))
        .ok()
}

/// The rebuild after recording older contracts: the same build as the first,
/// so `--no-install` and `--dry-run` apply to it as well (#3350).
pub(crate) fn rebuild(project_root: &Path, build_dir: &Path, opts: &DeployOptions) -> bool {
    let build = workers::Build::new(true, opts.dry_run)
        .no_install(opts.no_install)
        .environment(&opts.environment);
    workers::compile_once_reporting(project_root, build_dir, build, None)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum DeployFormat {
    #[default]
    Short,
    Json,
}

#[derive(Debug, Clone, Default)]
pub struct DeployOptions {
    pub dry_run: bool,
    /// `--no-install` (or `BYNK_NO_INSTALL`): stop on a missing npm package
    /// instead of installing it (#3350).
    pub no_install: bool,
    /// `--strict-environments` (#3941): refuse a named environment whose
    /// `env(…)` constants fall back to `[vars]` or their default.
    pub strict_environments: bool,
    pub format: DeployFormat,
    pub yes: bool,
    /// `--context NAME` — deploy this context alone, assuming the contexts it
    /// consumes are already live (slice 2, D4). Absent deploys the whole
    /// project in dependency order.
    pub context: Option<String>,
    /// #3562: `context` was given as `--worker NAME`, a Worker directory (a group's
    /// key) and not a context's name.
    pub by_worker: bool,
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
    /// #4526: `--revoke-previous-edge-key <context>` (repeatable): re-key each edge key on this
    /// context with no grace window, so the key it replaces stops verifying at once.
    pub revoke_edge_keys: Vec<String>,
    /// `--prune` (slice 5) — delete every reported KV/queue orphan, behind
    /// its own confirmation. Never deletes a Worker (DECISION C). Defaults
    /// to report-only, matching the track's "report or prevent, never
    /// silently share/destroy" posture (§6).
    pub prune: bool,
    /// State-migrations S3 (#2248): `--confirm-delete <context>.<Class>` —
    /// each Durable Object class whose data a `removed agent` deletes, named
    /// back. `--yes` never stands in for it (Decision E).
    pub confirm_delete: Vec<String>,
    /// #3819: `--confirm-jurisdiction <context>.<Class>` — each agent class whose
    /// `@jurisdiction` changes since its last push, named back. A change mints different
    /// Durable Object ids and orphans every instance, so `--yes` never stands in for it.
    pub confirm_jurisdiction: Vec<String>,
    /// #2554: `--accept-compatible-skew` — with `--context`, deploy a callee
    /// ahead of live callers still on an older contract when each such change
    /// is compatible for them. Records the older contracts in
    /// `bynk.contracts.lock` and rebuilds, so the callee answers them.
    pub accept_compatible_skew: bool,
    /// #3562 (G5): `--accept-fresh-agent <context>` (repeatable): a context with agents that
    /// moves to another Worker script starts fresh, its old state staying in the old script.
    pub accept_fresh_agent: Vec<String>,
    /// #3565: `--move-agent <context>` (repeatable): a context with agents that moves to another
    /// Worker script keeps its state, moved in phases (`design/tracks/agent-context-moves.md`).
    pub move_agent: Vec<String>,
    pub wrangler_args: Vec<String>,
}

/// What the ledger learns from a successful push of `worker` (whose build
/// directory is `dir`), from the build's contract manifest:
///
/// - v0.177 (#643): what it now *provides*, so a later `--context` push of one of
///   its callers can be refused before it ships a caller that would 409. `Some`
///   even when empty: this build *knows* what the Worker provides, and "knows it
///   provides nothing" must not read as "no record" at the next gate.
/// - #2077: what it was compiled against, so a later push of one of its callees
///   can be refused before it ships a callee this Worker would 409 against, and
///   #2485 the structure behind both, so the next gate can say whether a skew is
///   compatible.
/// - I4 (#1970): what its declared foreign callers may call it at, so the ledger
///   knows the consumers no manifest of their own records.
/// - #3562: the contexts of a group's Worker.
pub(crate) fn record_pushed_contracts(
    lock: &mut DeployLock,
    environment: &str,
    (worker, dir): (&str, &Path),
    (members, script): (&[String], &str),
) {
    let manifest = read_contracts_manifest(dir);
    let provided = manifest.as_ref().ok().map(|m| m.provides.clone());
    lock.record_deployed(environment, worker, provided);
    lock.record_members(environment, worker, members);
    lock.record_script(environment, worker, script);
    if let Ok(m) = &manifest {
        lock.record_expects(environment, worker, m.expects.clone());
        lock.record_shapes(m.provides_shapes.clone(), m.expects_shapes.clone());
    }
    lock.record_foreign(
        environment,
        worker,
        manifest.map(|m| m.foreign).unwrap_or_default(),
    );
}

/// The `exports` tombstones a successful push of `worker` carried, recorded
/// so later plans show them as applied: its deletions (State-migrations S3,
/// #2248, #2253 review) and its renames (#2276 Decision D). Each replaces what
/// was recorded for the Worker.
pub(crate) fn record_pushed_tombstones(
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
    // #3819: and the jurisdiction each hosted class's ids were minted under, which the next
    // plan compares against.
    lock.record_agent_jurisdictions(
        environment,
        worker,
        pushed
            .durable_objects
            .iter()
            .map(|object| {
                (
                    object.class.clone(),
                    object
                        .jurisdiction
                        .clone()
                        .unwrap_or_else(|| UNRESTRICTED_AGENT.to_string()),
                )
            })
            .collect(),
    );
    // #3328: and the Email Routing addresses it carried. #3883: an environment that claims
    // none omits `addresses`, which changes no rule, so what was recorded is still live and
    // stays recorded (the plan names it).
    if !pushed.email_unclaimed(environment) {
        lock.record_email_addresses(
            environment,
            worker,
            pushed.email_addresses.iter().cloned().collect(),
        );
    }
}

/// The `--` passthrough argument that conflicts with the driver's own
/// `--env`, if any (slice 4, DECISION E) — bare or `=`-joined, mirroring
/// `dev/run.rs`'s `passthrough_has` matching rule for the same class of clash
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
        "bynk: error: `{arg}` after `--` conflicts with the secrets file `bynk deploy` uploads with a new Worker's first version ({}).\n  \
         Pass it to bynk instead, before `--`: `bynk deploy --secrets-file <file>`.\n",
        first.join(", ")
    ))
}

/// Run the slice-0 single-context deployment pipeline.
pub fn run(
    tb: &dyn Toolbox,
    project_root: &Path,
    node_floor: (u32, u32),
    opts: &DeployOptions,
) -> ExitCode {
    match deploy(tb, project_root, node_floor, opts) {
        Ok(()) => ExitCode::SUCCESS,
        Err(stop) => {
            // #3797: a client that asked for JSON reads the refusals from the document.
            if opts.format == DeployFormat::Json && !stop.refusals.is_empty() {
                print!(
                    "{}",
                    refused_report(
                        &opts.environment,
                        &stop.order,
                        &stop.refusals,
                        &stop.warnings
                    )
                );
            }
            stop.code
        }
    }
}

/// [`run`]'s orchestration (1D.2c, #4001 Decision E): the gates, the plan, the confirmation, the
/// apply and `--prune`, each a step that returns the code the run exits with when it stops.
fn deploy(
    tb: &dyn Toolbox,
    project_root: &Path,
    node_floor: (u32, u32),
    opts: &DeployOptions,
) -> Result<(), Stop> {
    preflight(tb, project_root, node_floor, opts)?;

    // `_scratch` removes a dry run's build when the run ends (#3796).
    let Some((build_dir, _scratch)) = build_dir_for(project_root, opts.dry_run) else {
        return Err(ExitCode::FAILURE.into());
    };
    let Some(model) = workers::compile_built(
        project_root,
        &build_dir,
        workers::Build::new(true, opts.dry_run)
            .no_install(opts.no_install)
            .environment(&opts.environment),
        bynk_driver::progress::terminal_sink(),
    ) else {
        return Err(ExitCode::FAILURE.into());
    };
    // Slice 2: every context, ordered — not the one context slice 0 demanded.
    let workers_dir = build_dir.join("workers");
    let available = workers::discover_workers(&workers_dir);
    let selected = match select_workers(opts, project_root, &available) {
        Ok(selected) => selected,
        Err(entry) => return Err(Stop::error(entry)),
    };
    // Read spans the *whole* project even under `--context`: D4 needs the
    // selected context's binding targets to check they are live, and they are
    // by definition outside the selection.
    let mut resources = match project_resources(&workers_dir, &available, &model) {
        Ok(resources) => resources,
        Err(e) => {
            return Err(Stop::error(refusals::workers_unreadable(&e)));
        }
    };
    // #3883: a named environment claims only the addresses `[deploy.email]` maps it to.
    if let Err(e) = crate::deploy::email::claim(&mut resources, &model.layout, &opts.environment) {
        return Err(Stop::error(refusals::email_claim_refused(
            &opts.environment,
            &e,
        )));
    }
    let (graph, groups) = (service_graph(&resources), GroupNames::of(project_root));
    // Read before the plan: a malformed `--secrets-file` is the user's typo, and
    // it should surface as one now rather than as a missing-secret failure
    // partway through a run that has already pushed a Worker.
    let secret_source = match SecretSource::read(opts) {
        Ok(source) => source,
        Err(e) => {
            return Err(Stop::error(Entry::new(
                code::SECRETS_SOURCE_UNREADABLE,
                e.to_string(),
            )));
        }
    };
    let lock_path = project_root.join(LOCK_FILE);
    let runner = wrangler_runner_in(project_root);
    let mut lock = match read_lock(&lock_path) {
        Ok(lock) => lock,
        Err(e) => {
            return Err(Stop::error(
                Entry::new(
                    code::LEDGER_UNREADABLE,
                    format!("could not read {}: {e}", lock_path.display()),
                )
                .with("path", lock_path.display().to_string()),
            ));
        }
    };

    // (D4) `--context` does not deploy a dependency closure, and a context deployed alone must
    // not skew the contracts of what it calls or what calls it.
    context_gate(
        &ContextGate {
            opts,
            project_root,
            build_dir: &build_dir,
            workers_dir: &workers_dir,
            graph: &graph,
            groups: &groups,
            lock: &lock,
            runner,
        },
        &selected,
    )?;

    let order = ordered(&selected, &graph, &resources)?;
    // #3797: every stop from here on has the upload order, which its document carries.
    let order_names: Vec<String> = order
        .iter()
        .map(|worker| deployed_worker_name(&opts.environment, worker))
        .collect();
    let run_stop = |stop: Stop, warnings: &[Entry]| stop.with_run(&order_names, warnings);

    // #3562 (G5): a context with agent state that would change scripts is refused before
    // anything is pushed, `--dry-run` included.
    // #3565: the Workers that declare routes or custom domains, whose move sets are refused in v1.
    let attached = attached_workers(project_root);
    let agents = checked_agent_moves(&lock, opts, &resources, &order, &groups, &attached);
    agents.eprint();
    if !agents.may_go_on() {
        // #3797: the refusals, with what the run knew, for a client that asked for JSON.
        return Err(Stop {
            order: order_names.clone(),
            refusals: agents.refusals,
            warnings: agents.warnings,
            ..Stop::from(ExitCode::FAILURE)
        });
    }
    let AgentGate {
        moves,
        warnings,
        planned: planned_moves,
        ..
    } = agents;

    check_foreign_pins(project_root, &workers_dir, &order, opts)
        .map_err(|stop| run_stop(stop, &warnings))?;

    // Review of #1974: names the account scopes (a namespace's title, a
    // bucket's, a database's, a dataset's) can coincide, so they are refused
    // before the plan, `--dry-run` included, not met as wrangler's own error.
    // #3479: an index `bynk.toml` names for this environment is adopted, not
    // created, so its name is known before the plan.
    let mut warnings = warnings;
    match apply_adoptions(project_root, &mut resources, &opts.environment) {
        Ok(notes) => warnings.extend(notes),
        Err((notes, refusal)) => {
            warnings.extend(notes);
            return Err(run_stop(Stop::refused(refusal), &warnings));
        }
    }
    if let Some(message) = account_name_problem(&resources, &lock, &opts.environment) {
        return Err(run_stop(
            Stop::error(refusals::account_name_conflict(message)),
            &warnings,
        ));
    }
    // #2055: a first deploy hands `wrangler deploy` Bynk's own
    // `--secrets-file`, so a passthrough one would be a second, with
    // wrangler deciding which wins. Refused before the plan, as `--env`'s
    // clash is.
    if let Some(refusal) =
        secrets_file_passthrough_refusal(&opts.wrangler_args, &order, &lock, &opts.environment)
    {
        colour::eprint(&refusal);
        return Err(run_stop(
            Stop::refused(refusals::secrets_file_passthrough(&refusal)),
            &warnings,
        ));
    }

    // #3319: each `Postgres` binding's connection string is read now, from the
    // environment variable `bynk.toml` names, so a missing variable fails the
    // deploy before the plan (`--dry-run` included) and the plan can show a
    // changed string as an update. The string itself goes no further than the
    // `Wants` below.
    let hyperdrive_wants =
        match hyperdrive::wants_for_run(project_root, &order, &resources, &opts.environment) {
            Ok(wants) => wants,
            Err(refusals) => {
                return Err(Stop {
                    refusals,
                    order: order_names.clone(),
                    warnings: warnings.clone(),
                    ..Stop::from(ExitCode::FAILURE)
                });
            }
        };
    let plan = derive_plan(
        &order,
        &resources,
        &lock,
        &secret_source,
        opts.force,
        &opts.environment,
    )
    .with_lives(
        &order,
        &resources,
        &lock,
        Lives::for_plan(&hyperdrive_wants),
    )
    .checked_against_account()
    .noting_unchecked_kv(!opts.dry_run)
    .with_report(Vec::new(), warnings.clone())
    .with_moves(planned_moves);
    let own_scripts: BTreeSet<String> =
        script_names(&workers_dir, &available).into_keys().collect();
    let shared = shared_with_default(project_root, opts, &model, &own_scripts)
        .map_err(|stop| run_stop(stop, &warnings))?;
    let mut plan = plan.with_shared(shared);
    // P5 (#2323): a changed `@jurisdiction` is a new bucket beside the old,
    // whose objects do not move, so it is only made on an explicit `--yes`. #3797: known before
    // the plan is printed, so the JSON plan lists it; a dry run only shows the plan.
    let recreate = recreate_refusal(&plan, opts.yes).filter(|_| !opts.dry_run);
    if let Some(refusal) = recreate {
        plan = plan.with_refusal(refusals::recreate_needs_yes(refusal));
    }
    let plan = print_plan_then_forward_warnings(plan, opts, &order, &resources, project_root);
    if opts.dry_run {
        return Ok(());
    }
    if let Some(refusal) = recreate {
        emsg!("bynk: error: {refusal}");
        return Err(ExitCode::FAILURE.into());
    }

    let AccountChecked {
        probe,
        linked_stores,
        hyperdrive_api,
    } = account_checks(
        tb,
        &AccountCx {
            project_root,
            opts,
            plan: &plan,
            order: &order,
            resources: &resources,
            lock: &lock,
            hyperdrive_wants: &hyperdrive_wants,
        },
    )?;
    let hyperdrive_run = hyperdrive::HyperdriveRun::new(&hyperdrive_wants, hyperdrive_api.as_ref());
    // State-migrations S3 (#2248): a deletion is confirmed by name, apart
    // from — and before — the ordinary confirmation.
    if !confirm_destructive(&plan, opts, &lock) {
        return Err(ExitCode::FAILURE.into());
    }
    if !confirm(opts.yes) {
        return Err(ExitCode::FAILURE.into());
    }

    let api = Account::new(Wrangler::new(&probe.provenance, project_root));
    let tools = Tools {
        api: &api,
        project_root,
    };
    // Slice 5, DECISION B: the account's live KV ids, read once per run.
    let live_kv = Kv::read_live(
        tools,
        &ProjectCx {
            ledger: LedgerView::of(&lock, &opts.environment),
            resources: &resources,
            order: &order,
        },
    );
    // 1D.1 (#3929): what the run read once, for each kind.
    // Shared across the loop: a queue two contexts consume is one queue, so it
    // wants one create attempt per run, not one per consumer (ADR 0194 D2).
    let attempted_queues = std::cell::RefCell::new(BTreeSet::new());
    let lives = Lives {
        kv: live_kv.as_ref(),
        hyperdrive: hyperdrive_run,
        queues: Some(&attempted_queues),
        linked: Some(&linked_stores),
    };

    // Provision → wire → push, per context, in dependency order. Each context's
    // state is written to the ledger as it lands (ADR 0180's incremental
    // posture), so an interrupted multi-context run is resumable rather than
    // restartable — and never rolled back (D2): a half-deployed project is a
    // real state the next plan will show, not an error to unwind.
    let keys = decide_keys(
        &KeysCx {
            order: &order,
            resources: &resources,
            api: &api,
            workers_dir: &workers_dir,
            lock: &lock,
            opts,
        },
        &secret_source,
    )?;
    sweep_and_report_stale_uploads();
    let mut writer = LedgerWriter::new(&mut lock, &lock_path);
    let apply = PlanApply {
        plan: &plan,
        order: &order,
        opts,
        tools,
        workers_dir: &workers_dir,
        resources: &resources,
        groups: &groups,
        moves: &moves,
        keys: &keys,
        secret_source: &secret_source,
        with_upload: secrets_file_supported(&probe),
        runner,
        lives,
    };
    apply_plan(&apply, &mut writer)?;
    prune_plan(&apply, &mut writer)?;
    Ok(())
}

/// #2055: a first deploy's secrets file that a SIGKILLed run left behind goes
/// before this run might write one of its own.
/// #3565 (D12): each `[deploy.workers.<key>]` Worker that declares `routes` or `custom_domains`,
/// with the first key it declares. A manifest that does not read has no groups here (the build
/// has already reported it).
fn attached_workers(project_root: &Path) -> std::collections::BTreeMap<String, &'static str> {
    bynk_project::worker_groups::try_read_worker_groups(project_root)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(key, group)| {
            if !group.routes.is_empty() {
                Some((key, "routes"))
            } else if !group.custom_domains.is_empty() {
                Some((key, "custom_domains"))
            } else {
                None
            }
        })
        .collect()
}

fn sweep_and_report_stale_uploads() {
    let stale_removed = sweep_stale_upload_dirs(&std::env::temp_dir());
    if stale_removed > 0 {
        emsg!(
            "bynk: removed {stale_removed} secrets file(s) an interrupted deploy left in the temp directory"
        );
    }
}

/// The Workers a run deploys: every one, or the one `--context` or `--worker`
/// names (#3562). A group is one Worker, so a member cannot deploy alone: naming
/// one says which Worker holds it.
fn select_workers(
    opts: &DeployOptions,
    project_root: &Path,
    available: &[String],
) -> Result<Vec<String>, Entry> {
    // #3797: the name a refusal is about, for the entries that have one.
    let not_found = |message: String| {
        Entry::new(code::CONTEXT_NOT_FOUND, message).with("requested", opts.context.clone())
    };
    let Some(name) = opts.context.as_deref().filter(|_| opts.by_worker) else {
        let found = workers::select_contexts(available, opts.context.as_slice());
        return found.map_err(|e| match (e, &opts.context) {
            (workers::SelectError::NotFound { .. }, Some(context)) => {
                match GroupNames::of(project_root).holding(context) {
                    Some(worker) => not_found(format!(
                        "context `{context}` is in Worker `{worker}`, which deploys as a unit: use `--worker {worker}`"
                    ))
                    .with("worker", worker),
                    None => not_found(
                        workers::SelectError::NotFound {
                            requested: context.clone(),
                            available: available.to_vec(),
                        }
                        .to_string(),
                    ),
                }
            }
            // An empty build is no `--context` problem: it has its own code.
            (e @ workers::SelectError::NoneBuilt, _) => {
                Entry::new(code::NO_WORKERS_BUILT, e.to_string())
            }
            (e, _) => not_found(e.to_string()),
        });
    };
    if available.iter().any(|worker| worker == name) {
        return Ok(vec![name.to_string()]);
    }
    Err(not_found(format!(
        "no Worker `{name}` was built (available: {})",
        available.join(", ")
    )))
}

/// #3941 decision H: each `env(…)` constant with its resolved value and where it
/// came from, ahead of the plan, on stderr (the plan's own stdout, `--format
/// json` included, is untouched by these lines). Empty when there are none.
pub(crate) fn env_constant_lines(
    facts: &[bynk_project::vars::ResolvedConst],
    environment: &str,
) -> Vec<String> {
    if facts.is_empty() {
        return Vec::new();
    }
    let environment = if environment.is_empty() {
        bynk_project::vars::DEFAULT_ENVIRONMENT
    } else {
        environment
    };
    let mut lines = vec![format!("Environment constants for `{environment}`:")];
    for f in facts {
        lines.push(format!(
            "  {} = {}   ({} from {})",
            f.constant,
            f.value,
            f.variable,
            f.source.label()
        ));
    }
    lines
}

/// #3941, #3943: what a named environment takes from `default`: the `env(…)`
/// constants it leaves to `[vars]` or their default, and the `Service @worker`
/// and `Email @to` / `@allowedRecipients` values equal to `default`'s. Prints the
/// constants' lines; the sharing itself goes into the plan (text and JSON). `Err`
/// when `--strict-environments` refuses it, with the reason printed.
fn shared_with_default(
    project_root: &Path,
    opts: &DeployOptions,
    model: &bynk_emit::emit_model::WorkersBuild,
    own_scripts: &BTreeSet<String>,
) -> Result<Vec<SharedWithDefault>, Stop> {
    use bynk_project::vars::DEFAULT_ENVIRONMENT;
    let Ok(options) = bynk_driver::try_project_options(project_root) else {
        return Ok(Vec::new());
    };
    let environment = if opts.environment.is_empty() {
        DEFAULT_ENVIRONMENT
    } else {
        opts.environment.as_str()
    };
    let facts = bynk_driver::project::env_const_report(&options.clone().environment(environment));
    for line in env_constant_lines(&facts, environment) {
        emsg!("{line}");
    }
    let mut shared = shared_constants(&facts, environment);
    // `default` is built only when there is something to compare it with.
    if environment != DEFAULT_ENVIRONMENT
        && has_comparable_bindings(model.bindings.by_context(), own_scripts)
    {
        // The same build for `default`, in memory: its bindings are what the
        // environment's are compared with.
        let default_build = options
            .environment(DEFAULT_ENVIRONMENT)
            .target(bynk_driver::project::BuildTarget::Workers)
            .tests(false);
        let built = bynk_driver::project::compile_project(&default_build)
            .map_err(|failure| {
                failure.errors.first().map_or_else(
                    || "it has errors".to_string(),
                    |e| format!("{}: {}", e.error.category, e.error.message),
                )
            })
            .and_then(|out| {
                out.workers
                    .ok_or_else(|| "it builds no Workers".to_string())
            });
        match built {
            Ok(default_model) => shared.extend(without_deliberate(
                shared_bindings(
                    model.bindings.by_context(),
                    default_model.bindings.by_context(),
                    own_scripts,
                ),
                &facts,
                environment,
            )),
            Err(cause) => {
                // Said, not skipped quietly: a strict run must not pass unchecked.
                let (message, refuse) =
                    comparison_skipped(environment, opts.strict_environments, &cause);
                if refuse {
                    emsg!("{message}");
                    return Err(Stop::refused(refusals::strict_environments_unchecked(
                        &message,
                        environment,
                    )));
                }
                emsg!("{message}");
            }
        }
    }
    if opts.strict_environments
        && let Some(refusal) = strict_refusal(&shared, environment)
    {
        emsg!("{refusal}");
        let mut names: Vec<String> = Vec::new();
        for s in &shared {
            if !names.contains(&s.name) {
                names.push(s.name.clone());
            }
        }
        return Err(Stop::refused(refusals::strict_environments_shared(
            &refusal,
            environment,
            &names,
        )));
    }
    Ok(shared)
}
