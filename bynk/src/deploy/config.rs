use super::*;

// ---------------------------------------------------------------------------
// What one context declares: the resources to provision and the bindings to
// order against (slices 1 and 2)
// ---------------------------------------------------------------------------

/// The stanzas of a generated `wrangler.toml` that `deploy` acts on.
///
/// Everything here is read from the **emitted config** rather than from the
/// checker's project model because this is precisely the file wrangler is about
/// to send and Cloudflare is about to validate — so the plan and the upload can
/// never describe different projects (ADR 0193 D3, extended to slice 1's kinds).
#[derive(Debug, Default, Deserialize)]
struct WranglerConfig {
    #[serde(default)]
    services: Vec<ServiceBinding>,
    #[serde(default)]
    queues: QueueBindings,
    /// #1796: Cloudflare's declarative `exports` map, keyed by class name —
    /// what replaced the `[[migrations]]` this struct used to read.
    #[serde(default)]
    exports: BTreeMap<String, Export>,
    // P7.4 (#1305): read structurally alongside everything else this struct
    // already deserialises, replacing `needs_kv`'s own former
    // `text.contains(KV_NAMESPACE_ID_PLACEHOLDER)` whole-file substring
    // search (R8.20 — deploy-time placeholders are typed, not textual).
    #[serde(default)]
    kv_namespaces: Vec<KvNamespaceBinding>,
    /// P5 (#2323): the `[[r2_buckets]]` of the `R2` instances.
    #[serde(default)]
    r2_buckets: Vec<R2BucketBinding>,
    /// P3 (#2342): the `[[ratelimits]]` of the `RateLimit` instances.
    #[serde(default)]
    ratelimits: Vec<RateLimitBinding>,
}

#[derive(Debug, Deserialize)]
struct RateLimitBinding {
    /// `RATELIMIT_<SNAKE>`: the instance's binding, keyed `name`, not
    /// `binding`. The `namespace_id` itself is only read and written
    /// structurally, by `materialise_ratelimit_namespace_ids`.
    name: String,
}

#[derive(Debug, Deserialize)]
struct ServiceBinding {
    /// The binding's `env` name: `SHOP_CART` for a project context,
    /// `SERVICE_BILLING` for a foreign Worker (interop I5, #1972).
    binding: String,
    /// The *target* worker's name (`worker_dir_name`, dots dasherised), or a
    /// foreign Worker's as its declaration names it.
    service: String,
}

/// Interop I5 (#1972): a Service Binding to a Worker that is not one of the
/// project's (`capability Billing = Service @worker("legacy-billing")`).
/// Deploy reads only generated output (ADR 0193 D3), so a `[[services]]`
/// entry is foreign exactly when its target is none of the build's Workers:
/// every context a project consumes is built beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ForeignService {
    pub(crate) binding: String,
    pub(crate) worker: String,
}

#[derive(Debug, Deserialize)]
struct KvNamespaceBinding {
    /// `KV` for the bare `Kv`, `KV_<SNAKE>` for a derived instance (S4b,
    /// #1967). The `id` itself is only ever read and written structurally, by
    /// `bynk_emit::emitter::wrangler::materialise_kv_namespace_ids`.
    binding: String,
}

#[derive(Debug, Default, Deserialize)]
struct QueueBindings {
    #[serde(default)]
    consumers: Vec<QueueConsumer>,
}

#[derive(Debug, Deserialize)]
struct QueueConsumer {
    /// The queue's user-given name, straight from `from queue("n")`.
    queue: String,
}

/// One `[exports.<Class>]` entry. Only the fields that say whether it is a
/// live Durable Object, and on which backend, are read: the map can also hold
/// `type = "worker"` entrypoints, and tombstones (`state = "deleted"` and the
/// like) that the emitter never writes but a reader must not mistake for a
/// class the push registers.
#[derive(Debug, Deserialize)]
struct Export {
    #[serde(rename = "type")]
    kind: String,
    /// Absent means `"created"` — Cloudflare's default, a live class.
    state: Option<String>,
    storage: Option<String>,
    /// State-migrations S3 (#2248): a `renamed` tombstone's new class.
    renamed_to: Option<String>,
}

/// One Durable Object class the push declares, and its storage backend.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct DurableObject {
    pub(crate) class: String,
    pub(crate) storage: String,
}

#[derive(Debug, Deserialize)]
struct R2BucketBinding {
    /// `R2_<SNAKE>`: the instance's binding.
    binding: String,
    /// The unqualified derived name (`<worker>-<capability, kebab>`), which
    /// [`r2_bucket_name`] environment-qualifies.
    bucket_name: String,
    #[serde(default)]
    jurisdiction: Option<String>,
}

/// P5 (#2323): one `R2` instance's bucket, as the emitted config declares it.
/// Unlike a KV namespace it has no minted id: the name is the identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct R2Decl {
    /// `R2_<SNAKE>`, the key the ledger records the bucket under.
    pub(crate) binding: String,
    /// The derived name before any environment qualification.
    pub(crate) name: String,
    /// The `@jurisdiction` the bucket is pinned to at creation, if any.
    pub(crate) jurisdiction: Option<String>,
}

/// The provisioning surface one context's closure locks it to.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Resources {
    /// The workers this one binds to — the edges the deploy order respects.
    /// Adapters are already excluded upstream (they are not Workers), and
    /// foreign Workers are in [`Resources::foreign_services`], so every edge
    /// here names a real worker directory.
    pub(crate) binds_to: Vec<String>,
    /// Interop I5 (#1972): the Service Bindings to Workers outside the
    /// project, in binding order. Never provisioned, ordered against or
    /// environment-qualified: the Worker is not Bynk's.
    pub(crate) foreign_services: Vec<ForeignService>,
    /// The queues this context consumes, by name. Created before the push
    /// (ADR 0194 D3): a `[[queues.consumers]]` binding whose queue does not
    /// exist fails the deploy.
    pub(crate) queues: Vec<String>,
    /// #1796: the Durable Object classes the config declares in `exports`
    /// (each agent, plus the events fan-out class), in class-name order.
    /// **Advisory** — Cloudflare reconciles the declared set against the
    /// Worker's namespaces on every deploy and owns the result, so this says
    /// what the push will declare, never which namespaces already exist (the
    /// principle of ADR 0194 D1).
    pub(crate) durable_objects: Vec<DurableObject>,
    /// State-migrations S3 (#2248): `renamed` tombstones, `(old, new)` — the
    /// push moves each old class's stored data to the new one.
    pub(crate) renamed_durable_objects: Vec<(String, String)>,
    /// State-migrations S3 (#2248): `deleted` tombstones — the push destroys
    /// each class's stored data, so the plan needs it confirmed by name.
    pub(crate) deleted_durable_objects: Vec<String>,
    /// Declares the bare `KV` namespace, the cloudflare adapter's `Kv`.
    pub(crate) needs_kv: bool,
    /// S4b (#1967): the bindings of the derived instances' namespaces
    /// (`capability Sessions = Kv` → `KV_SESSIONS`), in binding order. Each is
    /// its own namespace, provisioned and recorded apart from the bare one.
    pub(crate) kv_instances: Vec<String>,
    /// P5 (#2323): the buckets of the `R2` instances, in binding order. Each is
    /// created by name (the name is the identity), recorded in the ledger, and
    /// reported, never deleted, when its instance goes.
    pub(crate) r2_buckets: Vec<R2Decl>,
    /// P3 (#2342): the bindings of the `RateLimit` instances, in binding
    /// order. Each has a `namespace_id` the ledger allocates; the limit and
    /// the window are configuration the push carries, not identity.
    pub(crate) ratelimits: Vec<String>,
    /// Slice 3: the secret names this context's handlers will read from `env` —
    /// an `actor`'s `auth` secret, read from the emitted `bynk-secrets.json`.
    ///
    /// A **floor, not a census** (ADR 0195 D2): an empty set means "the compiler
    /// proved nothing required", never "this context needs no secret".
    pub(crate) declared_secrets: Vec<String>,
    /// Slice 3 follow-up (ADR 0196): the literal `bynk.Secrets` names this
    /// context reads. **Advisory** — `Secrets.get` returns `Option`, so absence
    /// is a legitimate handled outcome and a missing value warns rather than
    /// failing.
    pub(crate) read_secrets: Vec<String>,
    /// False when the context reads a secret the compiler cannot name (a
    /// computed expression, or an adapter binding — #1976), so `read_secrets`
    /// is not everything. The plan must say so rather than
    /// presenting its list as whole.
    pub(crate) reads_complete: bool,
    /// I3 (#1971): the foreign callers this context's `on call` handlers admit,
    /// from the manifest's `foreign` list. `deploy` mints each one's secret,
    /// sets it here, and prints the hand-off for the foreign Worker.
    pub(crate) foreign_callers: Vec<ForeignCaller>,
}

