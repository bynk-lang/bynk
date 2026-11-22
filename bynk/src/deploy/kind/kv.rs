//! 1D.1a (#3929): KV, the cloudflare adapter's `Kv` and each derived instance (`capability
//! Sessions = Kv`), as one [`ResourceKind`].
//!
//! A namespace's id is minted by Cloudflare and recorded in the ledger before the push, under
//! two maps that keep their shape on disk: the bare `KV` in `Environment::kv`, an instance in
//! `Environment::kv_instances` (S4b, #1967). Its title is account-wide, so a create is checked
//! against every environment the ledger knows, and a recorded id the account no longer has is
//! created again (slice 5, DECISION B).

use super::*;

/// The KV kind.
pub(crate) struct Kv;

/// [`Kv`]'s records: the bare namespace in `Environment::kv`, by Worker, and each instance in
/// `Environment::kv_instances`, by Worker and binding.
pub(crate) struct KvTable;

impl KvNamespace {
    /// The record of namespace `id`.
    pub(crate) fn of(id: &str) -> Self {
        KvNamespace { id: id.to_string() }
    }
}

impl LedgerTable for KvTable {
    type Record = KvNamespace;

    fn get<'l>(env: &'l Environment, worker: &str, binding: &str) -> Option<&'l KvNamespace> {
        if is_bare_kv(binding) {
            env.kv.get(worker)
        } else {
            env.kv_instances
                .get(worker)
                .and_then(|instances| instances.get(binding))
        }
    }

    fn insert(env: &mut Environment, worker: &str, binding: &str, record: KvNamespace) {
        if is_bare_kv(binding) {
            env.kv.insert(worker.to_string(), record);
        } else {
            env.kv_instances
                .entry(worker.to_string())
                .or_default()
                .insert(binding.to_string(), record);
        }
    }

    /// Drops a Worker's instance map once it is empty, so a pruned ledger is the file it would
    /// have been had the instance never existed.
    fn remove(env: &mut Environment, worker: &str, binding: &str) {
        if is_bare_kv(binding) {
            env.kv.remove(worker);
        } else if let Some(instances) = env.kv_instances.get_mut(worker) {
            instances.remove(binding);
            if instances.is_empty() {
                env.kv_instances.remove(worker);
            }
        }
    }

    /// Every bare namespace, by Worker, then every instance, by Worker and binding.
    fn entries(env: &Environment) -> Vec<(&str, &str, &KvNamespace)> {
        env.kv
            .iter()
            .map(|(worker, kv)| {
                (
                    worker.as_str(),
                    bynk_emit::emitter::wrangler::KV_BINDING_NAME,
                    kv,
                )
            })
            .chain(env.kv_instances.iter().flat_map(|(worker, instances)| {
                instances
                    .iter()
                    .map(move |(binding, kv)| (worker.as_str(), binding.as_str(), kv))
            }))
            .collect()
    }
}

impl ResourceKind for Kv {
    const ID: KindId = KindId::Kv;

    /// The binding: `KV` for the bare namespace, `KV_<SNAKE>` for an instance.
    type Decl<'r> = &'r str;
    type Record = KvNamespace;
    type Table = KvTable;
    type Line<'r> = PlanKv<'r>;
    type Orphan = OrphanKv;
    /// The account's namespace ids, when they were read ([`live_kv_namespace_ids`]).
    type Live = Option<BTreeSet<String>>;
    /// The namespace id.
    type Bound = String;

    fn declared(r: &Resources) -> Vec<&str> {
        kv_bindings(r).collect()
    }

