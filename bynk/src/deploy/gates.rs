//! Back-half paydown 1D.2c (#4001 Decision E): the checks `plan::run` makes before it applies,
//! moved out of it unchanged so that `run` reads as orchestration: the `--context` contract-skew
//! gate, the upload order and its cycle report, the foreign callers' pins, and the edge and
//! managed keys. Each returns `Err` with the code `run` exits with, after printing what it always
//! printed, in the order it always printed it.

use super::*;
use bynk_driver::{colour, emsg};

/// Slice 4 (DECISION E), and the first check of all: once `--env` is a
/// real, driver-curated concept, a conflicting `-- --env`/`-- --environment`
/// would otherwise reach `wrangler deploy` as a second, contradictory flag —
/// Wrangler's own last-wins parsing deciding silently which one actually
/// deploys, while the ledger records the driver's choice regardless. Reject
/// before any other work, rather than pick a winner between two explicit,
/// conflicting inputs.
/// Then the project's own refusal of a deployment, and the doctor's deploy checks.
pub(crate) fn preflight(
    tb: &dyn Toolbox,
    project_root: &Path,
    node_floor: (u32, u32),
    opts: &DeployOptions,
) -> Result<(), Stop> {
    if let Some(conflict) = conflicting_env_passthrough(&opts.wrangler_args) {
        return Err(Stop::error(
            Entry::new(
                code::ENV_PASSTHROUGH_CONFLICT,
                format!(
                    "`--env {}` conflicts with `{conflict}` after `--` — pass one or the other, not both",
                    opts.environment
                ),
            )
            .with("environment", opts.environment.as_str())
            .with("flag", conflict),
        ));
    }

    // #3107: no contract checks in a deployment; #3562: no Worker groups yet.
    if let Err(e) = bynk_driver::refuse_undeployable_project(project_root) {
        return Err(Stop::error(Entry::new(code::PROJECT_UNDEPLOYABLE, e)));
    }

    let preflight_opts = DoctorOptions {
        only: Some(Capability::Deploy),
        strict: false,
    };
    let report = doctor::diagnose(
        tb,
        &Context {
            project_root: Some(project_root.to_path_buf()),
            in_repo: false,
            node_floor,
        },
        &preflight_opts,
    );
    if report.exit_nonzero(&preflight_opts) {
        return Err(Stop::printed(
            code::PREFLIGHT_FAILED,
            &preflight_failure_message(&report),
            &[],
        ));
    }
    Ok(())
}

/// What the `--context` gate reads ([`context_gate`]).
pub(crate) struct ContextGate<'a> {
    pub(crate) opts: &'a DeployOptions,
    pub(crate) project_root: &'a Path,
    pub(crate) build_dir: &'a Path,
    pub(crate) workers_dir: &'a Path,
    pub(crate) graph: &'a BTreeMap<String, Vec<String>>,
    pub(crate) groups: &'a GroupNames,
    pub(crate) lock: &'a DeployLock,
    pub(crate) runner: &'static str,
}

