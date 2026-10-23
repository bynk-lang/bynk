use super::*;

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct DeployLock {
    // No serde `default`: a ledger with no `version` is not a fresh project, it
    // is corruption (a truncated write), and must fail the read rather than
    // parse as an empty v1 ledger that re-mints every namespace (#736).
    version: u32,
    #[serde(default)]
    pub(crate) environments: BTreeMap<String, Environment>,
}

/// #2077: what a Worker was compiled against — dependency (qualified context
/// name) → service → contract hash ([`WorkerRecord::expects`]).
pub(crate) type Expects = BTreeMap<String, BTreeMap<String, String>>;

fn lock_version() -> u32 {
    1
}

#[derive(Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Environment {
    /// Each Worker's bare `KV` namespace, the cloudflare adapter's `Kv`.
    #[serde(default)]
    pub(crate) kv: BTreeMap<String, KvNamespace>,
    /// S4b (#1967): each Worker's derived KV instances (`capability Sessions =
    /// Kv`), keyed by the instance's binding (`KV_SESSIONS`): the capability's
    /// name as the generated config carries it, which `deploy` reads rather
    /// than the source (ADR 0193 D3). A renamed capability is a new binding,
    /// so a new key: the plan shows a create and an orphan.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project with only the bare `Kv` writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) kv_instances: BTreeMap<String, BTreeMap<String, KvNamespace>>,
    /// P5 (#2323): each Worker's `R2` instances (`capability Archives = R2`),
    /// keyed by the instance's binding (`R2_ARCHIVES`), as `kv_instances` is.
    /// A bucket has no minted id (its name is its identity), so the record is
    /// the name and jurisdiction it was created under: what a changed
    /// `@jurisdiction` is compared against, and what an orphan names.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project without R2 writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) r2_instances: BTreeMap<String, BTreeMap<String, R2Bucket>>,
    /// P3 (#2342): each Worker's `RateLimit` instances, keyed by binding
    /// (`RATELIMIT_LOGIN_LIMIT`), as `kv_instances` is. The `namespace_id` is
    /// minted here, not by Cloudflare: a positive integer unique across every
    /// environment of the ledger, since environments may share an account
    /// and the id is unique per account.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project without rate limiting writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) ratelimit_instances: BTreeMap<String, BTreeMap<String, RateLimitNamespace>>,
    /// Slice 2: which Workers this project has ever pushed. Additive and
    /// `default`ed, so a slice-0 ledger still reads.
    ///
    /// KV state alone could not answer "does this Worker exist on the account?"
    /// — a context with no KV has no `kv` entry at all — and `--context` must
    /// know, because a Service Binding to an absent Worker fails at upload.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    workers: BTreeMap<String, WorkerRecord>,
    /// Slice 1: the queue names this project has created at least once.
    ///
    /// Environment-wide rather than per-worker, because a queue is an account
    /// resource addressed by name, not something a Worker owns — two contexts
    /// consuming `"jobs"` mean the same queue.
    ///
    /// **Authoritative for nothing** (ADR 0194 D2). It exists so the plan can
    /// say `create` or `reuse` without a `wrangler queues list` call; the
    /// provision step attempts the create regardless, so a queue deleted
    /// out-of-band comes back rather than being skipped on this set's word.
    /// Additive and `default`ed, so a slice-0 or slice-2 ledger still reads.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    queues: BTreeSet<String>,
    /// State-migrations S3 (#2248, #2253 review): per Worker, the agent classes
    /// whose `deleted` tombstone a successful push carried in this
    /// environment. A `removed agent` declaration stays in source (and its
    /// tombstone in the config) until Cloudflare reports it stale, so later
    /// plans show the deletion as already applied and do not ask to confirm
    /// it again. Replaced on every push of the Worker, so a declaration that is
    /// later removed leaves nothing behind. A record of what was confirmed and
    /// pushed — never a claim about the namespace, which Cloudflare owns (ADR
    /// 0194 D1). Additive and `default`ed, so an older ledger still reads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    deleted_classes: BTreeMap<String, BTreeSet<String>>,
    /// #2276 Decision D: per Worker, the agent class renames (old name → new
    /// name) whose `renamed` tombstone a successful push carried in this
    /// environment, so later plans show them as applied. Replaced on every
    /// push of the Worker, like [`Environment::deleted_classes`], and for the
    /// same reason a record, never a claim about the namespace. Additive and
    /// `default`ed, so an older ledger still reads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    renamed_classes: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct KvNamespace {
    pub(crate) id: String,
}

/// P3 (#2342): the `namespace_id` this project allocated for one rate limiter.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RateLimitNamespace {
    pub(crate) namespace_id: String,
}

/// P3 (#2342): the first `namespace_id` the ledger allocates. Low ids are the
/// ones a hand-written Worker on the same account is likeliest to have chosen.
pub(crate) const FIRST_RATELIMIT_NAMESPACE: u64 = 1000;

/// P5 (#2323): one R2 bucket this project created, as the account names it.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct R2Bucket {
    /// The bucket's account-facing name, environment-qualified.
    pub(crate) name: String,
    /// The jurisdiction it was created in, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jurisdiction: Option<String>,
    /// Buckets this binding used to have, left in place when a changed
    /// `@jurisdiction` made a new one. They hold data, so they stay recorded
    /// (and are reported as orphans) until their entry is removed by hand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) superseded: Vec<R2Past>,
}

/// A bucket a binding no longer uses (see [`R2Bucket::superseded`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct R2Past {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jurisdiction: Option<String>,
}

/// What the ledger remembers about one pushed Worker. A struct rather than a
/// bare bool so slice 3's secrets have somewhere to land.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct WorkerRecord {
    deployed: bool,
    /// v0.177 (#643): the contract hash this Worker *provides* per `on call`
    /// service, as of the push that recorded it — what is **live**.
    ///
    /// This is what makes a skew visible before a request finds it: a later
    /// `deploy --context A` compares A's compiled `expects` against these, and
    /// refuses rather than shipping a caller that will 409 in production.
    ///
    /// `None` means **no record** — a Worker pushed by a pre-v0.177 driver, which
    /// has nothing to say about contracts either way. `Some({})` means the
    /// Worker is *known* to provide no `on call` service at all.
    ///
    /// The distinction is load-bearing, and an empty map cannot carry it: a
    /// callee that removes **all** its services emits no manifest, so a
    /// bare-`BTreeMap` field would record `{}` — indistinguishable from "old
    /// ledger" — and the gate's `continue` would let a total, real skew through.
    /// `Option` keeps "silence is not a match" while still catching removal.
    ///
    /// Additive and `default`ed, so a pre-v0.177 ledger still reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contracts: Option<BTreeMap<String, String>>,
    /// #2077: the contract hash this Worker was compiled against for each
    /// dependency service it calls, as of the push that recorded it —
    /// dependency (qualified context name) → service → hash, the build's
    /// `bynk-contracts.json` `expects` record. What its live call sites stamp.
    ///
    /// The other half of [`WorkerRecord::contracts`]: a later `deploy
    /// --context B` compares B's newly built `provides` against every other
    /// live Worker's `expects`, and refuses rather than shipping a callee its
    /// live callers would 409 against.
    ///
    /// `None` means **no record** — a Worker pushed by a driver that predates
    /// this field, which cannot be checked (the gate warns, never refuses, on
    /// it: silence is not a match). `Some({})` means the Worker is *known* to
    /// call no `on call` service at all. Additive and `default`ed, so an older
    /// ledger still reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    expects: Option<Expects>,
    /// #1883, slice 1b: the static files this Worker serves, as of the push
    /// that recorded it — each one's served path (`/_assets/board.<hash>.css`).
    /// The content hash is in the path, so a changed, added or removed file
    /// is a changed line in the committed lock's diff. Empty (and omitted)
    /// for a Worker with none; additive and `default`ed, so an older lock
    /// still reads.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    assets: BTreeSet<String>,
    /// I4 (#1970): the contract each declared foreign caller may call this
    /// Worker at, as of the push that recorded it — actor → service → hash, the
    /// build's `bynk-contracts.json` `foreign` record. The foreign callers are
    /// not Bynk and record nothing themselves, so this is where the ledger
    /// learns of them, beside what the Worker `provides`. Empty (and omitted)
    /// for a Worker with none; additive and `default`ed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    foreign: BTreeMap<String, BTreeMap<String, String>>,
}

impl DeployLock {
    pub(crate) fn is_deployed(&self, environment: &str, worker: &str) -> bool {
        self.environments
            .get(environment)
            .and_then(|env| env.workers.get(worker))
            .is_some_and(|record| record.deployed)
    }

    /// State-migrations S4 (#2259): every environment this ledger records
    /// `worker` as pushed in, sorted. A floor-raising `bynk migrate sweep`
    /// must cover all of them.
    pub(crate) fn deployed_environments(&self, worker: &str) -> Vec<String> {
        self.environments
            .iter()
            .filter(|(env, _)| self.is_deployed(env, worker))
            .map(|(env, _)| env.clone())
            .collect()
    }

    /// State-migrations S3 (#2248): whether a push in `environment` already
    /// carried `class`'s `deleted` tombstone for `worker`.
    pub(crate) fn has_deleted_class(&self, environment: &str, worker: &str, class: &str) -> bool {
        self.environments
            .get(environment)
            .and_then(|env| env.deleted_classes.get(worker))
            .is_some_and(|classes| classes.contains(class))
    }