/// I3 (#1971): one foreign caller a context admits, as the manifest records it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct ForeignCaller {
    /// The `Foreign` actor's name, which the caller sends as `X-Bynk-Caller`.
    pub(crate) actor: String,
    /// The name of the env binding the context verifies its calls under: a
    /// name, never a value. The manifest's `secret` key.
    #[serde(rename = "secret")]
    pub(crate) binding: String,
    /// The foreign Worker's name, when the declaration gives one.
    #[serde(default)]
    pub(crate) worker: Option<String>,
}

impl Default for Resources {
    /// Hand-written for one field: `reads_complete` is **true** by vacuity — a
    /// context that names no secret has none out of sight — whereas `bool`'s
    /// default is false, which would say the opposite. A derived `Default` here
    /// would make every resource-less context claim an incomplete list.
    fn default() -> Self {
        Self {
            binds_to: Vec::new(),
            foreign_services: Vec::new(),
            queues: Vec::new(),
            durable_objects: Vec::new(),
            renamed_durable_objects: Vec::new(),
            deleted_durable_objects: Vec::new(),
            needs_kv: false,
            kv_instances: Vec::new(),
            r2_buckets: Vec::new(),
            ratelimits: Vec::new(),
            declared_secrets: Vec::new(),
            read_secrets: Vec::new(),
            reads_complete: true,
            foreign_callers: Vec::new(),
        }
    }
}

/// The emitted `bynk-secrets.json` (ADR 0195 D5) — the seam that carries the
/// compiler's knowledge to the driver across both compile paths, including the
/// shelled-`bynkc` one where there is no in-memory model to consult.
#[derive(Debug, Deserialize)]
struct SecretsManifest {
    version: u32,
    #[serde(default)]
    declared: Vec<String>,
    /// v0.173 (ADR 0196): literal `bynk.Secrets` names this context reads.
    /// **Advisory** — `Secrets.get` returns `Option`, so absence is a legitimate
    /// handled outcome and a missing value warns rather than failing.
    #[serde(default)]
    read: Vec<String>,
    /// False when the context reads at least one secret the compiler cannot
    /// name (a computed expression, or an adapter binding — #1976), so `read`
    /// is not everything. Defaulted `true` **only**
    /// because the version guard already refuses a manifest that predates the
    /// field — no v1 file reaches this.
    #[serde(default = "read_complete_default")]
    read_complete: bool,
    /// I3 (#1971): additive within version 2 — absent means the context
    /// admits no foreign caller, which is what an older manifest means too.
    #[serde(default)]
    foreign: Vec<ForeignCaller>,
}

fn read_complete_default() -> bool {
    true
}

/// The manifest schema this driver understands.
///
/// **2** (v0.173): `read` + `read_complete`. Refusing v1 rather than reading it
/// with defaults is the point — a v1 manifest carries no evidence about computed
/// names, and assuming `read_complete: true` for it would make the driver claim
/// a completeness nothing established. A stale build tree is re-compiled by the
/// same command that reads it, so refusing costs nothing real.
const SECRETS_MANIFEST_VERSION: u32 = 2;