/// (D4) `--context` does not deploy a dependency closure. A binding to a
/// Worker that has never been pushed fails at upload, so say which one
/// rather than letting Cloudflare's own error carry it. Then the contract
/// skews, either way round, of the one context selected.
pub(crate) fn context_gate(gate: &ContextGate<'_>, selected: &[String]) -> Result<(), Stop> {
    let ContextGate {
        opts,
        project_root,
        build_dir,
        workers_dir,
        graph,
        groups,
        lock,
        runner,
    } = *gate;
    let env = opts.environment.as_str();
    if opts.context.is_some()
        && let [worker] = selected
    {
        let absent = absent_dependencies(worker, graph, lock, &opts.environment);
        if !absent.is_empty() {
            return Err(Stop::printed(
                code::DEPENDENCY_NOT_DEPLOYED,
                &absent_dependencies_refusal(env, worker, &absent),
                &[
                    ("worker", deployed_worker_name(env, worker).into()),
                    (
                        "dependencies",
                        absent
                            .iter()
                            .map(|a| deployed_worker_name(env, a))
                            .collect::<Vec<_>>()
                            .into(),
                    ),
                ],
            ));
        }

        // v0.177 (#643): the other half of D4. The dependency exists — but does
        // it still provide the contract this worker was compiled against?
        // Without this, the push succeeds and production discovers the skew by
        // 409ing. `--context` is precisely the flag that makes this reachable.
        let mut manifest = match read_contracts_manifest(&workers_dir.join(worker)) {
            Ok(m) => m,
            Err(e) => return Err(manifest_unreadable(worker, &e)),
        };
        let mut skews = contract_skews(
            &manifest.expects,
            lock,
            |context: &str| groups.dir(context),
            &opts.environment,
        );
        classify_contract_skews(
            &mut skews,
            &manifest.expects_shapes,
            lock,
            |context: &str| groups.dir(context),
            &opts.environment,
        );
        if !skews.is_empty() {
            colour::eprint(&contract_skew_refusal(&opts.environment, worker, &skews));
            return Err(ExitCode::FAILURE.into());
        }

        // #2077: and the callee half. Does this build still provide what every
        // *other* live Worker was compiled against? Without this, pushing a
        // callee whose contract changed answers each live caller 409.
        let mut callee_skews = callee_contract_skews(
            worker,
            &manifest.provides,
            lock,
            |context: &str| groups.dir(context),
            &opts.environment,
        );
        classify_callee_skews(
            &mut callee_skews,
            &manifest.provides_shapes,
            lock,
            &opts.environment,
        );
        // #2554: a callee ahead of its callers may answer the older contract
        // they were compiled against, when the change is compatible for them.
        // The older contracts are recorded in `bynk.contracts.lock` and the
        // callee rebuilt to carry them.
        let mut recorded = false;
        if opts.accept_compatible_skew {
            let wanted =
                shapes_to_accept(&callee_skews, &manifest.accepts, lock, &opts.environment);
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
                        let path = bynk_driver::contracts_lock::lock_path(project_root);
                        return Err(Stop::error(
                            Entry::new(
                                code::CONTRACTS_LOCK_UNWRITABLE,
                                format!(
                                    "could not record the older contracts in {}: {e}",
                                    path.display()
                                ),
                            )
                            .with("path", path.display().to_string()),
                        ));
                    }
                }
                // The build reads the lock, so the callee carries what was just
                // recorded.
                if !rebuild(project_root, build_dir, opts) {
                    let path = bynk_driver::contracts_lock::lock_path(project_root);
                    return Err(Stop::error(
                        Entry::new(
                            code::REBUILD_FAILED,
                            format!(
                                "the older contracts were recorded in {}, but the rebuild failed; nothing was deployed. The entries are safe to keep (every build re-checks them), or revert the file.",
                                path.display()
                            ),
                        )
                        .with("path", path.display().to_string()),
                    ));
                }
                manifest = match read_contracts_manifest(&workers_dir.join(worker)) {
                    Ok(m) => m,
                    Err(e) => return Err(manifest_unreadable(worker, &e)),
                };
                callee_skews = callee_contract_skews(
                    worker,
                    &manifest.provides,
                    lock,
                    |context: &str| groups.dir(context),
                    &opts.environment,
                );
                classify_callee_skews(
                    &mut callee_skews,
                    &manifest.provides_shapes,
                    lock,
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
            report_callee_skews(
                callee_skews,
                graph,
                (&opts.environment, worker),
                (runner, lock),
            );
            return Err(ExitCode::FAILURE.into());
        }
        // #2554: older contracts the lock still accepts that no live caller
        // was compiled against any more are noise; say so.
        if let Ok(existing) = bynk_driver::contracts_lock::read(project_root)
            && let Ok(contracts) =
                bynk_project::contracts_lock::parse(existing.as_deref(), project_root)
        {
            let dir_of = |context: &str| groups.dir(context);
            let unused = unused_accepted(&contracts, (worker, &dir_of), lock, &opts.environment);
            if !unused.is_empty() {
                colour::eprint(&unused_accepted_notice(&unused));
            }
        }
        for caller in unrecorded_callers(worker, graph, lock, &opts.environment) {
            colour::eprint(&unrecorded_caller_warning(env, worker, &caller));
        }
    }
    Ok(())
}