    /// State-migrations S3 (#2248): record the `deleted` tombstones a
    /// successful push of `worker` carried, replacing what was recorded for it.
    pub(crate) fn record_deleted_classes(
        &mut self,
        environment: &str,
        worker: &str,
        classes: BTreeSet<String>,
    ) {
        let env = self
            .environments
            .entry(environment.to_string())
            .or_default();
        if classes.is_empty() {
            env.deleted_classes.remove(worker);
        } else {
            env.deleted_classes.insert(worker.to_string(), classes);
        }
    }

    /// #2276 Decision D: whether a push in `environment` already carried the
    /// rename `from → to` for `worker`.
    pub(crate) fn has_renamed_class(
        &self,
        environment: &str,
        worker: &str,
        from: &str,
        to: &str,
    ) -> bool {
        self.environments
            .get(environment)
            .and_then(|env| env.renamed_classes.get(worker))
            .and_then(|renames| renames.get(from))
            .is_some_and(|recorded| recorded == to)
    }

    /// #2276 Decision D: record the `renamed` tombstones a successful push of
    /// `worker` carried, replacing what was recorded for it.
    pub(crate) fn record_renamed_classes(
        &mut self,
        environment: &str,
        worker: &str,
        renames: BTreeMap<String, String>,
    ) {
        let env = self
            .environments
            .entry(environment.to_string())
            .or_default();
        if renames.is_empty() {
            env.renamed_classes.remove(worker);
        } else {
            env.renamed_classes.insert(worker.to_string(), renames);
        }
    }

    pub(crate) fn record_deployed(
        &mut self,
        environment: &str,
        worker: &str,
        contracts: Option<BTreeMap<String, String>>,
    ) {
        self.environments
            .entry(environment.to_string())
            .or_default()
            .workers
            .insert(
                worker.to_string(),
                WorkerRecord {
                    deployed: true,
                    contracts,
                    expects: None,
                    assets: BTreeSet::new(),
                    foreign: BTreeMap::new(),
                },
            );
    }

    /// #1883, slice 1b: record the served paths of the static files `worker`
    /// shipped in this push ([`WorkerRecord::assets`]). Called right after
    /// [`DeployLock::record_deployed`]; a Worker not yet recorded is ignored.
    pub(crate) fn record_assets(
        &mut self,
        environment: &str,
        worker: &str,
        assets: BTreeSet<String>,
    ) {
        if let Some(record) = self
            .environments
            .get_mut(environment)
            .and_then(|env| env.workers.get_mut(worker))
        {
            record.assets = assets;
        }
    }

    /// I4 (#1970): record the contract `worker`'s declared foreign callers may
    /// call it at, as of this push ([`WorkerRecord::foreign`]). Called right
    /// after [`DeployLock::record_deployed`]; a Worker not yet recorded is
    /// ignored.
    pub(crate) fn record_foreign(
        &mut self,
        environment: &str,
        worker: &str,
        foreign: BTreeMap<String, BTreeMap<String, String>>,
    ) {
        if let Some(record) = self
            .environments
            .get_mut(environment)
            .and_then(|env| env.workers.get_mut(worker))
        {
            record.foreign = foreign;
        }
    }

    /// #2077: record the contracts `worker` was compiled against as of this
    /// push ([`WorkerRecord::expects`]). Called right after
    /// [`DeployLock::record_deployed`]; a Worker not yet recorded is ignored.
    /// `Some` even when empty: this build *knows* what the Worker calls.
    pub(crate) fn record_expects(&mut self, environment: &str, worker: &str, expects: Expects) {
        if let Some(record) = self
            .environments
            .get_mut(environment)
            .and_then(|env| env.workers.get_mut(worker))
        {
            record.expects = Some(expects);
        }
    }

    /// #2077: every Worker the ledger records as deployed in `environment`,
    /// with what it was compiled against — `None` when its record predates
    /// [`WorkerRecord::expects`].
    pub(crate) fn live_expects(
        &self,
        environment: &str,
    ) -> impl Iterator<Item = (&str, Option<&Expects>)> {
        self.environments
            .get(environment)
            .into_iter()
            .flat_map(|env| env.workers.iter())
            .filter(|(_, record)| record.deployed)
            .map(|(worker, record)| (worker.as_str(), record.expects.as_ref()))
    }

    /// I4 (#1970): what the ledger recorded `worker`'s foreign callers may call
    /// it at, empty when it recorded none.
    #[cfg(test)]
    pub(crate) fn live_foreign(
        &self,
        environment: &str,
        worker: &str,
    ) -> Option<&BTreeMap<String, BTreeMap<String, String>>> {
        self.environments
            .get(environment)
            .and_then(|env| env.workers.get(worker))
            .map(|record| &record.foreign)
    }

    /// v0.177 (#643): what the ledger believes `worker` currently provides.
    ///
    /// `None` for both "never deployed" and "deployed before contracts were
    /// recorded" — in each case the ledger cannot speak, and the gate must not
    /// invent an answer.
    pub(crate) fn live_contracts(
        &self,
        environment: &str,
        worker: &str,
    ) -> Option<&BTreeMap<String, String>> {
        self.environments
            .get(environment)
            .and_then(|env| env.workers.get(worker))
            .and_then(|record| record.contracts.as_ref())
    }

    /// Record the namespace `id` this project created for `worker`'s KV
    /// `binding`: the bare `KV` in [`Environment::kv`], an instance in
    /// [`Environment::kv_instances`] (S4b, #1967).
    pub(crate) fn record_kv(&mut self, environment: &str, worker: &str, binding: &str, id: &str) {
        let env = self
            .environments
            .entry(environment.to_string())
            .or_default();
        let namespace = KvNamespace { id: id.to_string() };
        if is_bare_kv(binding) {
            env.kv.insert(worker.to_string(), namespace);
        } else {
            env.kv_instances
                .entry(worker.to_string())
                .or_default()
                .insert(binding.to_string(), namespace);
        }
    }

    /// P5 (#2323): record the bucket this project created for `worker`'s `R2`
    /// `binding`.
    pub(crate) fn record_r2(
        &mut self,
        environment: &str,
        worker: &str,
        binding: &str,
        name: &str,
        jurisdiction: Option<&str>,
    ) {
        let instances = self
            .environments
            .entry(environment.to_string())
            .or_default()
            .r2_instances
            .entry(worker.to_string())
            .or_default();
        // A bucket this replaces is not forgotten: its objects are still there.
        let mut superseded = Vec::new();
        if let Some(old) = instances.remove(binding) {
            superseded = old.superseded;
            if old.name != name || old.jurisdiction.as_deref() != jurisdiction {
                superseded.push(R2Past {
                    name: old.name,
                    jurisdiction: old.jurisdiction,
                });
            }
        }
        instances.insert(
            binding.to_string(),
            R2Bucket {
                name: name.to_string(),
                jurisdiction: jurisdiction.map(str::to_string),
                superseded,
            },
        );
    }

    /// P3 (#2342): the `namespace_id` for `worker`'s rate limiter `binding`
    /// in `environment`: the recorded one, else a new one — the smallest
    /// integer from [`FIRST_RATELIMIT_NAMESPACE`] that no rate limiter in any
    /// environment holds — recorded now. `true` when it was allocated.
    pub(crate) fn allocate_ratelimit(
        &mut self,
        environment: &str,
        worker: &str,
        binding: &str,
    ) -> (String, bool) {
        if let Some(id) = recorded_ratelimit(self, worker, binding, environment) {
            return (id.to_string(), false);
        }
        let taken: BTreeSet<&str> = self
            .environments
            .values()
            .flat_map(|env| env.ratelimit_instances.values())
            .flat_map(|instances| instances.values())
            .map(|r| r.namespace_id.as_str())
            .collect();
        let id = (FIRST_RATELIMIT_NAMESPACE..)
            .map(|n| n.to_string())
            .find(|n| !taken.contains(n.as_str()))
            .expect("an unused id");
        self.environments
            .entry(environment.to_string())
            .or_default()
            .ratelimit_instances
            .entry(worker.to_string())
            .or_default()
            .insert(
                binding.to_string(),
                RateLimitNamespace {
                    namespace_id: id.clone(),
                },
            );
        (id, true)
    }

    /// P3 (#2342): forget `worker`'s rate limiters that `declared` no longer
    /// names. A limiter holds no data and has no account resource, so a
    /// removed instance needs nothing deleted, only its row dropped (its id
    /// is then free again). `true` when anything was forgotten.
    pub(crate) fn forget_undeclared_ratelimits(
        &mut self,
        environment: &str,
        worker: &str,
        declared: &[String],
    ) -> bool {
        let Some(env) = self.environments.get_mut(environment) else {
            return false;
        };
        let Some(instances) = env.ratelimit_instances.get_mut(worker) else {
            return false;
        };
        let before = instances.len();
        instances.retain(|binding, _| declared.contains(binding));
        let changed = instances.len() != before;
        if instances.is_empty() {
            env.ratelimit_instances.remove(worker);
        }
        changed
    }

    /// Forget `worker`'s KV `binding`, dropping a Worker's instance map once
    /// it is empty, so a pruned ledger is the file it would have been had the
    /// instance never existed.
    fn forget_kv(&mut self, environment: &str, worker: &str, binding: &str) {
        let Some(env) = self.environments.get_mut(environment) else {
            return;
        };
        if is_bare_kv(binding) {
            env.kv.remove(worker);
        } else if let Some(instances) = env.kv_instances.get_mut(worker) {
            instances.remove(binding);
            if instances.is_empty() {
                env.kv_instances.remove(worker);
            }
        }
    }