    fn binding<'r>(decl: Self::Decl<'r>) -> &'r str {
        decl
    }

    fn lines<'p, 'r>(context: &'p ContextPlan<'r>) -> &'p [PlanKv<'r>] {
        &context.kv
    }

    fn lines_mut<'p, 'r>(context: &'p mut ContextPlan<'r>) -> &'p mut Vec<PlanKv<'r>> {
        &mut context.kv
    }

    fn plan_line<'r>(cx: &PlanCx<'_>, binding: Self::Decl<'r>) -> PlanKv<'r> {
        PlanKv {
            binding,
            action: if cx.ledger.recorded::<Kv>(cx.worker, binding).is_some() {
                "reuse"
            } else {
                "create"
            },
            namespace: kv_title(cx.worker, binding, cx.ledger.environment),
        }
    }

    fn short_line(kv: &PlanKv<'_>, out: &mut String) {
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

    fn listing(kv: &PlanKv<'_>) -> Option<on_account::Listing> {
        matches!(kv.action, "create" | "create or reuse").then_some((Self::ID, None))
    }

    /// A namespace already on the account is noted, not reused: Cloudflare refuses a second
    /// namespace of one title, so the deploy cannot create it (#3494).
    fn checked_against(
        kv: &mut PlanKv<'_>,
        shown_worker: &str,
        on: &on_account::OnAccount,
        notes: &mut Vec<String>,
    ) {
        if kv.action == "create" && on.holds(Self::ID, None, &kv.namespace) == Some(true) {
            notes.push(format!(
                "bynk: warning: the KV namespace `{}` (`{}` of `{}`) is already on the \
                 account, but `bynk.deploy.lock` does not record it, and Cloudflare \
                 refuses a second namespace of one title: the deploy cannot create it. \
                 Delete the namespace if it holds nothing you need, or rename the \
                 capability",
                kv.namespace, kv.binding, shown_worker
            ));
        }
    }

    fn name_problem(cx: &ProjectCx<'_>) -> Option<String> {
        kv_title_collision(cx.resources, cx.ledger)
    }

    /// The first namespace in `cx.order` with no recorded id: a namespace's id is minted (S4b,
    /// #1967: each derived instance's too), so one created in CI and never committed is an
    /// orphan nobody can find again. Every namespace counts, the bare `KV` and each derived
    /// instance alike.
    fn unrecordable_in_ci(cx: &ProjectCx<'_>) -> Option<String> {
        let ledger = cx.ledger;
        let (worker, binding) = cx.order.iter().find_map(|worker| {
            kv_bindings(&cx.resources[worker])
                .find(|binding| ledger.recorded::<Kv>(worker, binding).is_none())
                .map(|binding| (worker.as_str(), binding))
        })?;
        Some(format!(
            "bynk: error: {} is unrecorded; provision locally first and commit {LOCK_FILE}",
            kv_namespace_noun(worker, binding)
        ))
    }

    fn orphans_in(orphans: &Orphans) -> &[OrphanKv] {
        &orphans.kv
    }

    fn orphans_in_mut(orphans: &mut Orphans) -> &mut Vec<OrphanKv> {
        &mut orphans.kv
    }

    /// The bare namespace with its Worker, or on its own once the live Worker stops using the
    /// bare `Kv` (review of #1974); then each instance, with its Worker or on its own when its
    /// Worker no longer declares it (S4b, #1967).
    fn orphans(cx: &OrphanCx<'_>) -> Vec<OrphanKv> {
        let resources = cx.resources;
        let instance_orphans = cx.env.kv_instances.iter().flat_map(|(worker, instances)| {
            instances
                .keys()
                .filter(move |binding| {
                    resources
                        .get(worker)
                        .is_none_or(|r| !r.kv_instances.contains(binding))
                })
                .map(move |binding| OrphanKv {
                    worker: worker.clone(),
                    binding: Some(binding.clone()),
                })
        });
        cx.env
            .kv
            .keys()
            .filter(|worker| resources.get(worker.as_str()).is_none_or(|r| !r.needs_kv))
            .map(|worker| OrphanKv {
                worker: worker.clone(),
                binding: None,
            })
            .chain(instance_orphans)
            .collect()
    }

    /// A KV orphan keeps its Worker's ledger key: its line names the ledger entry, not the
    /// namespace's title ([`kv_title`]), which a namespace made before #2037 may not match.
    fn shown_orphan(kv: &OrphanKv, _environment: &str) -> OrphanKv {
        kv.clone()
    }

    fn short_orphan(kv: &OrphanKv, out: &mut String) {
        match &kv.binding {
            None => out.push_str(&format!("orphan kv {}\n", kv.worker)),
            Some(binding) => {
                out.push_str(&format!("orphan kv {} {binding}\n", kv.worker));
            }
        }
    }

    fn orphan_worker(kv: &OrphanKv) -> &str {
        &kv.worker
    }

    fn prunable(_kv: &OrphanKv) -> bool {
        true
    }

    fn prune_notice(kv: &OrphanKv, environment: &str) -> Option<Notice> {
        Some(match &kv.binding {
            None => Notice::Styled(format!(
                "bynk: will delete KV namespace for `{}`",
                kv.worker
            )),
            Some(binding) => Notice::Plain(format!(
                "bynk: will delete KV namespace `{}` for `{}`'s `{binding}`",
                kv_title(&kv.worker, binding, environment),
                kv.worker
            )),
        })
    }

    /// Deletes the namespace by its recorded id, and forgets it whether the delete removed it
    /// or found it gone (DECISION E). An orphan with no recorded id is left alone.
    fn prune(cx: &mut ApplyCx<'_, Kv>, orphan: &OrphanKv) -> Result<Option<String>, DeployFailure> {
        let (worker, binding) = (orphan.worker.as_str(), orphan.binding());
        let Some(id) = cx
            .ledger()
            .recorded::<Kv>(worker, binding)
            .map(|kv| kv.id.clone())
        else {
            return Ok(None);
        };
        let what = match &orphan.binding {
            None => format!("the orphaned KV namespace for `{worker}`"),
            Some(binding) => format!("the orphaned KV namespace `{binding}` for `{worker}`"),
        };
        delete_kv_namespace(cx.tools.provenance, &id, cx.tools.project_root)
            .map_err(|e| DeployFailure::driver(format!("could not delete {what}: {e}")))?;
        cx.forget(binding);
        Ok(Some(what))
    }

    /// Slice 5, DECISION B: once per run, not once per context (an account-wide list), and only
    /// when it could change an outcome: a Worker of `cx.order` binds a namespace whose id is
    /// recorded (a first deploy has nothing to check drift against). `None` (no wrangler, or
    /// the call failed) trusts the ledger unconditionally, as every deploy before that slice did.
    fn live(tools: Tools<'_>, cx: &ProjectCx<'_>) -> Option<BTreeSet<String>> {
        if cx.order.iter().any(|worker| {
            kv_bindings(&cx.resources[worker])
                .any(|binding| cx.ledger.recorded::<Kv>(worker, binding).is_some())
        }) {
            live_kv_namespace_ids(tools.provenance, tools.project_root)
        } else {
            None
        }
    }

    /// Reuses the recorded id when [`should_trust_recorded_kv`]; otherwise creates the namespace
    /// under its current title and records it.
    fn provision(
        cx: &mut ApplyCx<'_, Kv>,
        binding: &str,
    ) -> Result<Provisioned<String>, DeployFailure> {
        let (worker, environment) = (cx.worker, cx.environment);
        let recorded = cx
            .ledger()
            .recorded::<Kv>(worker, binding)
            .map(|kv| kv.id.clone());
        if should_trust_recorded_kv(recorded.as_deref(), cx.live.as_ref()) {
            let id = recorded.expect("trust_recorded is only true when recorded is Some");
            return Ok(Provisioned {
                bound: Some((binding.to_string(), id)),
                recorded: None,
            });
        }
        // #2059: the plan-time check covered only namespaces with no recorded id. One whose
        // recorded id has gone from the account is created again here, under its current title,
        // so compare that title across environments now, before `kv namespace create`.
        if let Some(message) = cross_environment_title_clash(
            cx.project,
            cx.ledger(),
            &[(worker, binding)],
            cx.live.as_ref(),
        ) {
            return Err(DeployFailure::driver(message));
        }
        let what = kv_namespace_noun(worker, binding);
        let id = create_kv(
            cx.tools.provenance,
            &kv_title(worker, binding, environment),
            cx.tools.project_root,
        )
        .map_err(|e| DeployFailure::driver(format!("could not create {what}: {e}")))?;
        cx.record(binding, KvNamespace::of(&id));
        Ok(Provisioned {
            bound: Some((binding.to_string(), id)),
            recorded: Some(what),
        })
    }

    fn materialise(
        config: &Path,
        worker: &str,
        bound: &BTreeMap<String, String>,
    ) -> Result<(), DeployFailure> {
        if !bound.is_empty() && !materialise_kv_ids(config, bound) {
            return Err(DeployFailure::driver(format!(
                "could not materialise the KV namespace ids into `{worker}`'s generated configuration"
            )));
        }
        Ok(())
    }

    fn bound_id(kv: &KvNamespace) -> Option<&str> {
        Some(&kv.id)
    }
}