/// The upload order of `selected` over `graph`, or the cycle that prevents one, reported.
pub(crate) fn ordered(
    selected: &[String],
    graph: &BTreeMap<String, Vec<String>>,
    resources: &BTreeMap<String, Resources>,
) -> Result<Vec<String>, Stop> {
    match deploy_order(selected, graph) {
        Ok(order) => Ok(order
            .into_iter()
            // A whole-project run orders every worker; `--context` orders only
            // the selection, but the DFS reaches its (already-live) targets —
            // drop them, D4 having already checked them.
            .filter(|worker| selected.contains(worker))
            .collect::<Vec<_>>()),
        Err(e) => {
            emsg!("bynk: error: {e}");
            let OrderError::Cycle(path) = &e;
            let entry =
                Entry::new(code::UPLOAD_ORDER_CYCLE, e.to_string()).with("cycle", path.clone());
            let publishers = cycle_publishers(path, resources);
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
            Err(Stop::refused(entry))
        }
    }
}

/// A Worker's contracts manifest that cannot be read: the run stops, naming the Worker.
fn manifest_unreadable(worker: &str, cause: &dyn std::fmt::Display) -> Stop {
    Stop::error(
        Entry::new(
            code::CONTRACTS_MANIFEST_UNREADABLE,
            format!(
                "could not read `{worker}`'s {}: {cause}",
                bynk_emit::emitter::contracts::CONTRACTS_MANIFEST
            ),
        )
        .with("worker", worker),
    )
}

/// I4 (#1970, track §11 Q6): a declared foreign caller's pin. Refused
/// before the plan, `--dry-run` included, as a Bynk caller's skew is: the
/// foreign caller's client would be refused on every call.
pub(crate) fn check_foreign_pins(
    project_root: &Path,
    workers_dir: &Path,
    order: &[String],
    opts: &DeployOptions,
) -> Result<(), ExitCode> {
    let pins = match read_pins(project_root) {
        Ok(pins) => pins,
        Err(e) => {
            emsg!("bynk: error: {e}");
            return Err(ExitCode::FAILURE);
        }
    };
    let mut built_foreign: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>> =
        BTreeMap::new();
    for worker in order {
        match read_contracts_manifest(&workers_dir.join(worker)) {
            Ok(m) if !m.foreign.is_empty() => {
                built_foreign.insert(context_of(worker), m.foreign);
            }
            Ok(_) => {}
            Err(e) => {
                emsg!(
                    "bynk: error: could not read `{worker}`'s {}: {e}",
                    bynk_emit::emitter::contracts::CONTRACTS_MANIFEST
                );
                return Err(ExitCode::FAILURE);
            }
        }
    }
    let in_run: BTreeSet<String> = order.iter().map(|w| context_of(w)).collect();
    let (broken, unpinned) = check_pins(&pins, &built_foreign, &in_run, opts.context.is_none());
    if !broken.is_empty() {
        colour::eprint(&refusal_message(&broken));
        return Err(ExitCode::FAILURE);
    }
    if !unpinned.is_empty() {
        colour::eprint(&unpinned_message(&unpinned));
    }
    Ok(())
}

/// What [`decide_keys`] reads.
pub(crate) struct KeysCx<'a> {
    pub(crate) order: &'a [String],
    pub(crate) resources: &'a BTreeMap<String, Resources>,
    pub(crate) api: &'a dyn CloudflareApi,
    pub(crate) workers_dir: &'a Path,
    pub(crate) lock: &'a DeployLock,
    pub(crate) opts: &'a DeployOptions,
}

/// What the run decided of the secrets it manages: each Worker's `secret list`, each edge's key,
/// and each foreign caller's secret and session key.
pub(crate) struct Keys {
    pub(crate) listed: BTreeMap<String, Option<BTreeSet<String>>>,
    pub(crate) edge_keys: BTreeMap<Edge, EdgeKey>,
    pub(crate) foreign_secrets: BTreeSet<String>,
    pub(crate) foreign_keys: BTreeMap<(String, String), ForeignKey>,
}