/// What the emitted manifest says about one context's secrets.
///
/// An absent file is the empty answer, not an error: a context with no secrets
/// of any kind emits none. An *unreadable* or *unparseable* one is an error — it
/// is emitted alongside the config we just read, so a failure there means the
/// build tree is damaged rather than merely old, and guessing would risk
/// skipping a secret the Worker fail-closes without.
fn read_secrets_manifest(worker_dir: &Path) -> Result<SecretsManifest, String> {
    let path = worker_dir.join(bynk_emit::emitter::secrets::SECRETS_MANIFEST);
    if !path.exists() {
        return Ok(SecretsManifest {
            version: SECRETS_MANIFEST_VERSION,
            declared: Vec::new(),
            read: Vec::new(),
            // Vacuously: a context that emits no manifest names no secret at
            // all, computed or otherwise.
            read_complete: true,
            foreign: Vec::new(),
        });
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let manifest: SecretsManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if manifest.version != SECRETS_MANIFEST_VERSION {
        return Err(format!(
            "unsupported {} version {}",
            bynk_emit::emitter::secrets::SECRETS_MANIFEST,
            manifest.version
        ));
    }
    Ok(manifest)
}

/// Read everything `deploy` acts on out of one worker's generated build output.
fn read_resources(config: &Path, project_workers: &[String]) -> Result<Resources, String> {
    let text = std::fs::read_to_string(config).map_err(|e| e.to_string())?;
    let parsed: WranglerConfig = toml::from_str(&text).map_err(|e| e.to_string())?;
    // Beside the config, not in it: secrets are a runtime store rather than
    // configuration, so `wrangler.toml` has no stanza to carry them.
    let secrets = read_secrets_manifest(
        config
            .parent()
            .ok_or_else(|| "configuration has no directory".to_string())?,
    )?;
    let mut queues: Vec<String> = parsed
        .queues
        .consumers
        .into_iter()
        .map(|c| c.queue)
        .collect();
    // The emitter already sorts and dedups, but a queue is created by name and
    // creating one twice is pure noise — so make it this reader's property
    // rather than an emitter detail deploy happens to inherit.
    queues.sort();
    queues.dedup();
    // A `BTreeMap`, so already in class-name order. A live class always has
    // a backend: Wrangler's schema requires `storage` unless `state` is a
    // tombstone. One without is refused, not skipped (review of #1803):
    // skipping it would let the plan say the push declares nothing while the
    // config still declares the class.
    let mut durable_objects = Vec::new();
    let mut renamed_durable_objects = Vec::new();
    let mut deleted_durable_objects = Vec::new();
    for (class, export) in parsed.exports {
        if export.kind == "durable-object" {
            match (export.state.as_deref(), export.renamed_to) {
                (Some("renamed"), Some(to)) => {
                    renamed_durable_objects.push((class, to));
                    continue;
                }
                (Some("deleted"), _) => {
                    deleted_durable_objects.push(class);
                    continue;
                }
                _ => {}
            }
        }
        let live = export.kind == "durable-object"
            && export
                .state
                .as_deref()
                .is_none_or(|state| state == "created");
        if !live {
            continue;
        }
        let storage = export.storage.ok_or_else(|| {
            format!("`exports.{class}` declares a Durable Object with no `storage`")
        })?;
        durable_objects.push(DurableObject { class, storage });
    }
    let (binds_to, mut foreign): (Vec<ServiceBinding>, Vec<ServiceBinding>) = parsed
        .services
        .into_iter()
        .partition(|s| project_workers.contains(&s.service));
    foreign.sort_by(|a, b| a.binding.cmp(&b.binding));
    let mut r2_buckets: Vec<R2Decl> = parsed
        .r2_buckets
        .into_iter()
        .map(|b| R2Decl {
            binding: b.binding,
            name: b.bucket_name,
            jurisdiction: b.jurisdiction,
        })
        .collect();
    r2_buckets.sort_by(|a, b| a.binding.cmp(&b.binding));
    let mut ratelimits: Vec<String> = parsed.ratelimits.into_iter().map(|r| r.name).collect();
    ratelimits.sort();
    Ok(Resources {
        binds_to: binds_to.into_iter().map(|s| s.service).collect(),
        foreign_services: foreign
            .into_iter()
            .map(|s| ForeignService {
                binding: s.binding,
                worker: s.service,
            })
            .collect(),
        queues,
        durable_objects,
        renamed_durable_objects,
        deleted_durable_objects,
        needs_kv: parsed
            .kv_namespaces
            .iter()
            .any(|ns| ns.binding == bynk_emit::emitter::wrangler::KV_BINDING_NAME),
        kv_instances: {
            let mut instances: Vec<String> = parsed
                .kv_namespaces
                .into_iter()
                .map(|ns| ns.binding)
                .filter(|b| b != bynk_emit::emitter::wrangler::KV_BINDING_NAME)
                .collect();
            instances.sort();
            instances
        },
        r2_buckets,
        ratelimits,
        declared_secrets: secrets.declared,
        read_secrets: secrets.read,
        reads_complete: secrets.read_complete,
        foreign_callers: secrets.foreign,
    })
}

/// Read the whole project's declared resources, one config read per worker.
/// `workers` is every Worker the build holds: a Service Binding to anything
/// else is to a foreign Worker.
pub(crate) fn project_resources(
    workers_dir: &Path,
    workers: &[String],
) -> Result<BTreeMap<String, Resources>, String> {
    let mut all = BTreeMap::new();
    for worker in workers {
        let resources = read_resources(&workers_dir.join(worker).join("wrangler.toml"), workers)
            .map_err(|e| format!("could not read the configuration for `{worker}`: {e}"))?;
        all.insert(worker.clone(), resources);
    }
    Ok(all)
}

/// v0.177 (#643): the contract manifest a Worker's build emitted.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct ContractsManifest {
    #[serde(default)]
    version: u32,
    /// This context's own hash per `on call` service — what its entry enforces.
    #[serde(default)]
    pub(crate) provides: BTreeMap<String, String>,
    /// This context's compiled view of each dependency's contract — the hash it
    /// stamps at each call site, keyed by the dependency's qualified name.
    #[serde(default)]
    pub(crate) expects: BTreeMap<String, BTreeMap<String, String>>,
    /// I4 (#1970): what each declared foreign caller's generated client
    /// embeds — actor → service → hash. Additive within version 1: absent for
    /// a context with no foreign caller, and for a build from an older
    /// compiler.
    #[serde(default)]
    pub(crate) foreign: BTreeMap<String, BTreeMap<String, String>>,
}

const CONTRACTS_MANIFEST_VERSION: u32 = 1;