/// The account-facing title of one of `worker`'s KV namespaces under
/// `environment` (S4b, #1967), always environment-qualified ([`env_qualify`]).
/// The bare `KV`'s is the Worker's name: `shop-cart` in the default
/// environment, as it has always been, and `shop-cart-staging` under
/// `--env staging` (#2037). A derived instance's is `<worker>-<capability,
/// kebab>`: `shop-cart-sessions`, and `shop-cart-sessions-staging`.
///
/// Qualified because Cloudflare refuses a second namespace with a title the
/// account already holds: "Returns `400` if the account already owns a
/// namespace with that title" (API reference, Create a Namespace), which
/// Wrangler reports as code 10014, "A KV namespace with the title … already
/// exists". Bynk never passes `--env` to `kv namespace create` (ADR 0254 D4),
/// so Wrangler's own `<env>-` title prefix never applies; this is the only
/// qualification. The title matters only on create: a namespace whose id the
/// ledger already records for the environment is reused by id, whatever it
/// was titled when it was made.
pub(crate) fn kv_title(worker: &str, binding: &str, environment: &str) -> String {
    if binding == bynk_emit::emitter::wrangler::KV_BINDING_NAME {
        env_qualify(environment, worker)
    } else {
        env_qualify(
            environment,
            &bynk_emit::emitter::wrangler::kv_instance_title(worker, binding),
        )
    }
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct KvNamespace {
    pub(crate) id: String,
}

/// One recorded KV namespace the build no longer declares (S4b, #1967): the
/// bare one of a removed Worker (`binding` absent), or a derived instance,
/// whose Worker may still be live — a capability removed or renamed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct OrphanKv {
    pub(crate) worker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) binding: Option<String>,
}