/// I2 (#1971): each edge's key is decided over both its ends at once, so
/// each Worker's `secret list` is asked here, before anything is pushed,
/// and the per-context step reuses the answer. That covers every Worker
/// this run deploys and the far end of each edge it touches, which a
/// `--context` run does not deploy. Asked before the KV ids are
/// materialised and any `[env.<name>]` table is synthesised: wrangler loads
/// that config (a placeholder id included) and addresses the same Worker
/// either way, since the `name` the synthesised table sets (#2028) is the
/// one Wrangler's `--env` default gives a table without one.
pub(crate) fn decide_keys(cx: &KeysCx<'_>, secret_source: &SecretSource) -> Result<Keys, ExitCode> {
    let KeysCx {
        order,
        resources,
        api,
        workers_dir,
        lock,
        opts,
    } = *cx;
    let in_run: BTreeSet<&str> = order.iter().map(String::as_str).collect();
    let edges: Vec<Edge> = project_edges(resources)
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
            let answer = list_secrets(api, &workers_dir.join(worker), &opts.environment);
            (worker.to_string(), answer)
        })
        .collect();
    let mut edge_keys: BTreeMap<Edge, EdgeKey> = BTreeMap::new();
    for edge in &edges {
        let names = match edge.keys() {
            Ok(names) => names,
            Err(e) => {
                emsg!("bynk: error: {e}");
                return Err(ExitCode::FAILURE);
            }
        };
        let held = |worker: &str, keys: &[String]| {
            end_presence(edge, worker, keys, &listed, lock, &opts.environment)
        };
        let callee = held(&edge.callee, &names.on_callee);
        let caller = held(&edge.caller, &names.on_caller);
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
                emsg!("bynk: error: {e}");
                return Err(ExitCode::FAILURE);
            }
        }
    }
    // I3 (#1971): each foreign caller's secret, and (#3386) each `SignedCookie` actor's signing
    // key, on the context in the run that holds it. Only that end is ours, so a `--context` run
    // decides them too.
    let managed = ManagedKeys {
        order,
        resources,
        listed: &listed,
    };
    let foreign_secrets = managed.names();
    let foreign_keys = match managed.decide(lock, opts, secret_source) {
        Ok(keys) => keys,
        Err(e) => {
            colour::eprint(&e);
            return Err(ExitCode::FAILURE);
        }
    };
    Ok(Keys {
        listed,
        edge_keys,
        foreign_secrets,
        foreign_keys,
    })
}

/// What [`account_checks`] reads: the plan the run confirms, and what it was derived from.
pub(crate) struct AccountCx<'a> {
    pub(crate) project_root: &'a Path,
    pub(crate) opts: &'a DeployOptions,
    pub(crate) plan: &'a Plan<'a>,
    pub(crate) order: &'a [String],
    pub(crate) resources: &'a BTreeMap<String, Resources>,
    pub(crate) lock: &'a DeployLock,
    pub(crate) hyperdrive_wants: &'a hyperdrive::Wants,
}

/// #4056 Decision E: a run whose steps will call the account's REST API for a resource kind
/// (KV since 1D.4a) needs `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`, checked here, after
/// the plan and before the confirmation, so a missing one is refused before anything is created.
/// The first kind in list order that needs them reports, in its own words. A run that needs
/// nothing asks nothing: a redeploy with everything recorded still works with `wrangler login`.
fn account_resource_credentials(cx: &AccountCx<'_>) -> Result<(), ExitCode> {
    let project = ProjectCx {
        ledger: LedgerView::of(cx.lock, &cx.opts.environment),
        resources: cx.resources,
        order: cx.order,
    };
    let checked = resource_credentials(&project, &cx.plan.orphans.kv, cx.opts.prune, |purpose| {
        crate::cloudflare::rest::for_kind(purpose).map(drop)
    });
    checked.map_err(|refusal| {
        emsg!("bynk: error: {refusal}");
        ExitCode::FAILURE
    })
}

/// [`account_resource_credentials`]'s decision over `credentials`, the run's client check for a
/// kind (a test's stand-in): the refusal, in the first needing kind's words, or `Ok` when no
/// step needs the account or the credentials are there.
pub(crate) fn resource_credentials(
    project: &ProjectCx<'_>,
    kv_orphans: &[OrphanKv],
    prune: bool,
    credentials: impl Fn(
        crate::cloudflare::rest::Purpose,
    ) -> Result<(), crate::cloudflare::CloudflareError>,
) -> Result<(), String> {
    if !kv_calls_the_account(project, kv_orphans, prune) {
        return Ok(());
    }
    credentials(crate::cloudflare::rest::Purpose::Kv).map_err(|e| e.to_string())
}

/// What the account checks found, for the apply: the wrangler to run, the ids each stored secret
/// and certificate is linked to, and the Hyperdrive API.
pub(crate) struct AccountChecked {
    pub(crate) probe: probe::Probe,
    pub(crate) linked_stores: BTreeMap<(String, String), String>,
    pub(crate) hyperdrive_api: Option<hyperdrive::RestHyperdrive>,
}