    pub(crate) fn has_queue(&self, environment: &str, queue: &str) -> bool {
        self.environments
            .get(environment)
            .is_some_and(|env| env.queues.contains(queue))
    }

    /// Note that this project has created `queue`. Returns whether the ledger
    /// changed, so a re-run that provisions nothing also writes nothing.
    pub(crate) fn record_queue(&mut self, environment: &str, queue: &str) -> bool {
        self.environments
            .entry(environment.to_string())
            .or_default()
            .queues
            .insert(queue.to_string())
    }
}

/// Slice 5: every ledger entry for this environment that the current build no
/// longer declares. Owned, not borrowed — ledger-derived names don't share
/// `Plan<'a>`'s lifetime over `order`/`resources` (the `PlanSecret.name:
/// String` precedent below).
///
/// `kv` and `workers` are independent checks, deliberately not merged: both
/// are keyed by worker name in the ledger (`Environment`, above), so a
/// context removed from source that had KV is reported as **two** orphans,
/// one per map, not one combined line. `--prune` (when it lands) treats them
/// independently too — the `kv` line is prunable, the `workers` line is
/// report-only (a whole-Worker delete is a materially larger blast radius
/// than a namespace or a queue, and out of this slice's scope).
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub(crate) struct Orphans {
    pub(crate) kv: Vec<OrphanKv>,
    /// P5 (#2323): recorded R2 buckets the build no longer declares. Reported
    /// only: a bucket holds data, so `--prune` never deletes one (and R2
    /// refuses to delete a non-empty one anyway).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) r2: Vec<OrphanR2>,
    pub(crate) workers: Vec<String>,
    pub(crate) queues: Vec<String>,
}

/// One recorded KV namespace the build no longer declares (S4b, #1967): the
/// bare one of a removed Worker (`binding` absent), or a derived instance,
/// whose Worker may still be live — a capability removed or renamed.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct OrphanKv {
    pub(crate) worker: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) binding: Option<String>,
}

/// One recorded R2 bucket the build no longer declares (P5, #2323): a
/// capability removed or renamed, or a whole Worker gone.
#[derive(Debug, PartialEq, Eq, Serialize)]
pub(crate) struct OrphanR2 {
    pub(crate) worker: String,
    pub(crate) binding: String,
    /// The bucket's name on the account, which stays there.
    pub(crate) bucket: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) jurisdiction: Option<String>,
}

impl OrphanKv {
    /// The binding whose ledger entry this is: `KV` for the bare namespace.
    pub(crate) fn binding(&self) -> &str {
        self.binding
            .as_deref()
            .unwrap_or(bynk_emit::emitter::wrangler::KV_BINDING_NAME)
    }
}

impl Orphans {
    /// Whether `--prune` would actually delete anything. **Not** the same as
    /// checking whether every field is empty (review, #840): `workers`
    /// orphans are report-only (DECISION C — never `wrangler delete`), so a
    /// project whose only orphan is an unprunable Worker must not trigger
    /// `confirm_prune`'s prompt at all — `Delete 0 resource(s)?` is a bug,
    /// not a valid state.
    pub(crate) fn has_prunable(&self) -> bool {
        !self.kv.is_empty() || !self.queues.is_empty()
    }

    /// These orphans as the account names them under `environment`, for the
    /// plan to show (#2028): a queue by the [`env_qualify`]d name `--prune`
    /// deletes, a Worker by its [`deployed_worker_name`]. A KV orphan keeps
    /// its Worker's ledger key: its line names the ledger entry, not the
    /// namespace's title ([`kv_title`]), which a namespace made before #2037
    /// may not match.
    pub(crate) fn as_named_in(&self, environment: &str) -> Orphans {
        Orphans {
            kv: self
                .kv
                .iter()
                .map(|kv| OrphanKv {
                    worker: kv.worker.clone(),
                    binding: kv.binding.clone(),
                })
                .collect(),
            r2: self
                .r2
                .iter()
                .map(|r2| OrphanR2 {
                    worker: r2.worker.clone(),
                    binding: r2.binding.clone(),
                    bucket: r2.bucket.clone(),
                    jurisdiction: r2.jurisdiction.clone(),
                })
                .collect(),
            workers: self
                .workers
                .iter()
                .map(|worker| deployed_worker_name(environment, worker))
                .collect(),
            queues: self
                .queues
                .iter()
                .map(|queue| env_qualify(environment, queue))
                .collect(),
        }
    }
}

/// The orphan diff: ledger vs. the current build's full declared resource
/// set, regardless of `--context` — `resources` already spans the *whole*
/// project (`project_resources`, called from `run()` over `available` — every
/// worker `workers::discover_workers` found — not the `--context`-narrowed
/// `order`/`selected`), so `resources.keys()` alone is the full live-worker
/// set and this reuses data `run()` already has rather than reading anything
/// new. No live Cloudflare call: the report half of reconciliation costs
/// nothing and needs no auth, keeping `--dry-run`'s "never authenticates"
/// promise intact.
///
/// Pure, so the diff — including the shared-queue case (a queue two contexts
/// still consume must never appear orphaned because a *third* context that
/// used to consume it was removed) — is tested without a build tree.
pub(crate) fn find_orphans(
    lock: &DeployLock,
    environment: &str,
    resources: &BTreeMap<String, Resources>,
) -> Orphans {
    let Some(env) = lock.environments.get(environment) else {
        return Orphans::default();
    };
    let live_queues: BTreeSet<&str> = resources
        .values()
        .flat_map(|r| r.queues.iter().map(String::as_str))
        .collect();
    // S4b (#1967): an instance is orphaned with its Worker, or on its own
    // when its Worker no longer declares it.
    let instance_orphans = env.kv_instances.iter().flat_map(|(worker, instances)| {
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
    // P5 (#2323): a bucket likewise, with its Worker or on its own.
    let r2_orphans = env
        .r2_instances
        .iter()
        .flat_map(|(worker, instances)| {
            instances
                .iter()
                .filter(move |(binding, _)| {
                    resources
                        .get(worker)
                        .is_none_or(|r| !r.r2_buckets.iter().any(|b| &b.binding == *binding))
                })
                .map(move |(binding, bucket)| OrphanR2 {
                    worker: worker.clone(),
                    binding: binding.clone(),
                    bucket: bucket.name.clone(),
                    jurisdiction: bucket.jurisdiction.clone(),
                })
        })
        .chain(env.r2_instances.iter().flat_map(|(worker, instances)| {
            // A bucket a changed jurisdiction left behind is an orphan while
            // its binding is live, too.
            instances.iter().flat_map(move |(binding, bucket)| {
                bucket.superseded.iter().map(move |past| OrphanR2 {
                    worker: worker.clone(),
                    binding: binding.clone(),
                    bucket: past.name.clone(),
                    jurisdiction: past.jurisdiction.clone(),
                })
            })
        }))
        .collect();
    Orphans {
        r2: r2_orphans,
        // The bare namespace likewise: with its Worker, or on its own once
        // the live Worker stops using the bare `Kv` (review of #1974).
        kv: env
            .kv
            .keys()
            .filter(|worker| resources.get(worker.as_str()).is_none_or(|r| !r.needs_kv))
            .map(|worker| OrphanKv {
                worker: worker.clone(),
                binding: None,
            })
            .chain(instance_orphans)
            .collect(),
        workers: env
            .workers
            .keys()
            .filter(|worker| !resources.contains_key(worker.as_str()))
            .cloned()
            .collect(),
        queues: env
            .queues
            .iter()
            .filter(|queue| !live_queues.contains(queue.as_str()))
            .cloned()
            .collect(),
    }
}

/// The bucket recorded for `worker`'s `R2` `binding` (P5, #2323).
pub(crate) fn recorded_r2<'a>(
    lock: &'a DeployLock,
    worker: &str,
    binding: &str,
    environment: &str,
) -> Option<&'a R2Bucket> {
    lock.environments
        .get(environment)
        .and_then(|env| env.r2_instances.get(worker))
        .and_then(|instances| instances.get(binding))
}

/// The `namespace_id` recorded for `worker`'s rate limiter `binding` (P3,
/// #2342).
pub(crate) fn recorded_ratelimit<'a>(
    lock: &'a DeployLock,
    worker: &str,
    binding: &str,
    environment: &str,
) -> Option<&'a str> {
    lock.environments
        .get(environment)
        .and_then(|env| env.ratelimit_instances.get(worker))
        .and_then(|instances| instances.get(binding))
        .map(|r| r.namespace_id.as_str())
}

/// Whether `binding` is the bare `Kv`'s, as opposed to a derived instance's.
pub(crate) fn is_bare_kv(binding: &str) -> bool {
    binding == bynk_emit::emitter::wrangler::KV_BINDING_NAME
}

/// The namespace id recorded for `worker`'s KV `binding` (S4b, #1967): the
/// bare `KV`, or a derived instance.
pub(crate) fn recorded_kv_binding<'a>(
    lock: &'a DeployLock,
    worker: &str,
    binding: &str,
    environment: &str,
) -> Option<&'a str> {
    if is_bare_kv(binding) {
        return recorded_kv(lock, worker, environment);
    }
    lock.environments
        .get(environment)
        .and_then(|env| env.kv_instances.get(worker))
        .and_then(|instances| instances.get(binding))
        .map(|kv| kv.id.as_str())
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