impl OrphanKv {
    /// The binding whose ledger entry this is: `KV` for the bare namespace.
    pub(crate) fn binding(&self) -> &str {
        self.binding
            .as_deref()
            .unwrap_or(bynk_emit::emitter::wrangler::KV_BINDING_NAME)
    }
}

/// Whether `binding` is the bare `Kv`'s, as opposed to a derived instance's.
pub(crate) fn is_bare_kv(binding: &str) -> bool {
    binding == bynk_emit::emitter::wrangler::KV_BINDING_NAME
}

/// Every KV binding `declared` names, the bare `KV` first, then its instances
/// in binding order (S4b, #1967): the order a deploy provisions them and the
/// plan lists them.
pub(crate) fn kv_bindings(declared: &Resources) -> impl Iterator<Item = &str> {
    declared
        .needs_kv
        .then_some(bynk_emit::emitter::wrangler::KV_BINDING_NAME)
        .into_iter()
        .chain(declared.kv_instances.iter().map(String::as_str))
}

/// Slice 5, DECISION B, extracted for direct testing (review, #840): should
/// `deploy_one` trust a recorded KV id, or treat it as though nothing were
/// recorded at all?
///
/// - No record at all → never trust (nothing to trust).
/// - A record, and the live fetch never ran or failed (`None`) → trust it,
///   unconditionally — exactly pre-slice-5 behaviour, so a fetch outage
///   never blocks a deploy that would have succeeded before this slice.
/// - A record, and a live id set — trust it only if the id is actually in
///   that set. Absent means Cloudflare no longer recognises it: re-provision,
///   the same as an unrecorded id would.
pub(crate) fn should_trust_recorded_kv(
    recorded: Option<&str>,
    live_kv_ids: Option<&BTreeSet<String>>,
) -> bool {
    match (recorded, live_kv_ids) {
        (Some(id), Some(live)) => live.contains(id),
        (Some(_), None) => true,
        (None, _) => false,
    }
}

#[derive(Debug, Serialize)]
pub(crate) struct PlanKv<'a> {
    /// `KV` for the bare `Kv`, `KV_<SNAKE>` for a derived instance.
    pub(crate) binding: &'a str,
    pub(crate) action: &'static str,
    /// The namespace's account-facing title ([`kv_title`]).
    pub(crate) namespace: String,
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
    ledger: LedgerView<'_>,
) -> Option<String> {
    let environment = ledger.environment;
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
        .filter(|(worker, binding)| ledger.recorded::<Kv>(worker, binding).is_none())
        .collect();
    cross_environment_title_clash(resources, ledger, &creates, None)
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
    ledger: LedgerView<'_>,
    creates: &[(&str, &str)],
    live_kv_ids: Option<&BTreeSet<String>>,
) -> Option<String> {
    if creates.is_empty() {
        return None;
    }
    let environment = ledger.environment;
    let is_live = |id: &str| live_kv_ids.is_none_or(|live| live.contains(id));
    let declared = || {
        resources.iter().flat_map(|(worker, declared)| {
            kv_bindings(declared).map(move |binding| (worker.as_str(), binding))
        })
    };
    for (other, recorded) in ledger.others() {
        // Each title `other` holds or would create, and whose it is: what its
        // ledger records first, then what this source declares.
        let held = KvTable::entries(recorded)
            .into_iter()
            .filter(|(_, _, kv)| is_live(&kv.id))
            .map(|(worker, binding, _)| (worker, binding));
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

/// How a message names one of `worker`'s KV namespaces: `KV namespace for
/// `api`` for the bare one, as it always has, `KV namespace `KV_SESSIONS` for
/// `shop-cart`` for a derived instance (S4b, #1967).
pub(crate) fn kv_namespace_noun(worker: &str, binding: &str) -> String {
    if is_bare_kv(binding) {
        format!("KV namespace for `{worker}`")
    } else {
        format!("KV namespace `{binding}` for `{worker}`")
    }
}

fn create_kv(provenance: &Provenance, name: &str, project_root: &Path) -> Result<String, String> {
    let Some(mut command) = workers::wrangler_command(provenance, "kv") else {
        return Err("wrangler not found".into());
    };
    let output = command
        .arg("namespace")
        .arg("create")
        .arg(name)
        // No `--json`: `kv namespace create` has no such flag (`Unknown
        // argument: json`, #2428). The id is read from the binding snippet it
        // prints instead.
        // The generated worker config still carries the placeholder until the
        // namespace exists. Create from the project root so Wrangler cannot
        // load and validate that incomplete config.
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err(wrangler_said(&output));
    }
    parse_kv_id(&String::from_utf8_lossy(&output.stdout))
        .ok_or_else(|| "wrangler did not return a namespace id".into())
}

/// The namespace id in `wrangler kv namespace create`'s printed binding
/// snippet: `"id": "…"` in a JSON config, `id = "…"` in a TOML one.
fn parse_kv_id(output: &str) -> Option<String> {
    // A whole-output JSON document first (compact or pretty), then the line scan.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
        let id = value
            .get("id")
            .or_else(|| value.get("kv_namespaces")?.get(0)?.get("id"));
        if let Some(id) = id.and_then(|id| id.as_str()) {
            return Some(id.to_owned());
        }
    }
    output.lines().find_map(|line| {
        let line = line.trim().trim_start_matches(['{', '[']).trim_start();
        let rest = line
            .strip_prefix("\"id\"")
            .or_else(|| line.strip_prefix("id"))?
            .trim_start();
        let rest = rest
            .strip_prefix([':', '='])?
            .trim_start()
            .strip_prefix('"')?;
        let id = &rest[..rest.find('"')?];
        (!id.is_empty()).then(|| id.to_owned())
    })
}