/// The checks after the plan and before the confirmation: whether a CI job may deploy it, the
/// wrangler that will, authentication, the foreign Workers it binds, the linked stores, AI Search
/// instances and certificates, and the Hyperdrive API.
pub(crate) fn account_checks(
    tb: &dyn Toolbox,
    cx: &AccountCx<'_>,
) -> Result<AccountChecked, ExitCode> {
    let AccountCx {
        project_root,
        opts,
        plan,
        order,
        resources,
        lock,
        hyperdrive_wants,
    } = *cx;
    if unrecordable_in_ci(order, resources, lock, &opts.environment, hyperdrive_wants) {
        return Err(ExitCode::FAILURE);
    }
    let (manager, probe) = probe::detect_in_project(tb, "wrangler", project_root);
    // #1796: before authenticating, so the refusal costs no network call.
    if let Some(refusal) = preflight_refusal(tb, &probe, manager, order, resources) {
        colour::eprintln(&refusal);
        return Err(ExitCode::FAILURE);
    }
    let api = Account::new(Wrangler::new(&probe.provenance, project_root));
    if !whoami(&api) {
        emsg!(
            "bynk: error: Cloudflare authentication is unavailable; run `wrangler login` or set CLOUDFLARE_API_TOKEN"
        );
        return Err(ExitCode::FAILURE);
    }
    // Interop I5 (#1972, track §11 Q8): a foreign Worker a Service Binding
    // names is not Bynk's to create, so a missing one is a warning, never a
    // refusal from the driver. Asked once per Worker, before the
    // confirmation, so the user decides with it in view.
    for warning in foreign_worker_warnings(order, resources, |name| worker_presence(&api, name)) {
        colour::eprintln(&warning);
    }
    // #3301: a stored secret is linked, never created, so a missing store or
    // secret stops the run here, before the confirmation and before anything
    // is provisioned or pushed.
    let Some(linked_stores) = link_for_run(order, resources) else {
        return Err(ExitCode::FAILURE);
    };
    // #3480: an AI Search instance too: found by name, never created.
    if !link_ai_search_for_run(order, resources) {
        return Err(ExitCode::FAILURE);
    }
    // #3319: the Hyperdrive API client, checked here, before the confirmation,
    // so a missing token costs nothing.
    let Ok(hyperdrive_api) = hyperdrive::api_for_run(
        order,
        resources,
        lock,
        &opts.environment,
        hyperdrive_wants,
        &plan.orphans,
        opts.prune,
    ) else {
        return Err(ExitCode::FAILURE);
    };
    account_resource_credentials(cx)?;
    Ok(AccountChecked {
        probe,
        linked_stores,
        hyperdrive_api,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #3797: a cycle in the Service Bindings is a refusal entry naming the cycle, with the
    /// stderr line unchanged.
    #[test]
    fn a_cycle_in_the_upload_order_is_a_refusal_entry_naming_it() {
        let graph = BTreeMap::from([
            ("a".to_string(), vec!["b".to_string()]),
            ("b".to_string(), vec!["a".to_string()]),
        ]);
        let stop = ordered(
            &["a".to_string(), "b".to_string()],
            &graph,
            &BTreeMap::new(),
        )
        .expect_err("a cycle has no upload order");
        assert_eq!(stop.refusals.len(), 1);
        let entry = &stop.refusals[0];
        assert_eq!(entry.code, code::UPLOAD_ORDER_CYCLE);
        let cycle = entry.subject["cycle"]
            .as_array()
            .expect("the cycle is listed");
        assert!(
            cycle.iter().any(|w| w == "a") && cycle.iter().any(|w| w == "b"),
            "{entry:?}"
        );
        assert!(entry.override_flag.is_none());
    }

    #[test]
    fn an_unreadable_manifest_names_its_worker() {
        let stop = manifest_unreadable("shop-orders", &"no such file");
        assert_eq!(stop.refusals.len(), 1);
        let entry = &stop.refusals[0];
        assert_eq!(entry.code, code::CONTRACTS_MANIFEST_UNREADABLE);
        assert_eq!(entry.subject["worker"], "shop-orders");
        assert!(
            entry.message.starts_with("could not read `shop-orders`'s "),
            "{}",
            entry.message
        );
    }
}