pub(crate) fn recorded_kv<'a>(
    lock: &'a DeployLock,
    worker: &str,
    environment: &str,
) -> Option<&'a str> {
    lock.environments
        .get(environment)
        .and_then(|env| env.kv.get(worker))
        .map(|kv| kv.id.as_str())
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

pub(crate) fn read_lock(path: &Path) -> Result<DeployLock, String> {
    if !path.exists() {
        return Ok(DeployLock {
            version: lock_version(),
            ..Default::default()
        });
    }
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    // A zero-byte or whitespace-only ledger is a truncated write, not an empty
    // project. Accepting it as an empty v1 ledger would tell the planner that no
    // namespaces exist and re-mint every one — the exact orphaning the ledger
    // exists to prevent (#736, ADR 0180). Fail hard so the operator restores it.
    if text.trim().is_empty() {
        return Err(format!(
            "deploy ledger `{}` is empty or truncated (corrupt); refusing to \
             treat it as a fresh project — restore it from version control",
            path.display()
        ));
    }
    // A file that does not parse — including one truncated mid-table or missing
    // its now-required `version` — is corruption too, and gets the same
    // restore-it guidance rather than a bare toml diagnostic. A version we simply
    // do not support is a distinct case (a newer or older format), not corruption.
    let lock: DeployLock = toml::from_str(&text).map_err(|e| {
        format!(
            "deploy ledger `{}` is corrupt ({e}) — restore it from version control",
            path.display()
        )
    })?;
    if lock.version != lock_version() {
        return Err(format!("unsupported deploy lock version {}", lock.version));
    }
    Ok(lock)
}