/// Every KV namespace id currently on the account — slice 5, DECISION B.
///
/// Fetched **once per `bynk deploy` run**, not once per context: this is an
/// account-wide list, so calling it from inside `deploy_one` would pay one
/// identical round-trip per KV-bearing context on every redeploy. `run()`
/// calls this once, before the per-context loop, and threads the result
/// through — see `deploy_one`'s `live_kv` parameter ([`Kv`]'s `live`).
///
/// `None` means "could not ask" (no wrangler, network/auth failure, or an
/// unparseable response) — read the same way [`list_secrets`]'s `None` is:
/// the caller falls back to trusting whatever the ledger already says, rather
/// than blocking every deploy on this call succeeding.
///
/// **Completeness (review, #840):** Cloudflare's list-namespaces endpoint is
/// itself paginated, but `wrangler`'s `listKVNamespaces` (`workers-sdk`
/// `packages/wrangler/src/kv/helpers.ts`) loops `page`/`per_page` until a
/// short page comes back, aggregating every page before this command prints
/// — confirmed against wrangler's own source, not assumed. A truncated
/// response is not a live risk on the CLI path this calls.
pub(crate) fn live_kv_namespace_ids(
    provenance: &Provenance,
    project_root: &Path,
) -> Option<BTreeSet<String>> {
    let mut command = workers::wrangler_command(provenance, "kv")?;
    // No `--format json`: unlike `secret list`/`queues`, this command has no
    // such flag at all — confirmed against wrangler 4.103 (`Unknown argument:
    // format`, exit 1). Its *default* output is already a raw JSON array, so
    // this is not a missing-flag gap, just a different default per command.
    let output = command
        .arg("namespace")
        .arg("list")
        .current_dir(project_root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_kv_namespace_ids(&String::from_utf8_lossy(&output.stdout))
}

/// The ids in `wrangler kv namespace list`'s (default, already-JSON) output —
/// the same structural shape [`parse_secret_list`] reads, `"id"` instead of
/// `"name"`.
fn parse_kv_namespace_ids(stdout: &str) -> Option<BTreeSet<String>> {
    let parsed: serde_json::Value = serde_json::from_str(stdout).ok()?;
    Some(
        parsed
            .as_array()?
            .iter()
            .filter_map(|entry| entry.get("id")?.as_str().map(str::to_owned))
            .collect(),
    )
}

/// Delete a KV namespace by id — `--prune`'s one KV-side mutation.
///
/// `--skip-confirmation`: wrangler's own prompt is redundant with
/// [`confirm_prune`], which already named every resource about to go and
/// asked once for the whole batch: a second, per-resource prompt would be
/// noise, not safety. Idempotent against a namespace already gone (matched
/// via [`kv_namespace_already_deleted`]) — confirmed empirically (#839):
/// Cloudflare answers a not-found delete with an error, never silent success,
/// so this driver-side match is what makes re-running `--prune` safe.
pub(crate) fn delete_kv_namespace(
    provenance: &Provenance,
    id: &str,
    project_root: &Path,
) -> Result<(), String> {
    let Some(mut command) = workers::wrangler_command(provenance, "kv") else {
        return Err("wrangler not found".into());
    };
    let output = command
        .arg("namespace")
        .arg("delete")
        .arg("--namespace-id")
        .arg(id)
        .arg("--skip-confirmation")
        .current_dir(project_root)
        .output()
        .map_err(|e| e.to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let said = wrangler_said(&output);
    if kv_namespace_already_deleted(&said) {
        return Ok(());
    }
    Err(said)
}

/// Does this `wrangler kv namespace delete` failure just mean the namespace
/// is already gone? Confirmed empirically (#839) against a real account:
/// `namespace not found [code: 10013]`. Matching the stable error code rather
/// than the prose, unlike [`queue_already_exists`]'s text match — Cloudflare
/// structures its API errors with a `code`, and `10013` is the more durable
/// half of the message if the wording is ever revised. Pure, so the rule is
/// tested without an account.
fn kv_namespace_already_deleted(stderr: &str) -> bool {
    stderr.contains("code: 10013") || stderr.to_ascii_lowercase().contains("namespace not found")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_id_from_the_printed_binding_snippet() {
        let toml = "🌀 Creating namespace with title \"api\"\n✨ Success!\n\
                    [[kv_namespaces]]\nbinding = \"api\"\nid = \"abc\"\n";
        assert_eq!(parse_kv_id(toml), Some("abc".into()));
        let json = "{\n  \"kv_namespaces\": [\n    {\n      \"binding\": \"api\",\n      \"id\": \"abc\"\n    }\n  ]\n}\n";
        assert_eq!(parse_kv_id(json), Some("abc".into()));
        let compact = r#"{"kv_namespaces":[{"binding":"api","id":"abc"}]}"#;
        assert_eq!(parse_kv_id(compact), Some("abc".into()));
        assert_eq!(parse_kv_id("✘ [ERROR] nope"), None);
    }

    #[test]
    fn should_trust_recorded_kv_only_when_the_id_is_live_or_unchecked() {
        let live = BTreeSet::from(["kv-abc".to_string()]);
        assert!(
            should_trust_recorded_kv(Some("kv-abc"), Some(&live)),
            "recorded and present in the live set"
        );
        assert!(
            !should_trust_recorded_kv(Some("kv-deleted"), Some(&live)),
            "recorded but Cloudflare no longer has it — treat as unrecorded"
        );
        assert!(
            should_trust_recorded_kv(Some("kv-abc"), None),
            "the fetch never ran or failed — trust the ledger, as pre-slice-5"
        );
        assert!(
            !should_trust_recorded_kv(None, Some(&live)),
            "nothing recorded at all"
        );
        assert!(!should_trust_recorded_kv(None, None));
    }

    #[test]
    fn kv_namespace_already_deleted_matches_the_confirmed_error_shape() {
        // Confirmed empirically against a real account (#839): `namespace not
        // found [code: 10013]`. The code is matched, not just the prose — the
        // more durable half if Cloudflare ever reword the message.
        assert!(kv_namespace_already_deleted(
            "✘ [ERROR] A request to the Cloudflare API (...) failed.\n  namespace not found [code: 10013]"
        ));
        assert!(kv_namespace_already_deleted("code: 10013"));
        assert!(!kv_namespace_already_deleted(
            "✘ [ERROR] Authentication error [10000]"
        ));
        assert!(!kv_namespace_already_deleted(""));
    }

    #[test]
    fn kv_titles_keep_the_default_names_and_qualify_every_other_environment() {
        // The default environment's titles are the ones every existing
        // production deployment was created with.
        assert_eq!(kv_title("api", "KV", "default"), "api");
        // #2037: the bare namespace is qualified under `--env` too, since an
        // account holds one namespace per title.
        assert_eq!(kv_title("api", "KV", "staging"), "api-staging");
        assert_eq!(
            kv_title("shop-cart", "KV_SESSIONS", "default"),
            "shop-cart-sessions"
        );
        assert_eq!(
            kv_title("shop-cart", "KV_PRICE_CACHE", "staging"),
            "shop-cart-price-cache-staging"
        );
    }

    // ---- 1D.1a (#3929): the `Kv` impl ----

    use crate::deploy::config::tests::project;

    fn lock_of(records: &[(&str, &str, &str, &str)]) -> DeployLock {
        let mut lock = DeployLock::default();
        for (environment, worker, binding, id) in records {
            lock.record::<Kv>(environment, worker, binding, KvNamespace::of(id));
        }
        lock
    }

    #[test]
    fn the_table_keeps_the_bare_namespace_and_the_instances_in_their_own_maps() {
        let mut env = Environment::default();
        KvTable::insert(&mut env, "shop", "KV", KvNamespace::of("bare"));
        KvTable::insert(&mut env, "shop", "KV_SESSIONS", KvNamespace::of("sessions"));
        assert_eq!(env.kv["shop"].id, "bare");
        assert_eq!(env.kv_instances["shop"]["KV_SESSIONS"].id, "sessions");
        assert_eq!(
            KvTable::get(&env, "shop", "KV").map(|kv| kv.id.as_str()),
            Some("bare")
        );
        assert_eq!(
            KvTable::get(&env, "shop", "KV_SESSIONS").map(|kv| kv.id.as_str()),
            Some("sessions")
        );
        assert_eq!(KvTable::get(&env, "shop", "KV_OTHER"), None);
        assert_eq!(
            KvTable::entries(&env)
                .into_iter()
                .map(|(w, b, kv)| (w, b, kv.id.as_str()))
                .collect::<Vec<_>>(),
            vec![("shop", "KV", "bare"), ("shop", "KV_SESSIONS", "sessions")]
        );
        // Forgetting the last instance drops the Worker's map, so the file is as it was.
        KvTable::remove(&mut env, "shop", "KV_SESSIONS");
        assert!(env.kv_instances.is_empty());
        KvTable::remove(&mut env, "shop", "KV");
        assert_eq!(env, Environment::default());
    }

    #[test]
    fn a_plan_line_reuses_a_recorded_namespace_and_names_an_instance_by_its_binding() {
        let lock = lock_of(&[("default", "shop", "KV", "bare")]);
        let cx = PlanCx {
            ledger: LedgerView::of(&lock, "default"),
            worker: "shop",
        };
        let declared = Resources::default()
            .needs_kv()
            .kv_instances(&["KV_SESSIONS"]);
        let lines: Vec<_> = Kv::declared(&declared)
            .into_iter()
            .map(|decl| Kv::plan_line(&cx, decl))
            .collect();
        let mut out = String::new();
        for line in &lines {
            Kv::short_line(line, &mut out);
        }
        assert_eq!(
            out,
            "kv reuse shop\nkv create shop-sessions (KV_SESSIONS)\n"
        );
        assert_eq!(Kv::listing(&lines[0]), None, "nothing to create");
        assert_eq!(Kv::listing(&lines[1]), Some((KindId::Kv, None)));
    }

    #[test]
    fn a_namespace_to_create_that_the_account_holds_is_noted_not_reused() {
        struct Holds;
        impl on_account::Listings for Holds {
            fn names(&self, _: KindId, _: Option<&str>) -> Result<BTreeSet<String>, String> {
                Ok(BTreeSet::from(["shop".to_string()]))
            }
        }
        let on = on_account::OnAccount::read(&BTreeSet::from([(KindId::Kv, None)]), &Holds);
        let mut line = PlanKv {
            binding: "KV",
            action: "create",
            namespace: "shop".to_string(),
        };
        let mut notes = Vec::new();
        Kv::checked_against(&mut line, "shop-staging", &on, &mut notes);
        assert_eq!(line.action, "create", "the deploy cannot reuse it");
        assert_eq!(notes.len(), 1);
        assert!(
            notes[0].contains("`shop` (`KV` of `shop-staging`)"),
            "{}",
            notes[0]
        );
    }

    #[test]
    fn orphans_are_the_bare_namespace_of_a_worker_without_kv_and_each_undeclared_instance() {
        let lock = lock_of(&[
            ("default", "shop", "KV", "bare"),
            ("default", "shop", "KV_SESSIONS", "sessions"),
            ("default", "shop", "KV_OLD", "old"),
            ("default", "gone", "KV", "gone"),
        ]);
        let resources = project(vec![(
            "shop",
            Resources::default()
                .needs_kv()
                .kv_instances(&["KV_SESSIONS"]),
        )]);
        let cx = OrphanCx {
            env: &lock.environments["default"],
            resources: &resources,
        };
        let orphans = Kv::orphans(&cx);
        assert_eq!(
            orphans,
            vec![
                OrphanKv {
                    worker: "gone".to_string(),
                    binding: None
                },
                OrphanKv {
                    worker: "shop".to_string(),
                    binding: Some("KV_OLD".to_string())
                },
            ]
        );
        let mut out = String::new();
        for orphan in &orphans {
            Kv::short_orphan(orphan, &mut out);
        }
        assert_eq!(out, "orphan kv gone\norphan kv shop KV_OLD\n");
        assert!(orphans.iter().all(Kv::prunable));
        assert_eq!(Kv::shown_orphan(&orphans[1], "staging"), orphans[1]);
        assert_eq!(
            Kv::prune_notice(&orphans[0], "default"),
            Some(Notice::Styled(
                "bynk: will delete KV namespace for `gone`".to_string()
            ))
        );
        assert_eq!(
            Kv::prune_notice(&orphans[1], "staging"),
            Some(Notice::Plain(
                "bynk: will delete KV namespace `shop-old-staging` for `shop`'s `KV_OLD`"
                    .to_string()
            ))
        );
    }

    #[test]
    fn the_bound_ids_are_the_recorded_ones_of_the_declared_namespaces() {
        let lock = lock_of(&[
            ("default", "shop", "KV", "bare"),
            ("default", "shop", "KV_SESSIONS", "sessions"),
            ("staging", "shop", "KV", "staging-bare"),
        ]);
        let declared = Resources::default()
            .needs_kv()
            .kv_instances(&["KV_CART", "KV_SESSIONS"]);
        let mut ids = BTreeMap::new();
        kind_bound_ids(
            LedgerView::of(&lock, "default"),
            "shop",
            &declared,
            &mut ids,
        );
        assert_eq!(
            ids,
            BTreeMap::from([
                ("KV".to_string(), "bare".to_string()),
                ("KV_SESSIONS".to_string(), "sessions".to_string()),
            ])
        );
        let staging = LedgerView::of(&lock, "staging");
        assert_eq!(
            recorded_bound_id(staging, "shop", "KV"),
            Some("staging-bare".to_string())
        );
        assert_eq!(recorded_bound_id(staging, "shop", "KV_SESSIONS"), None);
        assert_eq!(recorded_bound_id(staging, "shop", "D1_ORDERS"), None);
    }

    #[test]
    fn ci_refuses_the_first_namespace_in_order_with_no_recorded_id() {
        let lock = lock_of(&[("default", "shop", "KV", "bare")]);
        let resources = project(vec![
            ("shop", Resources::default().needs_kv()),
            (
                "shop-cart",
                Resources::default().kv_instances(&["KV_SESSIONS"]),
            ),
        ]);
        let order = vec!["shop".to_string(), "shop-cart".to_string()];
        let cx = ProjectCx {
            ledger: LedgerView::of(&lock, "default"),
            resources: &resources,
            order: &order,
        };
        assert_eq!(
            Kv::unrecordable_in_ci(&cx).as_deref(),
            Some(
                "bynk: error: KV namespace `KV_SESSIONS` for `shop-cart` is unrecorded; provision locally first and commit bynk.deploy.lock"
            )
        );
        assert_eq!(kind_unrecordable_in_ci(&cx), Kv::unrecordable_in_ci(&cx));
    }

    #[test]
    fn a_trusted_recorded_namespace_is_bound_without_a_record_or_an_account_call() {
        let mut lock = lock_of(&[("default", "shop", "KV", "bare")]);
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let tools = Tools {
            provenance: &provenance,
            project_root: Path::new("/nonexistent"),
        };
        let resources = project(vec![("shop", Resources::default().needs_kv())]);
        // Unchecked (no live ids) and checked against an account that has it alike.
        for live in [None, Some(BTreeSet::from(["bare".to_string()]))] {
            let done = Kv::provision(
                &mut ApplyCx {
                    environment: "default",
                    worker: "shop",
                    project: &resources,
                    environments: &mut lock.environments,
                    live: &live,
                    tools,
                },
                "KV",
            )
            .expect("nothing to create");
            assert_eq!(
                done,
                Provisioned {
                    bound: Some(("KV".to_string(), "bare".to_string())),
                    recorded: None,
                }
            );
        }
    }

    #[test]
    fn pruning_an_orphan_with_no_recorded_id_does_nothing() {
        let mut lock = DeployLock::default();
        let provenance = Provenance::Path("/nonexistent/wrangler".into());
        let project = BTreeMap::new();
        let forgot = Kv::prune(
            &mut ApplyCx {
                environment: "default",
                worker: "gone",
                project: &project,
                environments: &mut lock.environments,
                live: &None,
                tools: Tools {
                    provenance: &provenance,
                    project_root: Path::new("/nonexistent"),
                },
            },
            &OrphanKv {
                worker: "gone".to_string(),
                binding: None,
            },
        )
        .expect("nothing to delete");
        assert_eq!(forgot, None);
        assert_eq!(lock, DeployLock::default());
    }

    #[test]
    fn materialising_no_ids_touches_no_configuration() {
        assert!(
            Kv::materialise(
                Path::new("/nonexistent/wrangler.toml"),
                "shop",
                &BTreeMap::new()
            )
            .is_ok()
        );
    }
}