/// Read one Worker's contract manifest.
///
/// An absent file is the empty answer, not an error: a context that neither
/// exposes nor calls an `on call` service emits none, and a build tree from a
/// compiler predating this file has none either. An *unreadable* or
/// *unparseable* one is an error — it sits beside the config we just read, so a
/// failure there means the tree is damaged rather than merely old.
/// #1883: the served paths of the static files a compiled Worker ships —
/// every file under its assets directory, as the URL path it is served at
/// (`/_assets/board.<hash>.css`, and since slice 2 fixed paths such as
/// `/favicon.ico`), except the platform's own `_headers` — for the deploy
/// lock. No directory is an empty set (a Worker with no assets). A hashed name
/// already carries its content hash, so no manifest is needed.
pub(crate) fn served_assets(worker_dir: &Path) -> BTreeSet<String> {
    fn walk(dir: &Path, url: &str, out: &mut BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(ty) = entry.file_type() else { continue };
            if ty.is_dir() {
                walk(&entry.path(), &format!("{url}{name}/"), out);
            } else if ty.is_file() && !(url == "/" && name == "_headers") {
                out.insert(format!("{url}{name}"));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(
        &worker_dir.join(bynk_emit::project::WORKER_ASSETS_DIR),
        "/",
        &mut out,
    );
    out
}

pub(crate) fn read_contracts_manifest(worker_dir: &Path) -> Result<ContractsManifest, String> {
    let path = worker_dir.join(bynk_emit::emitter::contracts::CONTRACTS_MANIFEST);
    if !path.exists() {
        return Ok(ContractsManifest::default());
    }
    let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
    let manifest: ContractsManifest = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    if manifest.version != CONTRACTS_MANIFEST_VERSION {
        return Err(format!(
            "unsupported {} version {}",
            bynk_emit::emitter::contracts::CONTRACTS_MANIFEST,
            manifest.version
        ));
    }
    Ok(manifest)
}

// P7.4 (#1305): a thin wrapper over `bynk_emit::emitter::wrangler::
// materialise_kv_namespace_ids` — the structural read-modify-write closing
// R7.6/R8.20. `-> bool` and the read-then-write shape are kept unchanged:
// both callers (`deploy/provisioning.rs`, `deploy/ledger.rs`) already treat
// `false` uniformly as "could not write" and don't need to distinguish read
// failure from a malformed file from a write failure.
//
// S4b (#1967): by binding, so each of a Worker's namespaces gets its own id.
pub(crate) fn materialise_kv_ids(path: &Path, ids: &BTreeMap<String, String>) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(patched) = bynk_emit::emitter::wrangler::materialise_kv_namespace_ids(&text, ids) else {
        return false;
    };
    // Review of #1305, finding 3: `materialise_kv_namespace_ids` returning
    // `text` unchanged (no `[[kv_namespaces]]` at all, or already
    // materialised) is a no-op by design — skip the write rather than
    // reopening the file to write back what's already there, restoring the
    // old `materialise_kv_id`'s own contract (a guaranteed `true`, no write
    // attempted, for a project that needed nothing materialised).
    if patched == text {
        return true;
    }
    std::fs::write(path, patched).is_ok()
}

/// P3 (#2342): materialise the rate limiters' allocated `namespace_id`s into
/// the generated config at `path`, as [`materialise_kv_ids`] does KV ids.
pub(crate) fn materialise_ratelimit_ids(path: &Path, ids: &BTreeMap<String, String>) -> bool {
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(patched) = bynk_emit::emitter::wrangler::materialise_ratelimit_namespace_ids(&text, ids)
    else {
        return false;
    };
    if patched == text {
        return true;
    }
    std::fs::write(path, patched).is_ok()
}

/// The environment-qualified physical name for a resource whose logical
/// identity is chosen by the source (`on queue "n"`) or the compiler (a
/// target context's `worker_dir_name`) — slice 4, DECISION C.
///
/// `"default"` is unqualified, which is the property that keeps a plain
/// `bynk deploy` byte-for-byte unchanged from before this slice. Any other
/// environment gets Cloudflare's own suffix shape (`<name>-<env>`) — not a
/// free choice for a Service Binding target: Cloudflare auto-suffixes a
/// Worker's *own* deployed name the same way under `--env`, so a binding must
/// match it exactly to resolve. Queues are not Cloudflare-mandated to follow
/// this shape, but using the same one keeps the two resource kinds — and the
/// mental model — consistent.
pub(crate) fn env_qualify(environment: &str, name: &str) -> String {
    if environment == "default" {
        name.to_string()
    } else {
        format!("{name}-{environment}")
    }
}

/// The name project Worker `worker` is deployed under in `environment`, and
/// so the name every Service Binding to it must carry (#2028).
///
/// The one place both sides of a deploy get this name: the plan prints it
/// (each context's `worker`, `binds_to`, and the `order` line), and
/// [`synthesise_environment_block`] writes it into the `[env.<name>]` block
/// the push hands Wrangler, as the block's `name` and as each project
/// Service Binding's `service`. So what `--dry-run` says is what the push
/// creates, by construction rather than by agreement.
///
/// It is [`env_qualify`]: `default` is the bare name, any other environment
/// `<worker>-<env>`. Writing `name` explicitly changes nothing Wrangler would
/// have done on its own: under `--env`, Wrangler 4.149.0 names an
/// environment that sets no `name` `<top-level name>-<env>` (`inheritable`
/// with `appendEnvName`; its `legacy_env` removal note: "each environment is
/// now deployed as its own Worker named `<name>-<environment>`"), and uses an
/// environment's own `name` as given. Stating it makes Bynk, not a Wrangler
/// default, the owner of the name the plan promises.
pub(crate) fn deployed_worker_name(environment: &str, worker: &str) -> String {
    env_qualify(environment, worker)
}

/// The `bynk deploy` command a message suggests, for the environment the run
/// is deploying (#2047): `bynk deploy` for `default`, `bynk deploy --env
/// <env>` otherwise. A remedy that dropped `--env` would deploy the default
/// environment, not the one that needs it.
pub(crate) fn deploy_command(environment: &str) -> String {
    if environment == "default" {
        "bynk deploy".to_string()
    } else {
        format!("bynk deploy --env {environment}")
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

/// P5 (#2323): the name an `R2` instance's bucket has on the account in
/// `environment`: its derived name, [`env_qualify`]d. R2 bucket names are
/// scoped to the account, so (like a Worker's) it is a function of the
/// project's names alone.
pub(crate) fn r2_bucket_name(environment: &str, name: &str) -> String {
    env_qualify(environment, name)
}

/// P5 (#2323): why `name` cannot be an R2 bucket's, or `None` when it can.
/// R2 takes 3 to 63 characters of lowercase letters, digits and hyphens, not
/// starting or ending with a hyphen. Checked at plan time, so a derived name
/// that breaks the rule is an error naming the instance, not a failed push.
pub(crate) fn r2_bucket_name_problem(name: &str) -> Option<&'static str> {
    if !(3..=63).contains(&name.len()) {
        return Some("it must be 3 to 63 characters");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return Some("it may hold only lowercase letters, digits and hyphens");
    }
    if name.starts_with('-') || name.ends_with('-') {
        return Some("it may not start or end with a hyphen");
    }
    None
}

/// Synthesise a `[env.<name>]` table covering every binding Wrangler does not
/// inherit into a named environment, and append it to the generated config —
/// slice 4, DECISIONs B and C combined.
///
/// **Why this exists at all.** Confirmed against Cloudflare's own docs:
/// bindings (`kv_namespaces`, `queues.consumers`, `durable_objects`,
/// `services`) are *non-inheritable* — a bare `--env staging` against the
/// flat config `emit_wrangler_toml` writes would deploy with **zero**
/// bindings. `emit_wrangler_toml` cannot fix this itself: it runs at compile
/// time, before any `--env` is known (there is no `environment` concept
/// anywhere in `bynk-syntax`/`bynk-check`), so the driver is the only place
/// this can live.
///
/// **Why this appends rather than edits.** The top-level stanza must stay
/// exactly as emitted — it continues to serve the plain, no-`--env`
/// `bynk deploy`. So this parses the config only to *read* the values it
/// needs (via `toml::Table`, not the narrow read-only `WranglerConfig`/
/// `ServiceBinding`/`QueueConsumer` structs above, which drop fields — e.g.
/// `ServiceBinding` has no `binding` — that must be copied byte-for-byte),
/// builds a
/// *separate* `{ env: { <name>: … } }` table, serialises only that
/// fragment (so TOML string-escaping is the
/// `toml` crate's job, not a hand-rolled duplicate of
/// `bynk-emit`'s private `escape_toml_basic_string`), and appends the result
/// as text. The original bytes are never touched.
///
/// The Worker's own `name` and the targets of Service Bindings to project
/// contexts are [`deployed_worker_name`]s, and queue names are
/// environment-qualified ([`env_qualify`]); a foreign Worker's is copied as
/// declared, since Bynk did not name it (interop I5, #1972); each KV namespace gets its resolved id for this
/// environment, by binding (S4b, #1967); each rate limiter is copied with the
/// `namespace_id` already materialised for this environment (P3, #2342); Durable
/// Object bindings, `exports`, and cron triggers carry no per-environment
/// identity and are copied verbatim.
///
/// #1796: unlike the bindings, `exports` *is* inheritable — Wrangler 4.107's
/// `normalizeAndValidateEnvironment` reads it with `inheritable(…)`, as it
/// did `migrations` — so copying it is not what makes it apply. It is copied
/// anyway, so the `[env.<name>]` block states every Durable Object fact its
/// bindings rely on rather than leaning on an inheritance rule the bindings
/// beside it don't follow. The two can't disagree: the copy is verbatim.
///
/// #1890: `compatibility_date` and `compatibility_flags` are inheritable too,
/// and nothing here is environment-specific about them, so neither is copied:
/// every environment runs under the top level's date and flags.
pub(crate) fn synthesise_environment_block(
    config_text: &str,
    environment: &str,
    // The resolved id of each KV namespace, by binding. Empty for a context
    // with no KV.
    kv_ids: &BTreeMap<String, String>,
    // Interop I5 (#1972): the bindings of Service Bindings to foreign
    // Workers, whose targets are copied as declared.
    foreign_bindings: &BTreeSet<String>,
) -> Result<String, String> {
    let doc: toml::Table = config_text
        .parse()
        .map_err(|e| format!("generated configuration is not valid TOML: {e}"))?;

    let mut env_block = toml::Table::new();

    // #2028: the deployed name, from the function the plan prints it with.
    if let Some(name) = doc.get("name").and_then(toml::Value::as_str) {
        env_block.insert(
            "name".to_string(),
            toml::Value::String(deployed_worker_name(environment, name)),
        );
    }

    if !kv_ids.is_empty()
        && let Some(toml::Value::Array(namespaces)) = doc.get("kv_namespaces")
    {
        let mut namespaces = namespaces.clone();
        for ns in &mut namespaces {
            if let toml::Value::Table(t) = ns
                && let Some(id) = t
                    .get("binding")
                    .and_then(toml::Value::as_str)
                    .and_then(|b| kv_ids.get(b))
            {
                t.insert("id".to_string(), toml::Value::String(id.clone()));
            }
        }
        env_block.insert("kv_namespaces".to_string(), toml::Value::Array(namespaces));
    }

    // P5 (#2323): each bucket under its environment-qualified name.
    if let Some(toml::Value::Array(buckets)) = doc.get("r2_buckets") {
        let mut buckets = buckets.clone();
        for bucket in &mut buckets {
            if let toml::Value::Table(t) = bucket
                && let Some(name) = t
                    .get("bucket_name")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            {
                t.insert(
                    "bucket_name".to_string(),
                    toml::Value::String(r2_bucket_name(environment, &name)),
                );
            }
        }
        env_block.insert("r2_buckets".to_string(), toml::Value::Array(buckets));
    }

    // P3 (#2342): the rate limiters, copied as the top level has them. The
    // push materialises this environment's `namespace_id`s into the top
    // level before this block is synthesised from it, so they are already
    // the right ones; the limit and window are the same in every environment.
    if let Some(ratelimits) = doc.get("ratelimits") {
        env_block.insert("ratelimits".to_string(), ratelimits.clone());
    }

    if let Some(mut queues) = doc.get("queues").cloned() {
        if let toml::Value::Table(q) = &mut queues
            && let Some(toml::Value::Array(consumers)) = q.get_mut("consumers")
        {
            for consumer in consumers.iter_mut() {
                if let toml::Value::Table(t) = consumer
                    && let Some(name) = t
                        .get("queue")
                        .and_then(toml::Value::as_str)
                        .map(str::to_owned)
                {
                    t.insert(
                        "queue".to_string(),
                        toml::Value::String(env_qualify(environment, &name)),
                    );
                }
            }
        }
        env_block.insert("queues".to_string(), queues);
    }

    if let Some(toml::Value::Array(services)) = doc.get("services") {
        let mut services = services.clone();
        for service in &mut services {
            let foreign = matches!(service, toml::Value::Table(t)
                if t.get("binding").and_then(toml::Value::as_str).is_some_and(|b| foreign_bindings.contains(b)));
            if !foreign
                && let toml::Value::Table(t) = service
                && let Some(target) = t
                    .get("service")
                    .and_then(toml::Value::as_str)
                    .map(str::to_owned)
            {
                t.insert(
                    "service".to_string(),
                    toml::Value::String(deployed_worker_name(environment, &target)),
                );
            }
        }
        env_block.insert("services".to_string(), toml::Value::Array(services));
    }

    for key in ["durable_objects", "exports", "triggers"] {
        if let Some(value) = doc.get(key) {
            env_block.insert(key.to_string(), value.clone());
        }
    }

    if env_block.is_empty() {
        // No name and nothing to bind (never true of a config `bynk build`
        // emits, which always names its Worker) — leave the config exactly as
        // emitted rather than appending an empty table.
        return Ok(config_text.to_string());
    }

    let mut env_table = toml::Table::new();
    env_table.insert(environment.to_string(), toml::Value::Table(env_block));
    let mut wrapper = toml::Table::new();
    wrapper.insert("env".to_string(), toml::Value::Table(env_table));

    let appended = toml::to_string_pretty(&wrapper)
        .map_err(|e| format!("could not serialise the `[env.{environment}]` block: {e}"))?;
    Ok(format!("{config_text}\n{appended}"))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// A fluent literal for what one context declares, so a test names only the
    /// resources it is about: `Resources::default().needs_kv().exports(&["Cart"])`.
    impl Resources {
        pub(crate) fn binds(mut self, targets: &[&str]) -> Self {
            self.binds_to = names(targets);
            self
        }
        pub(crate) fn consumes(mut self, queues: &[&str]) -> Self {
            self.queues = names(queues);
            self
        }
        /// SQLite-backed Durable Object classes — the only backend the
        /// emitter declares (#1796, ADR 0438).
        pub(crate) fn exports(mut self, classes: &[&str]) -> Self {
            self.durable_objects = classes
                .iter()
                .map(|class| DurableObject {
                    class: class.to_string(),
                    storage: "sqlite".to_string(),
                })
                .collect();
            self
        }
        pub(crate) fn needs_kv(mut self) -> Self {
            self.needs_kv = true;
            self
        }
        /// Derived KV instances, by binding (S4b, #1967).
        pub(crate) fn kv_instances(mut self, bindings: &[&str]) -> Self {
            self.kv_instances = names(bindings);
            self
        }
        /// Derived `R2` instances, as `(binding, derived name, jurisdiction)`
        /// (P5, #2323).
        pub(crate) fn r2_buckets(mut self, buckets: &[(&str, &str, Option<&str>)]) -> Self {
            self.r2_buckets = buckets
                .iter()
                .map(|(binding, name, jurisdiction)| R2Decl {
                    binding: binding.to_string(),
                    name: name.to_string(),
                    jurisdiction: jurisdiction.map(str::to_string),
                })
                .collect();
            self
        }
        /// The secret names the emitted manifest carries — an `actor`'s `auth`
        /// secret, which the compiler proved this Worker reads.
        pub(crate) fn declares(mut self, secrets: &[&str]) -> Self {
            self.declared_secrets = names(secrets);
            self
        }
        /// Literal `bynk.Secrets` names the compiler saw this Worker read.
        pub(crate) fn reads(mut self, secrets: &[&str]) -> Self {
            self.read_secrets = names(secrets);
            self
        }
        /// Service Bindings to foreign Workers, as `(binding, worker)`
        /// (interop I5, #1972).
        pub(crate) fn foreign(mut self, services: &[(&str, &str)]) -> Self {
            self.foreign_services = services
                .iter()
                .map(|(binding, worker)| ForeignService {
                    binding: binding.to_string(),
                    worker: worker.to_string(),
                })
                .collect();
            self
        }
        /// This context reads at least one secret the compiler cannot name.
        pub(crate) fn reads_incompletely(mut self) -> Self {
            self.reads_complete = false;
            self
        }
    }

    pub(crate) fn project(specs: Vec<(&str, Resources)>) -> BTreeMap<String, Resources> {
        specs
            .into_iter()
            .map(|(worker, r)| (worker.to_string(), r))
            .collect()
    }

    /// A temp path unique to this process and call site.
    fn temp_config(label: &str) -> std::path::PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("bynk-{label}-{}-{unique}.toml", std::process::id()))
    }

    /// The Workers the tests' configs are built beside: a `[[services]]`
    /// target among them is a project context, any other a foreign Worker.
    const PROJECT_WORKERS: &[&str] = &["payment", "commerce-payment"];

    /// Parse `config` as `deploy` does. The literals in these tests are the
    /// emitter's real output (`bynk-emit/src/emitter/wrangler.rs`), pinned
    /// fixture-side by `bynkc/tests/fixtures/positive/` — 121 (an agent), 151
    /// (a queue) and 372 (KV + agent + queue in one context). Deploy reads what
    /// the emitter writes, so the two must depict the same file.
    fn parse_config(label: &str, text: &str) -> Resources {
        let path = temp_config(label);
        std::fs::write(&path, text).unwrap();
        let resources =
            read_resources(&path, &names(PROJECT_WORKERS)).expect("the emitted config parses");
        let _ = std::fs::remove_file(&path);
        resources
    }

    /// A config carrying every v1 binding kind — mirrors what
    /// `emit_wrangler_toml` actually writes (`bynk-emit/src/emitter/
    /// wrangler.rs`), so a gap here is a gap the real emitter output would
    /// hit too.
    const FULL_CONFIG: &str = r#"
# Generated by bynkc — do not edit by hand.
name = "api"
main = "index.ts"
compatibility_date = "2026-07-01"
compatibility_flags = ["global_fetch_strictly_public"]

[[services]]
binding = "PAYMENT"
service = "payment"

[[kv_namespaces]]
binding = "KV"
id = "<KV_NAMESPACE_ID>" # set at deploy time

[[durable_objects.bindings]]
name = "ORDER_ENTITY"
class_name = "OrderEntity"

[exports.OrderEntity]
type = "durable-object"
storage = "sqlite"

[triggers]
crons = ["*/5 * * * *"]

[[queues.consumers]]
queue = "jobs"
max_batch_size = 10
"#;

    fn kv_ids(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(b, i)| (b.to_string(), i.to_string()))
            .collect()
    }

    // ---- #835 slice 4: environments -------------------------------------

    /// S4b (#1967): each namespace in the `[env.<name>]` block gets its own
    /// id, by binding, never another's.
    #[test]
    fn synthesise_environment_block_gives_each_kv_namespace_its_own_id() {
        let config = "name = \"shop-cart\"\n\n[[kv_namespaces]]\nbinding = \"KV\"\nid = \"<KV_NAMESPACE_ID>\"\n\n[[kv_namespaces]]\nbinding = \"KV_SESSIONS\"\nid = \"<KV_SESSIONS_ID>\"\n";
        let out = synthesise_environment_block(
            config,
            "staging",
            &kv_ids(&[("KV", "bare-staging"), ("KV_SESSIONS", "sessions-staging")]),
            &BTreeSet::new(),
        )
        .unwrap();
        let doc: toml::Table = out.parse().unwrap();
        let namespaces = doc["env"]["staging"]["kv_namespaces"].as_array().unwrap();
        let ids: Vec<(&str, &str)> = namespaces
            .iter()
            .map(|ns| (ns["binding"].as_str().unwrap(), ns["id"].as_str().unwrap()))
            .collect();
        assert_eq!(
            ids,
            [("KV", "bare-staging"), ("KV_SESSIONS", "sessions-staging")]
        );
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

    #[test]
    fn env_qualify_is_a_no_op_for_default_and_suffixes_otherwise() {
        assert_eq!(env_qualify("default", "jobs"), "jobs");
        assert_eq!(env_qualify("staging", "jobs"), "jobs-staging");
        assert_eq!(env_qualify("staging", "payment"), "payment-staging");
    }

    #[test]
    fn synthesise_environment_block_covers_every_binding_kind() {
        let out = synthesise_environment_block(
            FULL_CONFIG,
            "staging",
            &kv_ids(&[("KV", "kv-staging-id")]),
            &BTreeSet::new(),
        )
        .expect("valid TOML in, valid TOML out");

        // The top level is untouched — it still serves a plain `bynk deploy`.
        assert!(
            out.starts_with(FULL_CONFIG),
            "the original bytes must survive verbatim: {out}"
        );

        // Parsed back, not string-matched: proves the appended block is valid
        // TOML with the right shape, not merely text that looks right.
        let doc: toml::Table = out.parse().expect("synthesised output is valid TOML");
        let env = doc["env"]["staging"].as_table().expect("env.staging");

        // #1890: `compatibility_flags` is inheritable, so the environment
        // takes the top level's rather than a copy.
        assert!(!env.contains_key("compatibility_flags"));

        assert_eq!(env["kv_namespaces"][0]["binding"].as_str(), Some("KV"));
        assert_eq!(
            env["kv_namespaces"][0]["id"].as_str(),
            Some("kv-staging-id"),
            "the KV id is this environment's resolved id, not the placeholder"
        );

        assert_eq!(
            env["services"][0]["binding"].as_str(),
            Some("PAYMENT"),
            "the binding name carries over unqualified"
        );
        assert_eq!(
            env["services"][0]["service"].as_str(),
            Some("payment-staging"),
            "the target is environment-qualified — DECISION C's Service Binding half"
        );
        assert_eq!(
            env["name"].as_str(),
            Some("api-staging"),
            "#2028: the deployed name is stated, from the plan's own function"
        );

        assert_eq!(
            env["queues"]["consumers"][0]["queue"].as_str(),
            Some("jobs-staging"),
            "the physical queue name is environment-qualified — DECISION C's queue half"
        );
        assert_eq!(
            env["queues"]["consumers"][0]["max_batch_size"].as_integer(),
            Some(10)
        );

        assert_eq!(
            env["durable_objects"]["bindings"][0]["class_name"].as_str(),
            Some("OrderEntity"),
            "DO bindings carry no per-environment identity — copied verbatim"
        );
        // #1796: Wrangler would inherit `exports` into the environment anyway,
        // but the block states it beside the bindings that rely on it.
        assert_eq!(
            env["exports"]["OrderEntity"]["type"].as_str(),
            Some("durable-object"),
            "exports carry no per-environment identity — copied verbatim"
        );
        assert_eq!(
            env["exports"]["OrderEntity"]["storage"].as_str(),
            Some("sqlite")
        );
        assert!(
            env.get("migrations").is_none(),
            "`migrations` and `exports` are mutually exclusive in one environment"
        );
        assert_eq!(
            env["triggers"]["crons"][0].as_str(),
            Some("*/5 * * * *"),
            "crons carry no per-environment identity — copied verbatim"
        );
    }

    #[test]
    fn synthesise_environment_block_is_unchanged_for_the_default_environment() {
        // `deploy_one` only calls this for a non-default environment, but the
        // function itself staying a faithful no-op at the boundary is worth
        // pinning: "default" must never gain a synthesised block.
        let out = synthesise_environment_block(
            FULL_CONFIG,
            "default",
            &kv_ids(&[("KV", "kv-id")]),
            &BTreeSet::new(),
        )
        .unwrap();
        let doc: toml::Table = out.parse().unwrap();
        assert_eq!(
            doc["env"]["default"]["queues"]["consumers"][0]["queue"].as_str(),
            Some("jobs"),
            "\"default\" qualifies to itself, so this is still a no-op in effect"
        );
    }

    #[test]
    fn synthesise_environment_block_names_the_worker_even_with_nothing_to_bind() {
        // #2028: the deployed name is stated whether or not anything needs
        // binding, so the push never leans on Wrangler's default for it.
        let bare = "name = \"api\"\nmain = \"index.ts\"\n";
        let out = synthesise_environment_block(bare, "staging", &BTreeMap::new(), &BTreeSet::new())
            .unwrap();
        assert!(out.starts_with(bare), "the top level is untouched: {out}");
        let doc: toml::Table = out.parse().unwrap();
        let block = doc["env"]["staging"].as_table().unwrap();
        assert_eq!(block["name"].as_str(), Some("api-staging"));
        assert_eq!(block.len(), 1, "a name and nothing else: {out}");
    }

    #[test]
    fn synthesise_environment_block_is_a_true_no_op_with_nothing_to_state() {
        let nameless = "main = \"index.ts\"\n";
        let out =
            synthesise_environment_block(nameless, "staging", &BTreeMap::new(), &BTreeSet::new())
                .unwrap();
        assert_eq!(
            out, nameless,
            "no name and no binding appends nothing, not an empty [env.staging]"
        );
    }

    #[test]
    fn synthesise_environment_block_rejects_unparseable_config() {
        assert!(
            synthesise_environment_block(
                "not = valid = toml = =",
                "staging",
                &BTreeMap::new(),
                &BTreeSet::new()
            )
            .is_err()
        );
    }

    #[test]
    fn service_targets_are_read_from_the_generated_config() {
        // The graph is read from the emitted `[[services]]` — the same file
        // wrangler uploads — so parse the real shape the emitter writes.
        assert_eq!(
            parse_config(
                "services",
                r#"
name = "commerce-orders"
main = "index.ts"
compatibility_date = "2026-07-01"
compatibility_flags = ["global_fetch_strictly_public"]

[[services]]
binding = "COMMERCE_PAYMENT"
service = "commerce-payment"
"#,
            ),
            Resources::default().binds(&["commerce-payment"]),
        );

        // A config with no bindings is the common single-context case, and
        // declares nothing else either.
        assert_eq!(
            parse_config("bare", "name = \"api\"\nmain = \"index.ts\"\n"),
            Resources::default(),
        );
    }

    #[test]
    fn a_queue_only_context_declares_its_queues_by_name() {
        // `bynkc/tests/fixtures/positive/153_queue_multiple` — two
        // `from queue(...)` services in one context.
        assert_eq!(
            parse_config(
                "queues",
                r#"
name = "jobs"
main = "index.ts"
compatibility_date = "2026-07-01"
compatibility_flags = ["global_fetch_strictly_public"]

[[queues.consumers]]
queue = "high-priority"
max_batch_size = 10

[[queues.consumers]]
queue = "low-priority"
max_batch_size = 10
"#,
            ),
            Resources::default().consumes(&["high-priority", "low-priority"]),
        );
    }

    #[test]
    fn a_do_only_context_declares_the_classes_the_push_will_export() {
        // `bynkc/tests/fixtures/positive/121_workers_with_agent` — an agent, so
        // a DO binding and the `exports` entry that registers its class.
        assert_eq!(
            parse_config(
                "durable-objects",
                r#"
name = "cart"
main = "index.ts"
compatibility_date = "2026-07-01"
compatibility_flags = ["global_fetch_strictly_public"]

[[durable_objects.bindings]]
name = "CART_ENTITY"
class_name = "CartEntity"

[exports.CartEntity]
type = "durable-object"
storage = "sqlite"
"#,
            ),
            Resources::default().exports(&["CartEntity"]),
        );
    }

    #[test]
    fn a_context_declaring_every_v1_resource_is_read_whole() {
        // `bynkc/tests/fixtures/positive/372_kv_agent_queue_workers` — the
        // combination slice 1 completes: KV (slice 0), an agent's export and
        // a queue, in one context. Each kind is read independently, so one
        // present must not mask another.
        assert_eq!(
            parse_config(
                "everything",
                r#"
name = "ops-hub"
main = "index.ts"
compatibility_date = "2026-07-01"
compatibility_flags = ["global_fetch_strictly_public"]

[[kv_namespaces]]
binding = "KV"
id = "<KV_NAMESPACE_ID>" # set at deploy time

[[durable_objects.bindings]]
name = "JOB_LEDGER"
class_name = "JobLedger"

[exports.JobLedger]
type = "durable-object"
storage = "sqlite"

[[queues.consumers]]
queue = "job-intake"
max_batch_size = 10
"#,
            ),
            Resources::default()
                .needs_kv()
                .exports(&["JobLedger"])
                .consumes(&["job-intake"]),
        );
    }

    #[test]
    fn only_live_durable_object_exports_are_read() {
        // #1796: `exports` is Cloudflare's map for more than Durable Objects
        // (a `type = "worker"` entrypoint), and a tombstone (`deleted`,
        // `renamed`) names a class the push *retires*. Neither is a class the plan may claim is declared.
        // The emitter writes only the first kind today; the rule is written
        // for the file, not for the emitter's current habit. The storage is
        // read, not assumed, so a hand-edited `legacy-kv` shows as one.
        assert_eq!(
            parse_config(
                "exports-kinds",
                r#"
name = "cart"
main = "index.ts"

[exports.Basket]
type = "durable-object"
state = "created"
storage = "legacy-kv"

[exports.Cart]
type = "durable-object"
storage = "sqlite"

[exports.Gone]
type = "durable-object"
state = "deleted"

[exports.Old]
type = "durable-object"
state = "renamed"
renamed_to = "Cart"

[exports.Entry]
type = "worker"
"#,
            )
            .durable_objects,
            vec![
                DurableObject {
                    class: "Basket".to_string(),
                    storage: "legacy-kv".to_string(),
                },
                DurableObject {
                    class: "Cart".to_string(),
                    storage: "sqlite".to_string(),
                },
            ],
        );
    }

    /// State-migrations S3 (#2248): tombstones are read as what they do — a
    /// rename (old → new) and a deletion — never as declared classes.
    #[test]
    fn tombstones_are_read_as_renames_and_deletions() {
        let resources = parse_config(
            "exports-tombstones",
            r#"
name = "shop"
main = "index.ts"

[exports.Basket]
type = "durable-object"
storage = "sqlite"

[exports.Cart]
type = "durable-object"
state = "renamed"
renamed_to = "Basket"

[exports.Wishlist]
type = "durable-object"
state = "deleted"
"#,
        );
        assert_eq!(
            resources.renamed_durable_objects,
            vec![("Cart".to_string(), "Basket".to_string())]
        );
        assert_eq!(
            resources.deleted_durable_objects,
            vec!["Wishlist".to_string()]
        );
        assert_eq!(resources.durable_objects.len(), 1);
    }

    #[test]
    fn r2_buckets_are_read_with_their_names_and_jurisdictions() {
        // P5 (#2323): fixture `2323_r2_workers`'s buckets, in binding order.
        let resources = parse_config(
            "r2-buckets",
            "name = \"registry-store\"\n\n[[r2_buckets]]\nbinding = \"R2_INDEXES\"\nbucket_name = \"registry-store-indexes\"\njurisdiction = \"eu\"\n\n[[r2_buckets]]\nbinding = \"R2_ARCHIVES\"\nbucket_name = \"registry-store-archives\"\n",
        );
        assert_eq!(
            resources.r2_buckets,
            [
                R2Decl {
                    binding: "R2_ARCHIVES".into(),
                    name: "registry-store-archives".into(),
                    jurisdiction: None
                },
                R2Decl {
                    binding: "R2_INDEXES".into(),
                    name: "registry-store-indexes".into(),
                    jurisdiction: Some("eu".into())
                },
            ]
        );
        assert!(!resources.needs_kv && resources.kv_instances.is_empty());
    }

    #[test]
    fn a_named_environment_qualifies_each_bucket_and_keeps_its_jurisdiction() {
        let config = "name = \"registry-store\"\n\n[[r2_buckets]]\nbinding = \"R2_INDEXES\"\nbucket_name = \"registry-store-indexes\"\njurisdiction = \"eu\"\n";
        let out =
            synthesise_environment_block(config, "staging", &BTreeMap::new(), &BTreeSet::new())
                .unwrap();
        let doc: toml::Table = out.parse().unwrap();
        let buckets = doc["env"]["staging"]["r2_buckets"].as_array().unwrap();
        assert_eq!(
            buckets[0]["bucket_name"].as_str(),
            Some("registry-store-indexes-staging")
        );
        assert_eq!(buckets[0]["jurisdiction"].as_str(), Some("eu"));
        // The top level still serves the plain `bynk deploy`.
        assert!(out.starts_with(config));
        assert_eq!(
            r2_bucket_name("default", "registry-store-indexes"),
            "registry-store-indexes"
        );
    }

    #[test]
    fn r2_names_are_held_to_r2s_rules() {
        assert_eq!(r2_bucket_name_problem("shop-cart-archives"), None);
        assert_eq!(r2_bucket_name_problem("a1b"), None);
        assert!(r2_bucket_name_problem("ab").is_some());
        assert!(r2_bucket_name_problem(&"a".repeat(64)).is_some());
        assert_eq!(r2_bucket_name_problem(&"a".repeat(63)), None);
        assert!(r2_bucket_name_problem("shop_cart-archives").is_some());
        assert!(r2_bucket_name_problem("Shop-cart").is_some());
        assert!(r2_bucket_name_problem("-shop").is_some());
        assert!(r2_bucket_name_problem("shop-").is_some());
    }

    #[test]
    fn the_bare_namespace_and_each_instance_are_read_apart() {
        // S4b (#1967): fixture `1966_derived_kv_workers`'s namespaces beside
        // the bare `KV`. The config lists them in emission order; the
        // instances come back in binding order.
        let resources = parse_config(
            "kv-instances",
            "name = \"shop-cart\"\n\n[[kv_namespaces]]\nbinding = \"KV\"\nid = \"<KV_NAMESPACE_ID>\"\n\n[[kv_namespaces]]\nbinding = \"KV_SESSIONS\"\nid = \"<KV_SESSIONS_ID>\"\n\n[[kv_namespaces]]\nbinding = \"KV_PRICE_CACHE\"\nid = \"<KV_PRICE_CACHE_ID>\"\n",
        );
        assert!(resources.needs_kv);
        assert_eq!(
            resources.kv_instances,
            names(&["KV_PRICE_CACHE", "KV_SESSIONS"])
        );
        let instances_only = parse_config(
            "kv-instances-only",
            "name = \"shop-cart\"\n\n[[kv_namespaces]]\nbinding = \"KV_SESSIONS\"\nid = \"<KV_SESSIONS_ID>\"\n",
        );
        assert!(
            !instances_only.needs_kv,
            "an instance is not the bare namespace"
        );
        assert_eq!(instances_only.kv_instances, names(&["KV_SESSIONS"]));
    }

    #[test]
    fn a_live_durable_object_export_without_storage_is_refused() {
        // Review of #1803: dropping it would understate the plan. The config
        // is the one the push sends, so a malformed entry stops the deploy
        // here, naming the class, before wrangler is ever asked.
        let path = temp_config("exports-no-storage");
        std::fs::write(
            &path,
            "name = \"cart\"\n\n[exports.Cart]\ntype = \"durable-object\"\n",
        )
        .unwrap();
        let err = read_resources(&path, &[]).expect_err("a live class with no backend is refused");
        let _ = std::fs::remove_file(&path);
        assert!(
            err.contains("`exports.Cart`") && err.contains("`storage`"),
            "{err}"
        );
    }

    #[test]
    fn the_manifest_is_absent_empty_or_versioned() {
        let dir = std::env::temp_dir().join(format!(
            "bynk-secrets-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(bynk_emit::emitter::secrets::SECRETS_MANIFEST);

        // Absent is the empty answer, not an error: a context with no secrets of
        // any kind emits no file. `read_complete` is true *vacuously* — such a
        // context names nothing, computed or otherwise.
        let absent = read_secrets_manifest(&dir).expect("an absent manifest is not an error");
        assert!(absent.declared.is_empty() && absent.read.is_empty());
        assert!(absent.read_complete);

        std::fs::write(
            &path,
            r#"{"version":2,"declared":["AUTH_JWT_SECRET"],"read":["API_KEY"],"read_complete":false}"#,
        )
        .unwrap();
        let read = read_secrets_manifest(&dir).expect("a v2 manifest parses");
        assert_eq!(read.declared, vec!["AUTH_JWT_SECRET".to_string()]);
        assert_eq!(read.read, vec!["API_KEY".to_string()]);
        assert!(!read.read_complete);

        // v1 is refused rather than read with defaults, and that is the point:
        // a v1 file carries no evidence either way about computed names, so
        // defaulting `read_complete` to true for it would make the driver claim
        // a completeness nothing established (ADR 0196 D2). The two constants
        // are independent — the emitter's `MANIFEST_VERSION` and this one — so
        // a stale build tree is exactly what this catches.
        std::fs::write(&path, r#"{"version":1,"declared":["AUTH_JWT_SECRET"]}"#).unwrap();
        assert!(read_secrets_manifest(&dir).is_err());

        // A version from the future is refused for the same reason.
        std::fs::write(&path, r#"{"version":3,"declared":[]}"#).unwrap();
        assert!(read_secrets_manifest(&dir).is_err());

        // Damaged rather than merely old — it sits beside a config we just read.
        std::fs::write(&path, "{not json").unwrap();
        assert!(read_secrets_manifest(&dir).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod served_assets_tests {
    use super::*;

    /// #1883, slice 1b: what a compiled Worker ships, read back for the lock.
    #[test]
    fn served_assets_lists_the_workers_hashed_files() {
        let dir = std::env::temp_dir().join(format!("bynk-served-assets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(served_assets(&dir).is_empty(), "no directory, no assets");
        std::fs::create_dir_all(dir.join("assets/_assets")).unwrap();
        std::fs::write(dir.join("assets/_assets/board.1a2b.css"), "a{}").unwrap();
        std::fs::write(dir.join("assets/_headers"), "x").unwrap();
        // #1883, slice 2: a fixed path, at the root or nested.
        std::fs::write(dir.join("assets/favicon.ico"), "i").unwrap();
        std::fs::create_dir_all(dir.join("assets/.well-known")).unwrap();
        std::fs::write(dir.join("assets/.well-known/security.txt"), "s").unwrap();
        let served = served_assets(&dir);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            served.into_iter().collect::<Vec<_>>(),
            vec![
                "/.well-known/security.txt".to_string(),
                "/_assets/board.1a2b.css".to_string(),
                "/favicon.ico".to_string(),
            ]
        );
    }
}

#[cfg(test)]
mod foreign_service_tests {
    use super::*;

    const WITH_FOREIGN: &str = r#"
name = "shop-billing"
main = "index.ts"
compatibility_date = "2026-07-01"
compatibility_flags = ["global_fetch_strictly_public"]

[[services]]
binding = "PAYMENT"
service = "payment"

[[services]]
binding = "SERVICE_LEDGER"
service = "ledger"

[[services]]
binding = "SERVICE_BILLING"
service = "legacy-billing"
"#;

    /// Interop I5 (#1972): a `[[services]]` target that is none of the
    /// build's Workers is a foreign Worker, kept out of the deploy graph.
    #[test]
    fn a_service_to_a_worker_outside_the_build_is_foreign() {
        let path = std::env::temp_dir().join(format!(
            "bynk-foreign-{}-{:?}.toml",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::write(&path, WITH_FOREIGN).unwrap();
        let resources = read_resources(&path, &["payment".to_string()]).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(resources.binds_to, ["payment"]);
        assert_eq!(
            resources.foreign_services,
            [
                ForeignService {
                    binding: "SERVICE_BILLING".into(),
                    worker: "legacy-billing".into()
                },
                ForeignService {
                    binding: "SERVICE_LEDGER".into(),
                    worker: "ledger".into()
                },
            ]
        );
    }

    /// Interop I5 (#1972): under `--env`, a project context's target is
    /// qualified and a foreign Worker's is copied as declared — Bynk did not
    /// name it, so `legacy-billing-staging` would be a Worker nobody made.
    #[test]
    fn synthesise_environment_block_leaves_a_foreign_target_as_declared() {
        let foreign: BTreeSet<String> = ["SERVICE_BILLING", "SERVICE_LEDGER"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let out = synthesise_environment_block(WITH_FOREIGN, "staging", &BTreeMap::new(), &foreign)
            .unwrap();
        let doc: toml::Table = out.parse().unwrap();
        let services: Vec<(&str, &str)> = doc["env"]["staging"]["services"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["binding"].as_str().unwrap(),
                    s["service"].as_str().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            services,
            [
                ("PAYMENT", "payment-staging"),
                ("SERVICE_LEDGER", "ledger"),
                ("SERVICE_BILLING", "legacy-billing"),
            ]
        );
    }
}