/// Distinguishes concurrent temp ledgers written by the same process.
static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn write_lock(path: &Path, lock: &DeployLock) -> Result<(), String> {
    let body = toml::to_string_pretty(lock).map_err(|e| e.to_string())?;
    // Atomic, durable replace: write a sibling temp file, fsync it, then rename
    // it over the ledger. A power loss or kill can then only leave the intact old
    // file or the intact new one — never a truncated ledger that reads as empty
    // (#736). Atomicity-for-readers (the rename) is not enough on its own: after
    // a crash the rename can be journaled while the temp's data blocks are still
    // only in the page cache, so we `sync_all` the data before the rename and
    // fsync the directory after it to make the new name itself durable.
    let dir = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(LOCK_FILE);

    // A per-process counter keeps the temp name unique within a process, and
    // `create_new` makes the create exclusive — a stale temp from a prior crash
    // or a pre-planted symlink at this path is refused rather than followed.
    let (tmp, mut file) = loop {
        let n = TMP_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let candidate = dir.join(format!(".{file_name}.{}.{n}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => break (candidate, f),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    };

    // From here on, any failure must remove the temp so a full disk or crash
    // does not litter the project with `.bynk.deploy.lock.*.tmp` files.
    let write_then_sync = file
        .write_all(body.as_bytes())
        .and_then(|()| file.sync_all());
    if let Err(e) = write_then_sync {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    drop(file);

    // Preserve the ledger's existing permissions across the replace.
    if let Ok(meta) = std::fs::metadata(path) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }

    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.to_string());
    }
    // Best-effort: make the rename itself durable. Directory fsync is a no-op or
    // unsupported on some platforms, so a failure here is not fatal.
    if let Ok(dir_file) = std::fs::File::open(dir) {
        let _ = dir_file.sync_all();
    }
    Ok(())
}

/// Fill a generated worker configuration from the committed deploy ledger.
/// This is shared with `bynk dev -- --remote`; local dev leaves placeholders
/// alone because Miniflare does not read the Cloudflare namespace id.
///
/// `environment` (slice 4, #837 review): before `--env` existed every real
/// deploy recorded into `"default"` regardless, so hardcoding it here always
/// matched. A project deployed only under a non-default `--env` now has
/// nothing under `"default"` — reading the wrong section would misreport a
/// provisioned project as never deployed, so this reads whichever section
/// `bynk dev --env NAME -- --remote` names (default `"default"`, unchanged).
pub fn materialise_deploy_state(
    project_root: &Path,
    worker: &str,
    config: &Path,
    environment: &str,
) -> Result<bool, String> {
    // P7.4 (#1305): structural, not a substring search — closes R7.6/R8.20.
    let text = std::fs::read_to_string(config).map_err(|e| e.to_string())?;
    // S4b (#1967): every namespace still at its placeholder, the bare `KV`
    // and each derived instance alike.
    let needed = bynk_emit::emitter::wrangler::kv_bindings_needing_ids(&text)?;
    // P5 (#2323): each `R2` bucket must have been created by a deploy, and is
    // then named as that environment names it.
    let buckets = bynk_emit::emitter::wrangler::r2_bucket_entries(&text)?;
    // P3 (#2342): each rate limiter must have had its id allocated by a deploy.
    let limiters = bynk_emit::emitter::wrangler::ratelimits_needing_ids(&text)?;
    if needed.is_empty() && buckets.is_empty() && limiters.is_empty() {
        return Ok(false);
    }
    let lock = read_lock(&project_root.join(LOCK_FILE))?;
    let mut limiter_ids = BTreeMap::new();
    for binding in limiters {
        let Some(id) = recorded_ratelimit(&lock, worker, &binding, environment) else {
            return Err(format!(
                "remote rate limiter `{binding}` for `{worker}` has not been provisioned under environment `{environment}`; run `bynk deploy --env {environment}` first"
            ));
        };
        limiter_ids.insert(binding, id.to_string());
    }
    let mut names = BTreeMap::new();
    for (binding, name, jurisdiction) in &buckets {
        let Some(recorded) = recorded_r2(&lock, worker, binding, environment) else {
            return Err(format!(
                "remote R2 bucket `{binding}` for `{worker}` has not been provisioned under environment `{environment}`; run `bynk deploy --env {environment}` first"
            ));
        };
        if recorded.jurisdiction != *jurisdiction {
            return Err(format!(
                "remote R2 bucket `{binding}` for `{worker}` was created under another `@jurisdiction` than the source now names; run `bynk deploy --env {environment}` first"
            ));
        }
        names.insert(binding.clone(), r2_bucket_name(environment, name));
    }
    let mut changed = false;
    if !limiter_ids.is_empty() {
        if !materialise_ratelimit_ids(config, &limiter_ids) {
            return Err("could not write generated configuration".into());
        }
        changed = true;
    }
    let text = std::fs::read_to_string(config).map_err(|e| e.to_string())?;
    if !names.is_empty() {
        let patched = bynk_emit::emitter::wrangler::materialise_r2_bucket_names(&text, &names)?;
        if patched != text {
            std::fs::write(config, patched).map_err(|e| e.to_string())?;
            changed = true;
        }
    }
    if needed.is_empty() {
        return Ok(changed);
    }
    let mut ids = BTreeMap::new();
    for binding in needed {
        let Some(id) = recorded_kv_binding(&lock, worker, &binding, environment) else {
            let what = if is_bare_kv(&binding) {
                format!("remote KV for `{worker}`")
            } else {
                format!("remote KV namespace `{binding}` for `{worker}`")
            };
            return Err(format!(
                "{what} has not been provisioned under environment `{environment}`; run `bynk deploy --env {environment}` first"
            ));
        };
        ids.insert(binding, id.to_string());
    }
    if materialise_kv_ids(config, &ids) {
        Ok(true)
    } else {
        Err("could not write generated configuration".into())
    }
}

// ---------------------------------------------------------------------------
// Slice 5: reconciliation — orphan pruning
// ---------------------------------------------------------------------------

/// Print exactly what `--prune` is about to delete, then ask once for the
/// whole batch — the "strictly stronger gate" the track doc (§6) calls for on
/// top of [`confirm`]'s creation gate. `--yes` alone does **not** imply this:
/// a CI job that wants unattended pruning must pass `--yes` **and**
/// `--prune` together, the same non-interactive-requires-`--yes` shape
/// [`confirm`] already uses, just with its own prompt so a script that only
/// meant to authorise *creation* cannot accidentally also authorise deletion.
pub(crate) fn confirm_prune(yes: bool, orphans: &Orphans, environment: &str) -> bool {
    for kv in &orphans.kv {
        match &kv.binding {
            None => eprintln!("bynk: will delete KV namespace for `{}`", kv.worker),
            Some(binding) => eprintln!(
                "bynk: will delete KV namespace `{}` for `{}`'s `{binding}`",
                kv_title(&kv.worker, binding, environment),
                kv.worker
            ),
        }
    }
    for queue in &orphans.queues {
        // The name `prune_orphans` deletes, not the ledger key (#2028).
        eprintln!(
            "bynk: will delete queue `{}`",
            env_qualify(environment, queue)
        );
    }
    if yes {
        return true;
    }
    if !requires_interactive_confirmation(yes, io::stdin().is_terminal()) {
        eprintln!("bynk: refusing to prune in a non-interactive session without --yes");
        return false;
    }
    let count = orphans.kv.len() + orphans.queues.len();
    eprint!("Delete {count} resource(s)? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Delete every KV and queue orphan `confirm_prune` just named — never a
/// worker (DECISION C: `wrangler delete`'s blast radius, routes/domains/crons
/// along with the script, is categorically larger than a namespace or a
/// queue, and pruning a whole Worker is explicitly out of this slice).
///
/// The ledger entry is stripped whether the delete found something to remove
/// or found it already gone (DECISION E) — both mean "this resource is not
/// there", and treating only a clean delete as ledger-worthy would wedge a
/// half-completed prune: crash between a successful Cloudflare delete and the
/// ledger write, and the next run would re-report the same orphan and
/// re-issue a delete Cloudflare now rejects as not-found.
pub(crate) fn prune_orphans(
    provenance: &Provenance,
    project_root: &Path,
    lock: &mut DeployLock,
    lock_path: &Path,
    environment: &str,
    orphans: &Orphans,
) -> Result<(), DeployFailure> {
    for orphan in &orphans.kv {
        let (worker, binding) = (orphan.worker.as_str(), orphan.binding());
        let Some(id) = recorded_kv_binding(lock, worker, binding, environment).map(str::to_owned)
        else {
            continue;
        };
        let what = match &orphan.binding {
            None => format!("the orphaned KV namespace for `{worker}`"),
            Some(binding) => format!("the orphaned KV namespace `{binding}` for `{worker}`"),
        };
        delete_kv_namespace(provenance, &id, project_root)
            .map_err(|e| DeployFailure::driver(format!("could not delete {what}: {e}")))?;
        lock.forget_kv(environment, worker, binding);
        write_lock(lock_path, lock).map_err(|e| {
            DeployFailure::driver(format!(
                "deleted {what} but could not record it in {}: {e}",
                lock_path.display()
            ))
        })?;
    }
    for queue in &orphans.queues {
        let physical = env_qualify(environment, queue);
        delete_queue(provenance, &physical, project_root).map_err(|e| {
            DeployFailure::driver(format!(
                "could not delete the orphaned queue `{physical}`: {e}"
            ))
        })?;
        if let Some(env) = lock.environments.get_mut(environment) {
            env.queues.remove(queue);
        }
        write_lock(lock_path, lock).map_err(|e| {
            DeployFailure::driver(format!(
                "deleted the orphaned queue `{physical}` but could not record it in {}: {e}",
                lock_path.display()
            ))
        })?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::deploy::config::tests::project;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    pub(crate) fn lock_with_deployed(workers: &[&str]) -> DeployLock {
        let mut lock = DeployLock::default();
        for worker in workers {
            lock.record_deployed("default", worker, Some(Default::default()));
        }
        lock
    }

    /// Record `worker`'s KV namespace, as a real deploy does before it pushes.
    pub(crate) fn with_kv(mut lock: DeployLock, worker: &str) -> DeployLock {
        lock.environments
            .entry("default".into())
            .or_default()
            .kv
            .insert(worker.to_string(), KvNamespace { id: "ns-id".into() });
        lock
    }

    /// Record `worker`'s derived KV instance `binding` (S4b, #1967).
    pub(crate) fn with_kv_instance(
        mut lock: DeployLock,
        worker: &str,
        binding: &str,
    ) -> DeployLock {
        lock.record_kv("default", worker, binding, &format!("{binding}-id"));
        lock
    }

    fn bare_orphan(worker: &str) -> OrphanKv {
        OrphanKv {
            worker: worker.to_string(),
            binding: None,
        }
    }

    fn instance_orphan(worker: &str, binding: &str) -> OrphanKv {
        OrphanKv {
            worker: worker.to_string(),
            binding: Some(binding.to_string()),
        }
    }

    /// Mark `queue` as one this project has already created.
    pub(crate) fn with_queue(mut lock: DeployLock, queue: &str) -> DeployLock {
        lock.record_queue("default", queue);
        lock
    }

    fn scratch_lock_path(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "bynk-{label}-{}-{unique}.deploy.lock",
            std::process::id()
        ))
    }

    fn scratch_lock_dir(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("bynk-{label}-{}-{unique}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_litter(dir: &Path) -> Vec<std::path::PathBuf> {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "tmp"))
            .collect()
    }

    /// P3 (#2342): ids are unique across every environment of the ledger, the
    /// smallest free one first, and a forgotten id is free again.
    #[test]
    fn rate_limiter_ids_are_allocated_across_environments() {
        let mut lock = DeployLock::default();
        assert_eq!(
            lock.allocate_ratelimit("default", "api", "RATELIMIT_A"),
            ("1000".into(), true)
        );
        assert_eq!(
            lock.allocate_ratelimit("staging", "api", "RATELIMIT_A"),
            ("1001".into(), true)
        );
        assert_eq!(
            lock.allocate_ratelimit("default", "web", "RATELIMIT_A"),
            ("1002".into(), true)
        );
        assert_eq!(
            lock.allocate_ratelimit("default", "api", "RATELIMIT_A"),
            ("1000".into(), false)
        );
        assert!(lock.forget_undeclared_ratelimits("default", "api", &[]));
        assert!(!lock.forget_undeclared_ratelimits("default", "api", &[]));
        assert!(
            !lock.environments["default"]
                .ratelimit_instances
                .contains_key("api")
        );
        assert_eq!(
            lock.allocate_ratelimit("default", "api", "RATELIMIT_B"),
            ("1000".into(), true)
        );
    }

    #[test]
    fn lock_round_trip_is_environment_keyed() {
        let lock = DeployLock {
            version: 1,
            environments: BTreeMap::from([(
                "default".into(),
                Environment {
                    deleted_classes: BTreeMap::new(),
                    renamed_classes: BTreeMap::new(),
                    kv: BTreeMap::from([("api".into(), KvNamespace { id: "abc".into() })]),
                    kv_instances: BTreeMap::from([(
                        "api".into(),
                        BTreeMap::from([("KV_SESSIONS".into(), KvNamespace { id: "s".into() })]),
                    )]),
                    ratelimit_instances: BTreeMap::from([(
                        "api".into(),
                        BTreeMap::from([(
                            "RATELIMIT_LOGIN".into(),
                            RateLimitNamespace {
                                namespace_id: "1000".into(),
                            },
                        )]),
                    )]),
                    r2_instances: BTreeMap::from([(
                        "api".into(),
                        BTreeMap::from([(
                            "R2_ARCHIVES".into(),
                            R2Bucket {
                                name: "api-archives".into(),
                                jurisdiction: Some("eu".into()),
                                superseded: vec![R2Past {
                                    name: "api-archives".into(),
                                    jurisdiction: None,
                                }],
                            },
                        )]),
                    )]),
                    workers: BTreeMap::from([(
                        "api".into(),
                        WorkerRecord {
                            deployed: true,
                            contracts: Default::default(),
                            expects: Some(BTreeMap::from([(
                                "shop.payment".into(),
                                BTreeMap::from([("authorise".into(), "febd15c2757b72e1".into())]),
                            )])),
                            assets: BTreeSet::new(),
                            foreign: BTreeMap::from([(
                                "Legacy".into(),
                                BTreeMap::from([("ping".into(), "4082ce9e2b74e919".into())]),
                            )]),
                        },
                    )]),
                    queues: BTreeSet::from(["intake".to_string()]),
                },
            )]),
        };
        assert_eq!(
            toml::from_str::<DeployLock>(&toml::to_string_pretty(&lock).unwrap()).unwrap(),
            lock
        );
    }

    #[test]
    fn a_slice_0_ledger_without_workers_or_queues_still_reads() {
        // Both tables are additive: a ledger committed before slice 2 (workers)
        // or slice 1 (queues) must keep working, reporting nothing recorded
        // rather than failing to parse. #600 D4: the version stays 1, so this
        // is the whole migration story.
        let lock: DeployLock = toml::from_str(
            r#"
            version = 1
            [environments.default.kv.api]
            id = "abc"
        "#,
        )
        .expect("a slice-0 ledger must still parse");
        assert_eq!(recorded_kv(&lock, "api", "default"), Some("abc"));
        assert!(!lock.is_deployed("default", "api"));
        assert!(!lock.has_queue("default", "intake"));
    }

    #[test]
    fn the_queue_set_serialises_as_names_under_the_environment() {
        // The committed shape is a documented surface — a reviewer reads this
        // file in a diff. Queues are environment-wide names, not a per-worker
        // table, and carry no id.
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_queue("default", "job-intake");
        lock.record_queue("default", "job-retry");
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(
            text.contains("[environments.default]") && text.contains("queues = ["),
            "the queue set is environment-wide, not a per-worker table: {text}"
        );
        for queue in ["job-intake", "job-retry"] {
            assert!(
                text.contains(&format!("\"{queue}\"")),
                "{queue} is recorded"
            );
        }
        assert!(
            !text.contains("id"),
            "a queue is addressed by name — the ledger has no id to record: {text}"
        );
        assert_eq!(toml::from_str::<DeployLock>(&text).unwrap(), lock);
    }

    /// #1883, slice 1b: a Worker's served asset paths round-trip, and a
    /// Worker with none writes no `assets` line.
    #[test]
    fn a_workers_served_assets_round_trip_and_none_is_not_written() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_deployed("default", "board", Some(Default::default()));
        lock.record_assets(
            "default",
            "board",
            ["/_assets/board.1a2b.css".to_string()]
                .into_iter()
                .collect(),
        );
        lock.record_deployed("default", "api", Some(Default::default()));
        lock.record_assets("default", "api", BTreeSet::new());
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("\"/_assets/board.1a2b.css\""), "{text}");
        assert_eq!(
            text.matches("assets = ").count(),
            1,
            "only `board` has assets: {text}"
        );
        assert_eq!(toml::from_str::<DeployLock>(&text).unwrap(), lock);
    }

    /// I4 (#1970): a Worker's foreign callers round-trip, a later push
    /// replaces them, and a Worker with none writes no `foreign` table.
    #[test]
    fn a_workers_foreign_callers_round_trip_and_none_is_not_written() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        let legacy = BTreeMap::from([(
            "Legacy".to_string(),
            BTreeMap::from([("ping".to_string(), "4082ce9e2b74e919".to_string())]),
        )]);
        lock.record_deployed("default", "shop-orders", Some(Default::default()));
        lock.record_foreign("default", "shop-orders", legacy.clone());
        lock.record_deployed("default", "shop-front", Some(Default::default()));
        lock.record_foreign("default", "shop-front", BTreeMap::new());
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("4082ce9e2b74e919"), "{text}");
        assert_eq!(
            text.matches("foreign").count(),
            1,
            "only `shop-orders`: {text}"
        );
        let read = toml::from_str::<DeployLock>(&text).unwrap();
        assert_eq!(read, lock);
        assert_eq!(read.live_foreign("default", "shop-orders"), Some(&legacy));
        // A later push records what that build declares, not a union.
        lock.record_deployed("default", "shop-orders", Some(Default::default()));
        lock.record_foreign("default", "shop-orders", BTreeMap::new());
        assert_eq!(
            lock.live_foreign("default", "shop-orders"),
            Some(&BTreeMap::new())
        );
        // A Worker never recorded is left alone.
        lock.record_foreign("default", "never", legacy);
        assert_eq!(lock.live_foreign("default", "never"), None);
    }

    /// #2077: what a Worker expects round-trips — a dotted dependency key, and
    /// "known to expect nothing" (`Some({})`), which must not read back as "no
    /// record" or every Worker that calls nothing would look unrecorded.
    #[test]
    fn a_workers_expects_round_trip_and_empty_is_not_no_record() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        let orders = BTreeMap::from([(
            "shop.payment".to_string(),
            BTreeMap::from([("authorise".to_string(), "febd15c2757b72e1".to_string())]),
        )]);
        lock.record_deployed("default", "shop-orders", Some(Default::default()));
        lock.record_expects("default", "shop-orders", orders.clone());
        lock.record_deployed("default", "shop-payment", Some(Default::default()));
        lock.record_expects("default", "shop-payment", BTreeMap::new());
        let text = toml::to_string_pretty(&lock).unwrap();
        let read = toml::from_str::<DeployLock>(&text).unwrap();
        assert_eq!(read, lock, "{text}");
        let live: BTreeMap<_, _> = read.live_expects("default").collect();
        assert_eq!(live["shop-orders"], Some(&orders));
        assert_eq!(live["shop-payment"], Some(&BTreeMap::new()), "{text}");
        // A Worker never recorded is left alone.
        lock.record_expects("default", "never", orders);
        assert!(lock.live_expects("default").all(|(w, _)| w != "never"));
    }

    /// #2077: a lock written before `expects` was recorded still loads; its
    /// Workers read as deployed with no record, not as expecting nothing.
    #[test]
    fn a_lock_without_expects_still_reads_as_no_record() {
        let lock: DeployLock = toml::from_str(
            r#"
            version = 1
            [environments.default.workers.shop-orders]
            deployed = true
            [environments.default.workers.shop-payment]
            deployed = true
            [environments.default.workers.shop-payment.contracts]
            authorise = "febd15c2757b72e1"
        "#,
        )
        .expect("a lock without `expects` must still parse");
        assert!(lock.is_deployed("default", "shop-orders"));
        let live: BTreeMap<_, _> = lock.live_expects("default").collect();
        assert_eq!(live["shop-orders"], None);
        assert_eq!(live["shop-payment"], None);
        // And a record that has no `expects` writes none back.
        assert!(!toml::to_string_pretty(&lock).unwrap().contains("expects"));
    }

    #[test]
    fn an_empty_queue_set_is_not_written_at_all() {
        // A project with no queues must not grow an empty `queues = []` line in
        // a committed file for a slice it does not use.
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_deployed("default", "api", Some(Default::default()));
        assert!(!toml::to_string_pretty(&lock).unwrap().contains("queues"));
    }

    #[test]
    fn materialises_only_the_placeholder() {
        // P7.4 (#1305): the real shape `emit_wrangler_toml` produces — a
        // `[[kv_namespaces]]` stanza, not a bare root `id` key (the old
        // fixture's shortcut, which only worked because the old
        // implementation was a whole-file substring replace that didn't
        // care where the placeholder sat).
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("bynk-deploy-{}-{}", std::process::id(), unique));
        std::fs::write(
            &path,
            format!(
                "[[kv_namespaces]]\nbinding = \"KV\"\nid = \"{KV_NAMESPACE_ID_PLACEHOLDER}\"\n"
            ),
        )
        .unwrap();
        assert!(materialise_kv_ids(
            &path,
            &BTreeMap::from([("KV".to_string(), "abc".to_string())])
        ));
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed: toml::Table = text.parse().unwrap();
        assert_eq!(
            parsed["kv_namespaces"].as_array().unwrap()[0]["id"].as_str(),
            Some("abc")
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn two_environments_do_not_cross_contaminate_the_ledger() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_deployed("staging", "api", None);
        lock.record_queue("staging", "jobs");
        lock.environments
            .entry("staging".into())
            .or_default()
            .kv
            .insert(
                "api".into(),
                KvNamespace {
                    id: "kv-staging".into(),
                },
            );

        // "default" was never touched — a `--env staging` run must not
        // fabricate or leak into the section a plain `bynk deploy` reads.
        assert!(!lock.is_deployed("default", "api"));
        assert!(!lock.has_queue("default", "jobs"));
        assert_eq!(recorded_kv(&lock, "api", "default"), None);

        assert!(lock.is_deployed("staging", "api"));
        assert!(lock.has_queue("staging", "jobs"));
        assert_eq!(recorded_kv(&lock, "api", "staging"), Some("kv-staging"));
    }

    /// #837 review: `materialise_deploy_state` (shared with `bynk dev --
    /// --remote`) hardcoded `"default"` even after `--env` shipped. Before
    /// `--env` existed every real deploy recorded into `"default"`
    /// regardless, so that always matched — but a project deployed *only*
    /// under `bynk deploy --env staging` now has nothing under `"default"`,
    /// and reading the wrong section misreports a provisioned project as
    /// never deployed.
    #[test]
    fn materialise_deploy_state_reads_the_named_environment_not_default() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "bynk-materialise-state-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let project_root = dir.clone();
        let config = dir.join("wrangler.toml");
        std::fs::write(
            &config,
            format!(
                "[[kv_namespaces]]\nbinding = \"KV\"\nid = \"{KV_NAMESPACE_ID_PLACEHOLDER}\"\n"
            ),
        )
        .unwrap();

        // Provisioned under "staging" alone — the scenario the review named:
        // a project that has never had a plain `bynk deploy` (no "default").
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.environments
            .entry("staging".into())
            .or_default()
            .kv
            .insert(
                "api".into(),
                KvNamespace {
                    id: "kv-staging".into(),
                },
            );
        std::fs::write(
            project_root.join(LOCK_FILE),
            toml::to_string_pretty(&lock).unwrap(),
        )
        .unwrap();

        // Reading "default" (what this function did unconditionally before
        // the fix) must fail helpfully, not silently mis-materialise.
        let err = materialise_deploy_state(&project_root, "api", &config, "default")
            .expect_err("nothing is recorded under \"default\" — this must not silently pass");
        assert!(
            err.contains("environment `default`"),
            "the error should name which environment it looked under: {err}"
        );

        // Reading "staging" — the environment it was actually deployed under
        // — must succeed and materialise that environment's id.
        assert!(materialise_deploy_state(&project_root, "api", &config, "staging").unwrap());
        let text = std::fs::read_to_string(&config).unwrap();
        let parsed: toml::Table = text.parse().unwrap();
        assert_eq!(
            parsed["kv_namespaces"].as_array().unwrap()[0]["id"].as_str(),
            Some("kv-staging")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// S4b (#1967): `bynk dev --remote` binds every namespace, including on a
    /// Worker with no bare `KV`, whose first stanza is an instance's. The old
    /// first-stanza gate returned early there and materialised nothing.
    #[test]
    fn materialise_deploy_state_binds_each_rate_limiter() {
        // P3 (#2342): `bynk dev --remote` binds the ids a deploy allocated.
        let dir = scratch_lock_dir("materialise-ratelimits");
        let config = dir.join("wrangler.toml");
        std::fs::write(
            &config,
            "[[ratelimits]]\nname = \"RATELIMIT_LOGIN\"\nnamespace_id = \"<RATELIMIT_LOGIN_NAMESPACE>\" # set at deploy time\nsimple = { limit = 20, period = 60 }\n",
        )
        .unwrap();
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        std::fs::write(dir.join(LOCK_FILE), toml::to_string_pretty(&lock).unwrap()).unwrap();
        let err = materialise_deploy_state(&dir, "shop", &config, "default")
            .expect_err("an unallocated limiter must not pass");
        assert!(
            err.contains("`RATELIMIT_LOGIN`") && err.contains("bynk deploy"),
            "{err}"
        );
        lock.allocate_ratelimit("default", "shop", "RATELIMIT_LOGIN");
        std::fs::write(dir.join(LOCK_FILE), toml::to_string_pretty(&lock).unwrap()).unwrap();
        assert!(materialise_deploy_state(&dir, "shop", &config, "default").unwrap());
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(
            text.contains(
                "namespace_id = \"1000\" # set at deploy time\nsimple = { limit = 20, period = 60 }"
            ),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn materialise_deploy_state_binds_every_instance() {
        let dir = scratch_lock_dir("materialise-instances");
        let config = dir.join("wrangler.toml");
        std::fs::write(
            &config,
            "[[kv_namespaces]]\nbinding = \"KV_PRICE_CACHE\"\nid = \"<KV_PRICE_CACHE_ID>\"\n\n[[kv_namespaces]]\nbinding = \"KV_SESSIONS\"\nid = \"<KV_SESSIONS_ID>\"\n",
        )
        .unwrap();
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_kv("default", "shop-cart", "KV_SESSIONS", "sess");
        std::fs::write(dir.join(LOCK_FILE), toml::to_string_pretty(&lock).unwrap()).unwrap();

        // One instance unrecorded: refused, naming it, and nothing written.
        let err = materialise_deploy_state(&dir, "shop-cart", &config, "default")
            .expect_err("an unprovisioned instance must not pass");
        assert!(
            err.contains("`KV_PRICE_CACHE`") && err.contains("`shop-cart`"),
            "{err}"
        );
        assert!(
            std::fs::read_to_string(&config)
                .unwrap()
                .contains("<KV_SESSIONS_ID>")
        );

        lock.record_kv("default", "shop-cart", "KV_PRICE_CACHE", "price");
        std::fs::write(dir.join(LOCK_FILE), toml::to_string_pretty(&lock).unwrap()).unwrap();
        assert!(materialise_deploy_state(&dir, "shop-cart", &config, "default").unwrap());
        let parsed: toml::Table = std::fs::read_to_string(&config).unwrap().parse().unwrap();
        let ids: Vec<&str> = parsed["kv_namespaces"]
            .as_array()
            .unwrap()
            .iter()
            .map(|ns| ns["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, ["price", "sess"]);
        let _ = std::fs::remove_dir_all(dir);
    }

    /// S4b (#1967): a project with only the bare `Kv` writes the ledger it
    /// wrote before instances existed, byte for byte: no `kv_instances` table.
    #[test]
    fn a_bare_kv_ledger_is_written_as_it_was_before_instances() {
        let pre_s4b = "version = 1\n\n[environments.default.kv.api]\nid = \"abc\"\n";
        let lock: DeployLock = toml::from_str(pre_s4b).expect("a pre-S4b ledger reads");
        assert_eq!(
            recorded_kv_binding(&lock, "api", "KV", "default"),
            Some("abc")
        );
        assert_eq!(toml::to_string_pretty(&lock).unwrap(), pre_s4b);
    }

    /// S4b (#1967): instances live beside the bare namespace, keyed by Worker
    /// then binding, and forgetting the last one leaves the file as it was.
    #[test]
    fn instances_are_recorded_by_worker_and_binding_and_forgotten_cleanly() {
        let mut lock = with_kv(
            DeployLock {
                version: 1,
                ..Default::default()
            },
            "shop-cart",
        );
        let before = toml::to_string_pretty(&lock).unwrap();
        lock.record_kv("default", "shop-cart", "KV_SESSIONS", "sess");
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(
            text.contains("[environments.default.kv_instances.shop-cart.KV_SESSIONS]"),
            "{text}"
        );
        assert_eq!(
            recorded_kv_binding(&lock, "shop-cart", "KV_SESSIONS", "default"),
            Some("sess")
        );
        assert_eq!(
            recorded_kv_binding(&lock, "shop-cart", "KV", "default"),
            Some("ns-id")
        );
        assert_eq!(
            recorded_kv_binding(&lock, "shop-cart", "KV_OTHER", "default"),
            None
        );
        assert_eq!(
            toml::from_str::<DeployLock>(&text).unwrap(),
            lock,
            "the instance table round-trips"
        );
        lock.forget_kv("default", "shop-cart", "KV_SESSIONS");
        assert_eq!(toml::to_string_pretty(&lock).unwrap(), before);
    }

    /// Review of #1974: a still-live Worker that stops using the bare `Kv`
    /// (say, for `capability Sessions = Kv`) no longer declares `KV`, so its
    /// recorded bare namespace is an orphan, like an instance it dropped.
    #[test]
    fn a_bare_namespace_its_live_worker_no_longer_declares_is_an_orphan() {
        let lock = with_kv(DeployLock::default(), "api");
        let switched = project(vec![(
            "api",
            Resources::default().kv_instances(&["KV_SESSIONS"]),
        )]);
        assert_eq!(
            find_orphans(&lock, "default", &switched).kv,
            [bare_orphan("api")]
        );
        let still_bare = project(vec![("api", Resources::default().needs_kv())]);
        assert!(find_orphans(&lock, "default", &still_bare).kv.is_empty());
    }

    /// S4b (#1967): an instance its still-live Worker no longer declares (a
    /// capability removed or renamed) is an orphan on its own; a removed
    /// Worker's instances go with it; a declared one is not an orphan.
    #[test]
    fn a_removed_or_renamed_instance_is_an_orphan() {
        let lock = with_kv_instance(
            with_kv_instance(
                with_kv_instance(with_kv(DeployLock::default(), "gone"), "gone", "KV_OLD"),
                "shop-cart",
                "KV_SESSIONS",
            ),
            "shop-cart",
            "KV_CARTS",
        );
        // `capability Carts = Kv` renamed to `Baskets`: `KV_CARTS` is gone
        // from the build, `KV_BASKETS` is new (a create, in the plan).
        let resources = project(vec![(
            "shop-cart",
            Resources::default().kv_instances(&["KV_BASKETS", "KV_SESSIONS"]),
        )]);
        let orphans = find_orphans(&lock, "default", &resources);
        assert_eq!(
            orphans.kv,
            [
                bare_orphan("gone"),
                instance_orphan("gone", "KV_OLD"),
                instance_orphan("shop-cart", "KV_CARTS"),
            ]
        );
        assert!(orphans.has_prunable());
    }

    /// P7.4 (#1305): a project with no KV binding at all must skip the lock
    /// lookup entirely and return `Ok(false)` — not error just because
    /// nothing is recorded for it (there's nothing to materialise). No lock
    /// file exists in this project root at all, proving the early exit
    /// really does short-circuit before `read_lock` runs.
    #[test]
    fn materialise_deploy_state_is_a_no_op_without_a_kv_binding() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "bynk-materialise-state-no-kv-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let config = dir.join("wrangler.toml");
        std::fs::write(&config, "name = \"api\"\nmain = \"index.ts\"\n").unwrap();

        assert_eq!(
            materialise_deploy_state(&dir, "api", &config, "default"),
            Ok(false)
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    // ---- #839 slice 5: reconciliation maturity + orphan reporting -------

    #[test]
    fn a_removed_context_with_kv_is_two_orphans_not_one() {
        // `kv` and `workers` are both keyed by worker name — a context that
        // had KV and was deleted from source shows up in both maps, so the
        // diff must report both, independently (DECISION A).
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.environments
            .entry("default".into())
            .or_default()
            .kv
            .insert(
                "gone".into(),
                KvNamespace {
                    id: "kv-gone".into(),
                },
            );
        lock.record_deployed("default", "gone", None);
        // "still-here" is in the current build, "gone" is not.
        let resources = BTreeMap::from([("still-here".into(), Resources::default())]);

        let orphans = find_orphans(&lock, "default", &resources);
        assert_eq!(orphans.kv, [bare_orphan("gone")]);
        assert_eq!(orphans.workers, names(&["gone"]));
        assert!(orphans.queues.is_empty());
    }

    // ---- P5 (#2323): R2 buckets ----

    fn r2_lock() -> DeployLock {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_r2("default", "shop", "R2_ARCHIVES", "shop-archives", None);
        lock.record_r2("default", "shop", "R2_OLD", "shop-old", Some("eu"));
        lock
    }

    #[test]
    fn a_removed_or_renamed_bucket_is_a_reported_orphan_that_prune_leaves_alone() {
        let resources = BTreeMap::from([(
            "shop".to_string(),
            Resources::default().r2_buckets(&[("R2_ARCHIVES", "shop-archives", None)]),
        )]);
        let orphans = find_orphans(&r2_lock(), "default", &resources);
        assert_eq!(
            orphans.r2,
            [OrphanR2 {
                worker: "shop".into(),
                binding: "R2_OLD".into(),
                bucket: "shop-old".into(),
                jurisdiction: Some("eu".into())
            }]
        );
        // A bucket holds data: `--prune` neither offers nor attempts it.
        assert!(!orphans.has_prunable());
    }

    #[test]
    fn a_removed_worker_orphans_every_bucket_it_held() {
        let orphans = find_orphans(&r2_lock(), "default", &BTreeMap::new());
        assert_eq!(orphans.r2.len(), 2);
        assert!(
            find_orphans(&r2_lock(), "staging", &BTreeMap::new())
                .r2
                .is_empty()
        );
    }

    #[test]
    fn a_bucket_is_recorded_under_the_environment_and_survives_the_file() {
        let lock = r2_lock();
        assert_eq!(
            recorded_r2(&lock, "shop", "R2_OLD", "default"),
            Some(&R2Bucket {
                name: "shop-old".into(),
                jurisdiction: Some("eu".into()),
                superseded: Vec::new()
            })
        );
        assert!(recorded_r2(&lock, "shop", "R2_OLD", "staging").is_none());
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("[environments.default.r2_instances.shop.R2_OLD]"));
        let back: DeployLock = toml::from_str(&text).unwrap();
        assert_eq!(back, lock);
        // No R2, no table: a project without it writes the file it always did.
        assert!(
            !toml::to_string_pretty(&with_kv(DeployLock::default(), "api"))
                .unwrap()
                .contains("r2_instances")
        );
    }

    /// A scratch project holding `config` as `workers/shop/wrangler.toml` and
    /// the ledger `lock`; returns the root and the config's path.
    fn r2_project(
        tag: &str,
        config: &str,
        lock: &DeployLock,
    ) -> (std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("bynk-r2-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("workers/shop")).unwrap();
        let path = root.join("workers/shop/wrangler.toml");
        std::fs::write(&path, config).unwrap();
        write_lock(&root.join(LOCK_FILE), lock).unwrap();
        (root, path)
    }

    const R2_CONFIG: &str = "name = \"shop\"\n\n[[r2_buckets]]\nbinding = \"R2_ARCHIVES\"\nbucket_name = \"shop-archives\"\n";

    #[test]
    fn dev_remote_binds_a_recorded_bucket_under_the_environments_name() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_r2(
            "staging",
            "shop",
            "R2_ARCHIVES",
            "shop-archives-staging",
            None,
        );
        let (root, config) = r2_project("staging", R2_CONFIG, &lock);
        assert_eq!(
            materialise_deploy_state(&root, "shop", &config, "staging"),
            Ok(true)
        );
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(
            text.contains("bucket_name = \"shop-archives-staging\""),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dev_remote_leaves_the_default_environments_bucket_as_generated() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_r2("default", "shop", "R2_ARCHIVES", "shop-archives", None);
        let (root, config) = r2_project("default", R2_CONFIG, &lock);
        assert_eq!(
            materialise_deploy_state(&root, "shop", &config, "default"),
            Ok(false)
        );
        assert_eq!(std::fs::read_to_string(&config).unwrap(), R2_CONFIG);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_replaced_bucket_stays_recorded_and_is_reported_while_its_binding_is_live() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_r2("default", "shop", "R2_ARCHIVES", "shop-archives", None);
        // Recording the same bucket again changes nothing.
        lock.record_r2("default", "shop", "R2_ARCHIVES", "shop-archives", None);
        assert!(
            recorded_r2(&lock, "shop", "R2_ARCHIVES", "default")
                .unwrap()
                .superseded
                .is_empty()
        );
        lock.record_r2(
            "default",
            "shop",
            "R2_ARCHIVES",
            "shop-archives",
            Some("eu"),
        );
        let resources = BTreeMap::from([(
            "shop".to_string(),
            Resources::default().r2_buckets(&[("R2_ARCHIVES", "shop-archives", Some("eu"))]),
        )]);
        assert_eq!(
            find_orphans(&lock, "default", &resources).r2,
            [OrphanR2 {
                worker: "shop".into(),
                binding: "R2_ARCHIVES".into(),
                bucket: "shop-archives".into(),
                jurisdiction: None
            }]
        );
        let text = toml::to_string_pretty(&lock).unwrap();
        assert_eq!(toml::from_str::<DeployLock>(&text).unwrap(), lock);
    }

    #[test]
    fn dev_remote_refuses_a_bucket_recorded_under_another_jurisdiction() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_r2(
            "default",
            "shop",
            "R2_ARCHIVES",
            "shop-archives",
            Some("eu"),
        );
        let (root, config) = r2_project("jurisdiction", R2_CONFIG, &lock);
        let error = materialise_deploy_state(&root, "shop", &config, "default").unwrap_err();
        assert!(error.contains("another `@jurisdiction`"), "{error}");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dev_remote_refuses_a_bucket_no_deploy_has_made() {
        let (root, config) = r2_project(
            "unprovisioned",
            R2_CONFIG,
            &DeployLock {
                version: 1,
                ..Default::default()
            },
        );
        let error = materialise_deploy_state(&root, "shop", &config, "default").unwrap_err();
        assert!(error.contains("R2 bucket `R2_ARCHIVES`"), "{error}");
        assert!(
            error.contains("run `bynk deploy --env default` first"),
            "{error}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_queue_two_contexts_share_is_never_orphaned_while_either_declares_it() {
        // The false-positive risk DECISION A's diff exists to avoid: a queue
        // consumed by two contexts must not be reported orphaned just because
        // a *third*, now-removed context used to consume it too.
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_queue("default", "jobs");
        let resources = BTreeMap::from([
            ("orders".into(), Resources::default().consumes(&["jobs"])),
            ("billing".into(), Resources::default().consumes(&["jobs"])),
        ]);

        let orphans = find_orphans(&lock, "default", &resources);
        assert!(
            orphans.queues.is_empty(),
            "jobs is still consumed by two live contexts: {orphans:?}"
        );
    }

    #[test]
    fn a_queue_no_context_declares_anymore_is_orphaned() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_queue("default", "stale-jobs");
        let resources = BTreeMap::from([("orders".into(), Resources::default())]);

        let orphans = find_orphans(&lock, "default", &resources);
        assert_eq!(orphans.queues, names(&["stale-jobs"]));
    }

    #[test]
    fn find_orphans_is_scoped_to_the_named_environment() {
        // A "staging" orphan must never leak into "default"'s report, and
        // vice versa — environments (slice 4) stay fully independent.
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.environments
            .entry("staging".into())
            .or_default()
            .kv
            .insert(
                "gone".into(),
                KvNamespace {
                    id: "kv-staging-gone".into(),
                },
            );
        let resources = BTreeMap::new();

        assert!(
            find_orphans(&lock, "default", &resources).kv.is_empty(),
            "default"
        );
        assert_eq!(
            find_orphans(&lock, "staging", &resources).kv,
            [bare_orphan("gone")]
        );
    }

    #[test]
    fn an_absent_environment_has_no_orphans() {
        let lock = DeployLock::default();
        let orphans = find_orphans(&lock, "default", &BTreeMap::new());
        assert_eq!(orphans, Orphans::default());
    }

    #[test]
    fn has_prunable_ignores_worker_only_orphans() {
        // The bug the review caught: a project whose only orphan is an
        // unprunable Worker must not trigger confirm_prune's "Delete 0
        // resource(s)?" prompt at all.
        let worker_only = Orphans {
            workers: names(&["gone"]),
            ..Default::default()
        };
        assert!(!worker_only.has_prunable());

        let with_kv = Orphans {
            kv: vec![bare_orphan("gone")],
            ..Default::default()
        };
        assert!(with_kv.has_prunable());

        let with_queue = Orphans {
            queues: names(&["gone"]),
            ..Default::default()
        };
        assert!(with_queue.has_prunable());

        assert!(!Orphans::default().has_prunable());
    }

    #[test]
    fn a_written_ledger_reads_back_identically() {
        // The floor the atomic write must not disturb: a real ledger survives a
        // write/read round-trip unchanged.
        let path = scratch_lock_path("roundtrip");
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.environments
            .entry("default".into())
            .or_default()
            .kv
            .insert(
                "api".into(),
                KvNamespace {
                    id: "kv-123".into(),
                },
            );
        write_lock(&path, &lock).unwrap();
        assert_eq!(read_lock(&path).unwrap(), lock);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn an_empty_ledger_is_corruption_not_a_fresh_project() {
        // #736: a truncated write leaves a zero-byte file. Reading it as an
        // empty v1 ledger would re-mint every namespace, so it must fail hard —
        // whereas a genuinely absent file is a fresh project and reads clean.
        let path = scratch_lock_path("empty");
        std::fs::write(&path, "").unwrap();
        assert!(
            read_lock(&path).is_err(),
            "a zero-byte ledger must be rejected, not treated as no environments"
        );
        std::fs::write(&path, "   \n\t\n").unwrap();
        assert!(
            read_lock(&path).is_err(),
            "a whitespace-only ledger is just as corrupt"
        );
        let _ = std::fs::remove_file(&path);
        assert!(
            read_lock(&path).is_ok(),
            "an absent ledger is a fresh project, not corruption"
        );
    }

    #[test]
    fn a_ledger_without_a_version_is_rejected() {
        // With the serde default gone, a file that parses but carries no
        // `version` is corruption rather than a silent empty v1 ledger.
        let path = scratch_lock_path("noversion");
        std::fs::write(&path, "[environments]\n").unwrap();
        assert!(read_lock(&path).is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_successful_write_leaves_no_temp_litter() {
        // The atomic write renames its temp over the ledger; nothing sibling to
        // the ledger may survive the write.
        let dir = scratch_lock_dir("nolitter");
        let path = dir.join(LOCK_FILE);
        let lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        write_lock(&path, &lock).unwrap();
        write_lock(&path, &lock).unwrap(); // over an existing ledger, too
        assert!(
            temp_litter(&dir).is_empty(),
            "no `.tmp` sibling may outlive the rename"
        );
        assert_eq!(read_lock(&path).unwrap(), lock);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[cfg(unix)]
    #[test]
    fn a_rewrite_preserves_the_ledger_permissions() {
        use std::os::unix::fs::PermissionsExt;
        // A committed ledger's mode must survive a rewrite, or the atomic replace
        // would silently loosen or tighten it via the temp file's fresh mode.
        let dir = scratch_lock_dir("perms");
        let path = dir.join(LOCK_FILE);
        let lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        write_lock(&path, &lock).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        write_lock(&path, &lock).unwrap();
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600,
            "the rewrite must carry the ledger's own mode across the rename"
        );
        let _ = std::fs::remove_dir_all(dir);
    }
}
