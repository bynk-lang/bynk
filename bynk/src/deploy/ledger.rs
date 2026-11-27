use super::*;
use bynk_driver::emsg;

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
pub(crate) struct DeployLock {
    // No serde `default`: a ledger with no `version` is not a fresh project, it
    // is corruption (a truncated write), and must fail the read rather than
    // parse as an empty v1 ledger that re-mints every namespace (#736).
    version: u32,
    #[serde(default)]
    pub(crate) environments: BTreeMap<String, Environment>,
    /// #3112: the structured form behind recent contract hashes, oldest first.
    /// A hash that is live resolves here, and so does one a `bynk revert`
    /// restores from a version's deploy facts, which the ledger no longer
    /// listed. Each entry is trusted only while it hashes to the hash it is
    /// filed under. Pruned on write ([`DeployLock::record_shapes`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    contract_shapes: Vec<ContractShape>,
}

/// One entry of [`DeployLock::contract_shapes`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ContractShape {
    hash: String,
    /// `bynk_check::contract_compat::ServiceShape::to_text`.
    shape: String,
}

/// How many contract shapes no Worker's record references are kept, for a
/// revert to restore (#3112).
const RECENT_SHAPES: usize = 16;

/// #2077: what a Worker was compiled against — dependency (qualified context
/// name) → service → contract hash ([`WorkerRecord::expects`]).
pub(crate) type Expects = BTreeMap<String, BTreeMap<String, String>>;

fn lock_version() -> u32 {
    1
}

/// #3565 (D5): the version a ledger is written as while an agent move is open in any
/// environment. A bynk that predates moves refuses it ("unsupported deploy lock version"),
/// rather than reading past the open record it does not know and pushing the half-moved
/// classes as plain live ones.
const LOCK_VERSION_MOVING: u32 = 2;

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
    /// #3479: each Worker's `Vectorize` instances (`capability Docs =
    /// Vectorize …`), keyed by the instance's binding (`VECTORIZE_DOCS`). An
    /// index's name is its identity and its configuration is fixed at
    /// creation, so the record is the name it was created (or adopted) under.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project without Vectorize writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) vectorize_instances: BTreeMap<String, BTreeMap<String, VectorizeRecord>>,
    /// #3477: each Worker's adopted container images, keyed by the `Container`
    /// capability's name (`Renderer`): the digest-pinned reference
    /// `[deploy.containers.<Cap>] image` named for this environment when it was
    /// last pushed. A container built from its Dockerfile has no entry, because
    /// the image is `wrangler`'s to make and the ledger records nothing it does
    /// not own. It holds the image the last deploy attempt named, written before
    /// the push, so a push that then failed still shows here. What it is for: a
    /// changed or dropped adoption is shown in the plan, and `bynk revert` says which image a restored Worker takes for new
    /// starts.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project without an adopted image writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) container_images: BTreeMap<String, BTreeMap<String, String>>,
    /// P4a (#2602): each Worker's `D1` instances (`capability Orders = D1`),
    /// keyed by the instance's binding (`D1_ORDERS`), as `kv_instances` is:
    /// the database's account-facing name and the id Cloudflare minted for it
    /// (ADR 0194: the ledger owns the ids it mints).
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project without D1 writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) d1_instances: BTreeMap<String, BTreeMap<String, D1Record>>,
    /// #3319: each Worker's `Postgres` instances (`capability Main =
    /// Postgres`), keyed by the instance's binding (`HYPERDRIVE_MAIN`): the
    /// Hyperdrive config the deploy created or adopted for it, and the id
    /// the push binds. It holds an id and a name and nothing derived from
    /// the connection string: the ledger is committed, so even a digest of a
    /// secret there could be guessed offline.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project without `Postgres` writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) hyperdrive_instances: BTreeMap<String, BTreeMap<String, HyperdriveRecord>>,
    /// #3301: each Worker's `StoredSecret` instances, keyed by the instance's
    /// binding (`SECRET_STRIPE_KEY`): the Secrets Store the deploy linked it
    /// to. Linked, never created, so there is nothing to orphan or prune; the
    /// record is what `bynk dev -- --remote` binds by, and is refreshed by
    /// every deploy. It holds a store's name and id and a secret's name, never
    /// a value: the deploy never asks for one.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project without stored secrets writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) stored_secret_links: BTreeMap<String, BTreeMap<String, StoredSecretLink>>,
    /// #3480: each Worker's `AiSearch` instances, keyed by the instance's
    /// binding (`AI_SEARCH_KB`): the account's instance the deploy linked it
    /// to. Linked, never created, so there is nothing to orphan or prune. It
    /// holds an instance's name only, and is refreshed by every deploy.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still
    /// reads and a project without AI Search writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) ai_search_links: BTreeMap<String, BTreeMap<String, AiSearchLink>>,
    /// #3513 A: each Worker's `MtlsCertificate` instances, keyed by the instance's binding
    /// (`MTLS_BACKEND`): the account certificate the deploy linked it to. Linked, never
    /// uploaded, so there is nothing to orphan or prune. It holds a certificate's name and id,
    /// never any key material: the deploy never asks for it. It is what `bynk dev` binds by.
    ///
    /// Additive, `default`ed and omitted when empty, so an older ledger still reads and a
    /// project without mTLS writes the same file.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) mtls_certificate_links: BTreeMap<String, BTreeMap<String, MtlsCertificateLink>>,
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
    /// #4526: per callee Worker, the edge-key slot a rotation is retiring, by the name of the
    /// edge's first slot (`BYNK_CALL_FROM_<S>`): which slot, and the instant after which the runtime
    /// refuses it. It holds no key, only a slot number and a time, so it commits safely (ADR 0195
    /// D1). Set when a rotation's callee is pushed, and cleared when the slot is cleaned up or a
    /// later rotation or revoke replaces it. Kept on the environment, not on the Worker's record,
    /// which every push rebuilds. Additive and `default`ed, so an older ledger still reads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) retiring_edge_keys:
        BTreeMap<String, BTreeMap<String, crate::deploy::edge_slots::Retiring>>,
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
    pub(crate) queues: BTreeSet<String>,
    /// #2579: the subset of [`Environment::queues`] this project only found
    /// already on the account, never created. `--prune` leaves them alone: a
    /// queue another Worker owns must not be deleted, with its undelivered
    /// messages, because this project stopped consuming it. A queue absent from
    /// this set is one the run created, or one an older ledger recorded without
    /// saying — which keeps its old, prunable meaning, so a ledger written
    /// before this field still holds its reused queues as prunable; the
    /// ledger cannot tell them apart. An entry no context declares any more
    /// stays here, harmlessly: `--prune` never reaches it. Additive and
    /// `default`ed, so an older ledger still reads.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub(crate) reused_queues: BTreeSet<String>,
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
    /// #3819: per Worker, each agent class its last successful push hosted, with the
    /// `@jurisdiction` its Durable Object ids were minted under (`"none"` for an agent with
    /// none: the unrestricted namespace). An id minted under another jurisdiction is a
    /// different id for the same name, so a changed value would orphan every instance, and the
    /// next plan compares against this record and asks to confirm the change. Replaced on every
    /// push of the Worker, so a class that is later removed leaves nothing behind. A record of
    /// what was pushed, never a claim about the namespace, which Cloudflare owns. Additive and
    /// `default`ed, so an older ledger still reads (a class it does not list reads as
    /// unrestricted, which is what every agent was before `@jurisdiction`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    agent_jurisdictions: BTreeMap<String, BTreeMap<String, String>>,
    /// #3328: per Worker, the Email Routing `addresses` its last successful push
    /// carried in this environment. Wrangler deletes the rule of an address no
    /// longer listed, so mail to it would bounce: the next deploy compares the
    /// list it is about to push with this record, and a removal is refused unless
    /// confirmed (`--prune`). A record of what was pushed, never a claim about
    /// the account's rules, which Cloudflare owns. Replaced on every push of the
    /// Worker; additive and `default`ed, so an older ledger still reads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    email_addresses: BTreeMap<String, BTreeSet<String>>,
    /// #3565: per Worker, the agent classes whose `transferred` tombstone (class → the script
    /// it was transferred to) a successful push carried in this environment, beside
    /// [`Environment::deleted_classes`] and for the same reason: a record of what was pushed,
    /// never a claim about the namespace. Additive and `default`ed, so an older ledger still
    /// reads.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    transferred_classes: BTreeMap<String, BTreeMap<String, String>>,
    /// #3565: the agent moves under way in this environment, one record per move set
    /// (`design/tracks/agent-context-moves.md` §3, §4.1). A record is written before a move's
    /// first push and removed when its last push lands, so it is empty except mid-move; while
    /// any environment has one, the ledger is written as version 2 ([`LOCK_VERSION_MOVING`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) agent_moves: Vec<AgentMoveRecord>,
}

/// #3565: one move set under way (`design/tracks/agent-context-moves.md` §3.3): every move
/// of a run that shares a source or a receiver, moved as one unit.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AgentMoveRecord {
    /// A random id, also carried in each phase version's deploy facts, so a resume can tell
    /// its own unrecorded push from anything else's.
    #[serde(rename = "move")]
    pub(crate) move_id: String,
    pub(crate) step: MoveStep,
    /// RFC 3339: when the move's first push was about to start.
    pub(crate) started: String,
    /// The bynk version that started the move.
    pub(crate) bynk: String,
    /// The Workers whose push for the current step has landed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) done: Vec<String>,
    pub(crate) moves: Vec<AgentMoveEntry>,
    /// Each Worker of the set → the deployment id it was last seen at, or `"none"` for
    /// a receiver that did not exist before the move.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) deployments: BTreeMap<String, String>,
}

/// #3565: where a move set had got to (`design/tracks/agent-context-moves.md` §3.3).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum MoveStep {
    Receiving,
    Received,
    Committing,
    Committed,
    GoingLive,
}

/// #3565: one context of a move set, with the classes it takes from one script to another.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AgentMoveEntry {
    pub(crate) context: String,
    pub(crate) classes: Vec<String>,
    pub(crate) from_worker: String,
    pub(crate) from_script: String,
    pub(crate) to_worker: String,
    pub(crate) to_script: String,
}

/// #3819: what the ledger records for an agent class whose ids come from the unrestricted
/// namespace. Not a jurisdiction name, so it cannot be mistaken for one.
pub(crate) const UNRESTRICTED_AGENT: &str = "none";

/// P3 (#2342): the `namespace_id` this project allocated for one rate limiter.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct RateLimitNamespace {
    pub(crate) namespace_id: String,
}

/// P3 (#2342): the lowest `namespace_id` the ledger allocates. Low ids are the
/// ones a hand-written Worker on the same account is likeliest to have chosen.
pub(crate) const FIRST_RATELIMIT_NAMESPACE: u64 = 1000;

/// P3 (#2342): how many ids above [`FIRST_RATELIMIT_NAMESPACE`] a seed spreads
/// over.
pub(crate) const RATELIMIT_NAMESPACE_SPAN: u64 = 1_000_000_000;

/// P3 (#2342, review): where the search for `worker`'s rate limiter
/// `binding`'s id in `environment` starts: a hash of the three, into
/// [`FIRST_RATELIMIT_NAMESPACE`]`..+`[`RATELIMIT_NAMESPACE_SPAN`]. Another
/// Bynk project on the same account has a ledger of its own, which this one
/// can't see; but its Workers have other names (a Worker name is unique on an
/// account), so its limiters start elsewhere. A fixed starting point would
/// give every project's first limiter the same id, and the same counters.
pub(crate) fn ratelimit_seed(environment: &str, worker: &str, binding: &str) -> u64 {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("{environment}\0{worker}\0{binding}").as_bytes());
    let n = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
    FIRST_RATELIMIT_NAMESPACE + n % RATELIMIT_NAMESPACE_SPAN
}

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

/// P4a (#2602): one D1 database this project created or adopted, as the
/// account names it, with the id the push binds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct D1Record {
    /// The database's account-facing name, environment-qualified.
    pub(crate) name: String,
    /// Cloudflare's `database_id` for it.
    pub(crate) id: String,
    /// #3820: the jurisdiction it was created in, if any. A database's jurisdiction is fixed when
    /// it is made, so a source that now names another is refused, not recreated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) jurisdiction: Option<String>,
    /// Databases this binding used to have, under a name it no longer
    /// derives. They hold data, so they stay recorded (and are reported as
    /// orphans) until their entry is removed by hand, as R2's are.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) superseded: Vec<D1Past>,
}

/// #3319: the Hyperdrive config of one `Postgres` binding, as the account
/// names it, with the id the push binds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct HyperdriveRecord {
    /// The config's account-facing name; for an adopted config, the name the
    /// deploy found it under, or empty when it did not ask.
    pub(crate) name: String,
    /// Cloudflare's id for the config.
    pub(crate) id: String,
    /// `true` when the project named an existing config (`config_id`) rather
    /// than having this deploy make one. An adopted config is not the
    /// project's to change or delete: it is never updated and never pruned.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) adopted: bool,
    /// Configs this binding used to have, under another id. They stay recorded
    /// (and are reported as orphans) until pruned or removed by hand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) superseded: Vec<HyperdrivePast>,
}

/// A config a binding no longer uses (see [`HyperdriveRecord::superseded`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct HyperdrivePast {
    pub(crate) name: String,
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) adopted: bool,
}

/// #3480: the AI Search instance an `AiSearch` binding was linked to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct AiSearchLink {
    /// The instance's name, as `@instance` wrote it.
    pub(crate) instance: String,
}

/// #3513 A: the account certificate an `MtlsCertificate` binding was linked to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct MtlsCertificateLink {
    /// The certificate's name, as `@certificate` wrote it.
    pub(crate) certificate: String,
    /// The certificate's id, which the push binds.
    pub(crate) certificate_id: String,
}

/// #3301: the Secrets Store secret a `StoredSecret` binding was linked to.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct StoredSecretLink {
    /// The store's name, as `@store` wrote it.
    pub(crate) store: String,
    /// The store's id, which the push binds.
    pub(crate) store_id: String,
    /// The secret's name in the store, as `@secret` wrote it.
    pub(crate) secret: String,
}

/// A database a binding no longer uses (see [`D1Record::superseded`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct D1Past {
    pub(crate) name: String,
    pub(crate) id: String,
}

/// #3479: one Vectorize index of a binding, as the account names it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct VectorizeRecord {
    /// The index's account-facing name, environment-qualified.
    pub(crate) name: String,
    /// `true` when the index was already on the account when this deploy first
    /// met it (named by the project, or found under the derived name) rather
    /// than created by this deploy. An adopted index is checked for drift and
    /// never created, changed or deleted.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) adopted: bool,
    /// Indexes this binding used to have, under another name. They hold
    /// vectors, so they stay recorded (and are reported as orphans) until
    /// pruned or removed by hand.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) superseded: Vec<VectorizePast>,
}

/// An index a binding no longer uses (see [`VectorizeRecord::superseded`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct VectorizePast {
    pub(crate) name: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(crate) adopted: bool,
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
    /// I4 (#1970): the contract each declared foreign caller may call this
    /// Worker at, as of the push that recorded it — actor → service → hash, the
    /// build's `bynk-contracts.json` `foreign` record. The foreign callers are
    /// not Bynk and record nothing themselves, so this is where the ledger
    /// learns of them, beside what the Worker `provides`. Empty (and omitted)
    /// for a Worker with none; additive and `default`ed.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    foreign: BTreeMap<String, BTreeMap<String, String>>,
    /// #2485, read-only since #3112: the per-Worker form of the shapes now
    /// filed by hash in [`DeployLock::contract_shapes`]. A ledger written
    /// before that carries them; [`read_lock`] moves them into the table and
    /// they are never written again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    shapes: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    expects_shapes: BTreeMap<String, BTreeMap<String, String>>,
    /// #3562: the contexts a `[deploy.workers.<key>]` group's Worker held as of
    /// the push that recorded it, in the manifest's order, so a plan, an orphan or a
    /// revert can name the members of a Worker whose key is not any of their
    /// names. Empty (and omitted) for a context on a Worker of its own, and for
    /// a ledger written before groups; additive and `default`ed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    contexts: Vec<String>,
    /// #3562: the Worker's script name on Cloudflare when it is not its key (a
    /// group's `name`), so a report or a `wrangler delete` names the script that
    /// exists. Omitted when the script is named by the key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    script: Option<String>,
    /// #3562 (G5): agent state this Worker's script still holds for a context that moved off
    /// it with `--accept-fresh-agent` (context → its classes): the context now runs on
    /// another script with fresh state, and this script's namespaces keep the old data.
    /// Recorded so an orphan report never calls the script empty. Omitted when none.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    fresh_agents: BTreeMap<String, Vec<String>>,
}

/// #3565 (D5): the ledger is written with the version its content needs, never the one it
/// was read with, so the version cannot drift from whether a move is open.
impl Serialize for DeployLock {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Written<'a> {
            version: u32,
            environments: &'a BTreeMap<String, Environment>,
            #[serde(skip_serializing_if = "Vec::is_empty")]
            contract_shapes: &'a Vec<ContractShape>,
        }
        // Destructured, so a field added to the ledger fails to compile here until it is
        // written too.
        let DeployLock {
            version: _,
            environments,
            contract_shapes,
        } = self;
        Written {
            version: self.written_version(),
            environments,
            contract_shapes,
        }
        .serialize(serializer)
    }
}

impl DeployLock {
    /// #3565: whether any environment has an agent move under way.
    pub(crate) fn has_open_move(&self) -> bool {
        self.environments
            .values()
            .any(|env| !env.agent_moves.is_empty())
    }

    /// #3565 (D5): the version this ledger is written as.
    fn written_version(&self) -> u32 {
        if self.has_open_move() {
            LOCK_VERSION_MOVING
        } else {
            lock_version()
        }
    }

    /// #3112: move any per-Worker shapes an older ledger carries into the
    /// hash-keyed table, so none is lost when the file is next written.
    fn fold_legacy_shapes(&mut self) {
        let mut moved = Vec::new();
        for record in self
            .environments
            .values_mut()
            .flat_map(|env| env.workers.values_mut())
        {
            moved.push((
                std::mem::take(&mut record.shapes),
                std::mem::take(&mut record.expects_shapes),
            ));
        }
        for (shapes, expects_shapes) in moved {
            self.record_shapes(shapes, expects_shapes);
        }
    }

    pub(crate) fn is_deployed(&self, environment: &str, worker: &str) -> bool {
        self.environments
            .get(environment)
            .and_then(|env| env.workers.get(worker))
            .is_some_and(|record| record.deployed)
    }

    /// #3073: the Workers this ledger records as pushed in `environment`, by
    /// logical name, sorted.
    pub(crate) fn deployed_workers(&self, environment: &str) -> Vec<String> {
        self.environments
            .get(environment)
            .map(|env| {
                env.workers
                    .iter()
                    .filter(|(_, record)| record.deployed)
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default()
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

    /// #3328: the recorded addresses of `worker` that `current` no longer lists,
    /// sorted. Nothing is recorded for a first deploy, so it removes nothing.
    pub(crate) fn removed_email_addresses(
        &self,
        environment: &str,
        worker: &str,
        current: &[String],
    ) -> Vec<String> {
        self.environments
            .get(environment)
            .and_then(|env| env.email_addresses.get(worker))
            .map(|recorded| {
                recorded
                    .iter()
                    .filter(|a| !current.contains(a))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// #3328: record the addresses a successful push of `worker` carried,
    /// replacing what was recorded for it.
    pub(crate) fn record_email_addresses(
        &mut self,
        environment: &str,
        worker: &str,
        addresses: BTreeSet<String>,
    ) {
        let env = self
            .environments
            .entry(environment.to_string())
            .or_default();
        if addresses.is_empty() {
            env.email_addresses.remove(worker);
        } else {
            env.email_addresses.insert(worker.to_string(), addresses);
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

    /// #3819: the jurisdiction `class` of `worker` was last pushed under in `environment`:
    /// [`UNRESTRICTED_AGENT`] for an agent with none. `None` when the ledger does not record the
    /// class (a ledger from before `@jurisdiction`, or a class never pushed), which callers read
    /// as unrestricted for a Worker that has been deployed, and as new otherwise.
    pub(crate) fn recorded_agent_jurisdiction(
        &self,
        environment: &str,
        worker: &str,
        class: &str,
    ) -> Option<&str> {
        self.environments
            .get(environment)
            .and_then(|env| env.agent_jurisdictions.get(worker))
            .and_then(|classes| classes.get(class))
            .map(String::as_str)
    }

    /// #3819: whether the ledger holds a record of `worker`'s classes at all, which a push after
    /// `@jurisdiction` writes. A Worker deployed without one was last pushed by an older toolchain.
    pub(crate) fn has_agent_jurisdictions(&self, environment: &str, worker: &str) -> bool {
        self.environments
            .get(environment)
            .is_some_and(|env| env.agent_jurisdictions.contains_key(worker))
    }

    /// #3819: record the classes a successful push of `worker` hosted, and the jurisdiction each
    /// one's ids were minted under, replacing what was recorded for it.
    pub(crate) fn record_agent_jurisdictions(
        &mut self,
        environment: &str,
        worker: &str,
        classes: BTreeMap<String, String>,
    ) {
        let env = self
            .environments
            .entry(environment.to_string())
            .or_default();
        if classes.is_empty() {
            env.agent_jurisdictions.remove(worker);
        } else {
            env.agent_jurisdictions.insert(worker.to_string(), classes);
        }
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
                    foreign: BTreeMap::new(),
                    shapes: BTreeMap::new(),
                    expects_shapes: BTreeMap::new(),
                    contexts: Vec::new(),
                    script: None,
                    fresh_agents: BTreeMap::new(),
                },
            );
    }

    /// #3562: record the script name `worker` was pushed under, when it is not
    /// the key ([`WorkerRecord::script`]). Called right after
    /// [`DeployLock::record_deployed`].
    pub(crate) fn record_script(&mut self, environment: &str, worker: &str, script: &str) {
        if let Some(record) = self
            .environments
            .get_mut(environment)
            .and_then(|env| env.workers.get_mut(worker))
        {
            record.script = (script != worker).then(|| script.to_string());
        }
    }

    /// #3562: the Worker `context` was last pushed in, by the members the ledger
    /// recorded for each group's Worker; a context on its own Worker (or one the
    /// ledger does not know) is the Worker its dashed name gives.
    pub(crate) fn worker_of(&self, environment: &str, context: &str) -> String {
        self.environments
            .get(environment)
            .and_then(|env| {
                env.workers.iter().find(|(_, record)| {
                    record
                        .contexts
                        .iter()
                        .any(|c| c == context || c.replace('.', "-") == context)
                })
            })
            .map_or_else(|| context.replace('.', "-"), |(worker, _)| worker.clone())
    }

    /// #3562: the name `worker`'s script has on Cloudflare: the recorded script
    /// name, else the key.
    pub(crate) fn script_of(&self, environment: &str, worker: &str) -> String {
        self.environments
            .get(environment)
            .and_then(|env| env.workers.get(worker))
            .and_then(|record| record.script.clone())
            .unwrap_or_else(|| worker.to_string())
    }

    /// #3562: record the contexts a group's Worker holds as of this push
    /// ([`WorkerRecord::contexts`]). Called right after
    /// [`DeployLock::record_deployed`]; a Worker not yet recorded is ignored.
    pub(crate) fn record_members(&mut self, environment: &str, worker: &str, members: &[String]) {
        // A context is in one Worker: whatever other records still list the contexts this push
        // holds are stale (a dissolved group, the group it left), and `worker_of` would find
        // them first. A context's own Worker holds just that context.
        let held: Vec<String> = if members.is_empty() {
            vec![worker.replace('-', ".")]
        } else {
            members.to_vec()
        };
        if let Some(env) = self.environments.get_mut(environment) {
            for (other, record) in env.workers.iter_mut() {
                if other != worker {
                    record.contexts.retain(|c| !held.contains(c));
                }
            }
        }
        if let Some(record) = self
            .environments
            .get_mut(environment)
            .and_then(|env| env.workers.get_mut(worker))
        {
            record.contexts = members.to_vec();
        }
    }

    /// #3562 (G5): record that `worker`'s script still holds the agent state of `context`'s
    /// `classes`, which moved off it fresh ([`WorkerRecord::fresh_agents`]).
    pub(crate) fn record_fresh_agents(
        &mut self,
        environment: &str,
        worker: &str,
        context: &str,
        classes: &[String],
    ) {
        if let Some(record) = self
            .environments
            .get_mut(environment)
            .and_then(|env| env.workers.get_mut(worker))
        {
            record
                .fresh_agents
                .insert(context.to_string(), classes.to_vec());
        }
    }

    /// #3562: the contexts a Worker was last pushed with: empty for a context's
    /// own Worker, an unrecorded one, and a ledger from before groups.
    #[cfg(test)]
    pub(crate) fn members_of(&self, environment: &str, worker: &str) -> &[String] {
        self.environments
            .get(environment)
            .and_then(|env| env.workers.get(worker))
            .map_or(&[], |record| record.contexts.as_slice())
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

    /// #2485: record the structured form behind the contracts a Worker provides
    /// and expects as of this push, filed by hash ([`DeployLock::contract_shapes`]).
    /// Called right after [`DeployLock::record_deployed`]. Entries no Worker's
    /// record references any more are kept up to [`RECENT_SHAPES`], oldest
    /// dropped first, so a `bynk revert` can still classify the contracts it
    /// restores (#3112).
    pub(crate) fn record_shapes(
        &mut self,
        shapes: BTreeMap<String, String>,
        expects_shapes: BTreeMap<String, BTreeMap<String, String>>,
    ) {
        let texts = shapes
            .into_values()
            .chain(expects_shapes.into_values().flat_map(BTreeMap::into_values));
        for text in texts {
            let Some(shape) = ServiceShape::from_text(&text) else {
                continue;
            };
            let hash = shape.hash();
            // Already filed: leave it where it is, so a push that changes
            // nothing leaves the committed lock byte-identical.
            if self.contract_shapes.iter().any(|e| e.hash == hash) {
                continue;
            }
            self.contract_shapes
                .push(ContractShape { hash, shape: text });
        }
        let live: BTreeSet<String> = self
            .environments
            .values()
            .flat_map(|env| env.workers.values())
            .flat_map(|w| {
                w.contracts.iter().flat_map(BTreeMap::values).chain(
                    w.expects
                        .iter()
                        .flat_map(|e| e.values().flat_map(BTreeMap::values)),
                )
            })
            .cloned()
            .collect();
        let mut spare = self
            .contract_shapes
            .iter()
            .filter(|e| !live.contains(&e.hash))
            .count();
        self.contract_shapes.retain(|e| {
            if live.contains(&e.hash) {
                return true;
            }
            let keep = spare <= RECENT_SHAPES;
            spare -= 1;
            keep
        });
    }

    /// The shape filed under `hash`, if it hashes to it.
    fn shape_for(&self, hash: &str) -> Option<ServiceShape> {
        let entry = self.contract_shapes.iter().find(|e| e.hash == hash)?;
        ServiceShape::from_text_checked(&entry.shape, hash)
    }

    /// #2485: the shape `worker` provides `service` at, if the ledger holds one
    /// for the hash recorded for it (and it hashes to it).
    pub(crate) fn live_shape(
        &self,
        environment: &str,
        worker: &str,
        service: &str,
    ) -> Option<ServiceShape> {
        let record = self.environments.get(environment)?.workers.get(worker)?;
        self.shape_for(record.contracts.as_ref()?.get(service)?)
    }

    /// #2485: the shape `caller` was compiled against for `dependency.service`,
    /// under the same trust rule as [`DeployLock::live_shape`].
    pub(crate) fn expected_shape(
        &self,
        environment: &str,
        caller: &str,
        dependency: &str,
        service: &str,
    ) -> Option<ServiceShape> {
        let record = self.environments.get(environment)?.workers.get(caller)?;
        self.shape_for(record.expects.as_ref()?.get(dependency)?.get(service)?)
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
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Orphans {
    pub(crate) kv: Vec<OrphanKv>,
    /// P5 (#2323): recorded R2 buckets the build no longer declares. Reported
    /// only: a bucket holds data, so `--prune` never deletes one (and R2
    /// refuses to delete a non-empty one anyway).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) r2: Vec<OrphanR2>,
    /// #3479: recorded Vectorize indexes the build no longer declares. An
    /// index holds vectors, so `--prune` deletes one only behind its
    /// confirmation (as for KV); an adopted one is only ever reported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) vectorize: Vec<OrphanVectorize>,
    /// P4a (#2602): recorded D1 databases the build no longer declares.
    /// Reported only, as buckets are: a database holds data.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) d1: Vec<OrphanD1>,
    /// #3319: recorded Hyperdrive configs the build no longer declares. A
    /// config holds no data, so `--prune` deletes one the deploy made; an
    /// adopted one is only ever reported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) hyperdrive: Vec<OrphanHyperdrive>,
    pub(crate) workers: Vec<String>,
    /// #3562: the contexts of each orphaned Worker the ledger recorded as a
    /// group's, by Worker name, so the report names a removed group's members.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) worker_contexts: BTreeMap<String, Vec<String>>,
    /// #3562: the script name of each orphaned Worker whose script is not named
    /// by its key (a renamed group), by key.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) worker_scripts: BTreeMap<String, String>,
    /// #3562 (G5): the agent state each orphaned Worker's script still holds (context →
    /// classes), from a move accepted with `--accept-fresh-agent`, by Worker.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub(crate) worker_agent_state: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    pub(crate) queues: Vec<String>,
    /// #4601: the project's own Workers each orphaned queue's prune detaches first, by ledger
    /// name, read from the account just before `--prune` asks ([`super::kind::vet_orphan_queues`]).
    /// Empty in the plan: the plan never calls Cloudflare.
    #[serde(skip)]
    pub(crate) queue_detaching: BTreeMap<String, Vec<String>>,
}

impl Orphans {
    /// Whether `--prune` would actually delete anything. **Not** the same as
    /// checking whether every field is empty (review, #840): `workers`
    /// orphans are report-only (DECISION C — never `wrangler delete`), so a
    /// project whose only orphan is an unprunable Worker must not trigger
    /// `confirm_prune`'s prompt at all — `Delete 0 resource(s)?` is a bug,
    /// not a valid state.
    pub(crate) fn has_prunable(&self) -> bool {
        kinds_have_prunable(self)
    }

    /// These orphans as the account names them under `environment`, for the
    /// plan to show (#2028): a queue by the [`env_qualify`]d name `--prune`
    /// deletes, a Worker by its [`deployed_worker_name`]. A KV orphan keeps
    /// its Worker's ledger key: its line names the ledger entry, not the
    /// namespace's title ([`kv_title`]), which a namespace made before #2037
    /// may not match.
    /// An orphaned Worker as the account names it: its script name when that is
    /// not its key, in `environment`.
    fn shown_worker(&self, environment: &str, worker: &str) -> String {
        let script = self
            .worker_scripts
            .get(worker)
            .map_or(worker, String::as_str);
        deployed_worker_name(environment, script)
    }

    pub(crate) fn as_named_in(&self, environment: &str) -> Orphans {
        let mut shown = Orphans {
            kv: Vec::new(),
            r2: Vec::new(),
            d1: Vec::new(),
            hyperdrive: Vec::new(),
            vectorize: Vec::new(),
            workers: self
                .workers
                .iter()
                .map(|worker| self.shown_worker(environment, worker))
                .collect(),
            worker_contexts: self
                .worker_contexts
                .iter()
                .map(|(worker, contexts)| {
                    (self.shown_worker(environment, worker), contexts.clone())
                })
                .collect(),
            worker_scripts: BTreeMap::new(),
            queue_detaching: self.queue_detaching.clone(),
            worker_agent_state: self
                .worker_agent_state
                .iter()
                .map(|(worker, state)| (self.shown_worker(environment, worker), state.clone()))
                .collect(),
            queues: Vec::new(),
        };
        show_kind_orphans(self, environment, &mut shown);
        shown
    }
}

impl Environment {
    /// #4601: the script name, on Cloudflare in `environment`, of every Worker this ledger records
    /// for it: the project's own Workers, the only queue consumers `--prune` detaches.
    pub(crate) fn worker_scripts(&self, environment: &str) -> BTreeSet<String> {
        self.workers
            .iter()
            .map(|(key, record)| env_qualify(environment, record.script.as_deref().unwrap_or(key)))
            .collect()
    }
}

/// The orphan diff: ledger vs. the current build's full declared resource
/// set, regardless of `--context` — `resources` already spans the *whole*
/// project (`project_resources`, called from `run()` over `available` — every
/// worker `workers::discover_workers` found — not the `--context`-narrowed
/// `order`/`selected`), so `resources.keys()` alone is the full live-worker
/// set and this reuses data `run()` already has rather than reading anything
/// new. No live Cloudflare call: the report half of reconciliation costs
/// nothing and needs no auth.
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
    let mut orphans = Orphans {
        r2: Vec::new(),
        vectorize: Vec::new(),
        d1: Vec::new(),
        hyperdrive: Vec::new(),
        kv: Vec::new(),
        workers: env
            .workers
            .keys()
            .filter(|worker| !resources.contains_key(worker.as_str()))
            .cloned()
            .collect(),
        worker_contexts: env
            .workers
            .iter()
            .filter(|(worker, record)| {
                !resources.contains_key(worker.as_str()) && !record.contexts.is_empty()
            })
            .map(|(worker, record)| (worker.clone(), record.contexts.clone()))
            .collect(),
        worker_scripts: env
            .workers
            .iter()
            .filter(|(worker, _)| !resources.contains_key(worker.as_str()))
            .filter_map(|(worker, record)| Some((worker.clone(), record.script.clone()?)))
            .collect(),
        worker_agent_state: env
            .workers
            .iter()
            .filter(|(worker, record)| {
                !resources.contains_key(worker.as_str()) && !record.fresh_agents.is_empty()
            })
            .map(|(worker, record)| (worker.clone(), record.fresh_agents.clone()))
            .collect(),
        queues: Vec::new(),
        queue_detaching: BTreeMap::new(),
    };
    // 1D.1 (#3929): each converted kind's orphans.
    find_kind_orphans(&OrphanCx { env, resources }, &mut orphans);
    orphans
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
    let mut lock: DeployLock = toml::from_str(&text).map_err(|e| {
        format!(
            "deploy ledger `{}` is corrupt ({e}) — restore it from version control",
            path.display()
        )
    })?;
    if lock.version == LOCK_VERSION_MOVING && !lock.has_open_move() {
        return Err(format!(
            "deploy ledger `{}` says version {LOCK_VERSION_MOVING}, which only a ledger with an agent move under way has, but it records no move — it is corrupt; restore it from version control",
            path.display()
        ));
    }
    if lock.version != lock_version() && lock.version != LOCK_VERSION_MOVING {
        return Err(format!("unsupported deploy lock version {}", lock.version));
    }
    lock.fold_legacy_shapes();
    Ok(lock)
}

/// Distinguishes concurrent temp ledgers written by the same process.
static TMP_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub(crate) fn write_lock(path: &Path, lock: &DeployLock) -> Result<(), String> {
    #[cfg(test)]
    write_watch::observe(path, lock)?;
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

/// 1D.2 (#4001): a test's view of every [`write_lock`] on its thread, which may also refuse
/// one, so the apply fixtures (`apply_tests.rs`) can pin the ledger after each write and the
/// message when a write fails. Nothing watches outside a test.
#[cfg(test)]
pub(crate) mod write_watch {
    use super::*;
    use std::cell::RefCell;

    /// Called with each ledger about to be written; an `Err` is the write's failure.
    pub(crate) type Watch = Box<dyn FnMut(&Path, &DeployLock) -> Result<(), String>>;

    thread_local! {
        static WATCH: RefCell<Option<Watch>> = const { RefCell::new(None) };
    }

    /// Run `f` with `watch` seeing every ledger write on this thread.
    pub(crate) fn watching<T>(watch: Watch, f: impl FnOnce() -> T) -> T {
        WATCH.with(|w| *w.borrow_mut() = Some(watch));
        let result = f();
        WATCH.with(|w| *w.borrow_mut() = None);
        result
    }

    pub(super) fn observe(path: &Path, lock: &DeployLock) -> Result<(), String> {
        WATCH.with(|w| match w.borrow_mut().as_mut() {
            Some(watch) => watch(path, lock),
            None => Ok(()),
        })
    }
}

/// #3513 A: bind a served Worker's `[[mtls_certificates]]` by the ids the last deploy of
/// `environment` recorded, for `bynk dev`. An mTLS binding has no local simulation, so the
/// development copy runs it on Cloudflare and needs the certificate's real id, which only a deploy
/// has resolved. Every binding with a recorded link is bound; the message names each that has
/// none (`Err`), for the caller to report and carry on: a call through it then fails, naming it.
/// `Ok(false)` when the Worker declares no certificate.
pub fn materialise_mtls_for_dev(
    project_root: &Path,
    worker: &str,
    config: &Path,
    environment: &str,
) -> Result<bool, String> {
    let text = std::fs::read_to_string(config).map_err(|e| e.to_string())?;
    let bindings = bynk_emit::emitter::wrangler::mtls_certificate_bindings(&text)?;
    if bindings.is_empty() {
        return Ok(false);
    }
    let lock = read_lock(&project_root.join(LOCK_FILE))?;
    let mut ids = BTreeMap::new();
    let mut unlinked = Vec::new();
    for binding in bindings {
        match lock.recorded::<MtlsCertificate>(environment, worker, &binding) {
            Some(link) => {
                ids.insert(binding, link.certificate_id.clone());
            }
            None => unlinked.push(binding),
        }
    }
    if !ids.is_empty() && !materialise_mtls_certificates(config, &ids) {
        return Err("could not write generated configuration".into());
    }
    if unlinked.is_empty() {
        return Ok(true);
    }
    // An unlinked entry is dropped from the development copy, not left to wrangler: marked
    // `remote = true` with a name where an id belongs, it might fail the whole startup.
    let text = std::fs::read_to_string(config).map_err(|e| e.to_string())?;
    let kept = bynk_emit::emitter::wrangler::drop_mtls_certificates(&text, &unlinked)?;
    if kept != text {
        std::fs::write(config, kept).map_err(|e| e.to_string())?;
    }
    Err(format!(
        "mTLS certificate binding {} of `{worker}` has not been linked under environment `{environment}`, so it is left unbound and a call through it fails; run `bynk deploy --env {environment}` first, which links the certificate by name",
        unlinked
            .iter()
            .map(|b| format!("`{b}`"))
            .collect::<Vec<_>>()
            .join(", ")
    ))
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
    // P4a (#2602): each `D1` database must have been created by a deploy, and
    // is then bound by the name and id that environment's deploy recorded.
    let databases = bynk_emit::emitter::wrangler::d1_database_entries(&text)?;
    // #3301: each `StoredSecret` must have been linked by a deploy, and is
    // then bound by the store id that deploy recorded.
    let stored = bynk_emit::emitter::wrangler::stored_secret_entries(&text)?;
    // #3319: each `Postgres` must have had its Hyperdrive config made or
    // adopted by a deploy, and is then bound by the id that deploy recorded.
    let hyperdrive = bynk_emit::emitter::wrangler::hyperdrive_entries(&text)?;
    if needed.is_empty()
        && buckets.is_empty()
        && limiters.is_empty()
        && databases.is_empty()
        && stored.is_empty()
        && hyperdrive.is_empty()
    {
        return Ok(false);
    }
    let lock = read_lock(&project_root.join(LOCK_FILE))?;
    let mut limiter_ids = BTreeMap::new();
    for binding in limiters {
        let Some(id) = lock
            .recorded::<RateLimit>(environment, worker, &binding)
            .map(|r| r.namespace_id.as_str())
        else {
            return Err(format!(
                "remote rate limiter `{binding}` for `{worker}` has not been provisioned under environment `{environment}`; run `bynk deploy --env {environment}` first"
            ));
        };
        limiter_ids.insert(binding, id.to_string());
    }
    let mut names = BTreeMap::new();
    for (binding, name, jurisdiction) in &buckets {
        let Some(recorded) = lock.recorded::<R2>(environment, worker, binding) else {
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
    let mut d1 = BTreeMap::new();
    for (binding, _) in &databases {
        let Some(recorded) = lock.recorded::<D1>(environment, worker, binding) else {
            return Err(format!(
                "remote D1 database `{binding}` for `{worker}` has not been provisioned under environment `{environment}`; run `bynk deploy --env {environment}` first"
            ));
        };
        d1.insert(
            binding.clone(),
            (recorded.name.clone(), recorded.id.clone()),
        );
    }
    let mut store_ids = BTreeMap::new();
    for (binding, _, _) in &stored {
        let Some(link) = lock.recorded::<StoredSecret>(environment, worker, binding) else {
            return Err(format!(
                "remote Secrets Store secret `{binding}` for `{worker}` has not been linked under environment `{environment}`; run `bynk deploy --env {environment}` first"
            ));
        };
        store_ids.insert(binding.clone(), link.store_id.clone());
    }
    let mut hyperdrive_ids = BTreeMap::new();
    for (binding, _) in &hyperdrive {
        let Some(recorded) = lock.recorded::<Hyperdrive>(environment, worker, binding) else {
            return Err(format!(
                "remote Hyperdrive config `{binding}` for `{worker}` has not been provisioned under environment `{environment}`; run `bynk deploy --env {environment}` first"
            ));
        };
        hyperdrive_ids.insert(binding.clone(), recorded.id.clone());
    }
    let mut changed = false;
    if !hyperdrive_ids.is_empty() {
        if !materialise_hyperdrive_ids(config, &hyperdrive_ids) {
            return Err("could not write generated configuration".into());
        }
        changed = true;
    }
    if !store_ids.is_empty() {
        if !materialise_stored_secret_stores(config, &store_ids) {
            return Err("could not write generated configuration".into());
        }
        changed = true;
    }
    if !d1.is_empty() {
        if !materialise_d1_databases(config, &d1) {
            return Err("could not write generated configuration".into());
        }
        changed = true;
    }
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
        let Some(id) = lock
            .recorded::<Kv>(environment, worker, &binding)
            .map(|kv| kv.id.as_str())
        else {
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
    // 1D.1 (#3929): each converted kind's, in the list's order (ruled for 1D.1c), first.
    let kinds = announce_prunable_kinds(orphans, environment);
    confirm_deletion_prompt(yes, kinds)
}

/// The `--yes`-or-ask tail of [`confirm_prune`], shared with the Email Routing
/// removal gate (#3328): `count` things are about to be deleted.
pub(crate) fn confirm_deletion_prompt(yes: bool, count: usize) -> bool {
    if yes {
        return true;
    }
    if !requires_interactive_confirmation(yes, io::stdin().is_terminal()) {
        emsg!("bynk: error: refusing to prune in a non-interactive session without --yes");
        return false;
    }
    eprint!("Delete {count} resource(s)? [y/N] ");
    let _ = io::stderr().flush();
    let mut answer = String::new();
    io::stdin().read_line(&mut answer).is_ok()
        && matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}

/// Delete every orphan `confirm_prune` just named: each converted kind's (1D.1),
/// then each queue — never a
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
    tools: Tools<'_>,
    writer: &mut LedgerWriter<'_>,
    environment: &str,
    orphans: &Orphans,
    lives: Lives<'_>,
) -> Result<(), DeployFailure> {
    // 1D.1 (#3929): each converted kind's, in the list's order (ruled for 1D.1c), first.
    prune_kind_orphans(tools, writer, environment, orphans, lives)?;
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
        lock.record::<Kv>(
            "default",
            worker,
            binding,
            KvNamespace::of(&format!("{binding}-id")),
        );
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
        QueueTable::created(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            queue,
        );
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

    /// P3 (#2342): an id starts at its limiter's own seed and probes upward
    /// past those the ledger holds, in any environment; a recorded id is
    /// kept; a forgotten one is free again.
    #[test]
    fn rate_limiter_ids_are_allocated_across_environments() {
        let mut lock = DeployLock::default();
        let api = ratelimit_seed("default", "api", "RATELIMIT_A");
        assert_eq!(
            allocate_in(&mut lock, "default", "api", "RATELIMIT_A"),
            (api.to_string(), true)
        );
        assert_eq!(
            allocate_in(&mut lock, "default", "api", "RATELIMIT_A"),
            (api.to_string(), false)
        );
        let staging = allocate_in(&mut lock, "staging", "api", "RATELIMIT_A").0;
        assert_ne!(staging, api.to_string(), "each environment its own");
        // A seed another limiter in the ledger already holds probes upward.
        let mut crowded = DeployLock::default();
        let seed = ratelimit_seed("default", "web", "RATELIMIT_B");
        crowded
            .environments
            .entry("staging".into())
            .or_default()
            .ratelimit_instances
            .insert(
                "other".into(),
                BTreeMap::from([(
                    "RATELIMIT_X".into(),
                    RateLimitNamespace {
                        namespace_id: seed.to_string(),
                    },
                )]),
            );
        assert_eq!(
            allocate_in(&mut crowded, "default", "web", "RATELIMIT_B"),
            ((seed + 1).to_string(), true)
        );
        // Forgotten after the push once the source declares none: twice is harmless.
        let none = Resources::default();
        for _ in 0..2 {
            RateLimit::after_push(lock.environments.get_mut("default").unwrap(), "api", &none);
        }
        assert!(
            !lock.environments["default"]
                .ratelimit_instances
                .contains_key("api")
        );
    }

    /// P3 (#2342, review): two projects on one account have separate ledgers,
    /// so neither can see the other's ids. Their Workers' names differ (a
    /// Worker name is unique on an account), and so do the seeds their
    /// limiters start from: neither starts where the other did.
    #[test]
    fn two_projects_ledgers_do_not_start_their_limiters_at_one_id() {
        let mut shop = DeployLock::default();
        let mut registry = DeployLock::default();
        let a = allocate_in(&mut shop, "default", "shop-cart", "RATELIMIT_LOGIN").0;
        let b = allocate_in(
            &mut registry,
            "default",
            "registry-publish",
            "RATELIMIT_LOGIN",
        )
        .0;
        assert_ne!(a, b);
        for id in [&a, &b] {
            let n: u64 = id.parse().unwrap();
            assert!(
                (FIRST_RATELIMIT_NAMESPACE..FIRST_RATELIMIT_NAMESPACE + RATELIMIT_NAMESPACE_SPAN)
                    .contains(&n)
            );
        }
    }

    #[test]
    fn lock_round_trip_is_environment_keyed() {
        let lock = DeployLock {
            version: 1,
            environments: BTreeMap::from([(
                "default".into(),
                Environment {
                    retiring_edge_keys: BTreeMap::new(),
                    deleted_classes: BTreeMap::new(),
                    renamed_classes: BTreeMap::new(),
                    agent_jurisdictions: BTreeMap::new(),
                    email_addresses: BTreeMap::new(),
                    transferred_classes: BTreeMap::new(),
                    agent_moves: Vec::new(),
                    kv: BTreeMap::from([("api".into(), KvNamespace { id: "abc".into() })]),
                    kv_instances: BTreeMap::from([(
                        "api".into(),
                        BTreeMap::from([("KV_SESSIONS".into(), KvNamespace { id: "s".into() })]),
                    )]),
                    vectorize_instances: BTreeMap::new(),
                    container_images: BTreeMap::new(),
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
                    d1_instances: BTreeMap::from([(
                        "api".into(),
                        BTreeMap::from([(
                            "D1_ORDERS".into(),
                            D1Record {
                                name: "api-orders".into(),
                                id: "0d1d0000-0000-4000-8000-000000000001".into(),
                                jurisdiction: None,
                                superseded: vec![D1Past {
                                    name: "api-orders-old".into(),
                                    id: "0d1d0000-0000-4000-8000-000000000000".into(),
                                }],
                            },
                        )]),
                    )]),
                    hyperdrive_instances: BTreeMap::new(),
                    ai_search_links: BTreeMap::new(),
                    mtls_certificate_links: BTreeMap::new(),
                    stored_secret_links: BTreeMap::from([(
                        "api".into(),
                        BTreeMap::from([(
                            "SECRET_STRIPE_KEY".into(),
                            StoredSecretLink {
                                store: "payments".into(),
                                store_id: "0b5e1c2a3d4f45678901234567890abc".into(),
                                secret: "stripe-live".into(),
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
                            foreign: BTreeMap::from([(
                                "Legacy".into(),
                                BTreeMap::from([("ping".into(), "4082ce9e2b74e919".into())]),
                            )]),
                            shapes: BTreeMap::new(),
                            expects_shapes: BTreeMap::new(),
                            contexts: Vec::new(),
                            script: None,
                            fresh_agents: BTreeMap::new(),
                        },
                    )]),
                    queues: BTreeSet::from(["intake".to_string()]),
                    reused_queues: BTreeSet::new(),
                },
            )]),
            contract_shapes: Vec::new(),
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
        assert_eq!(
            lock.recorded::<Kv>("default", "api", "KV")
                .map(|kv| kv.id.as_str()),
            Some("abc")
        );
        assert!(!lock.is_deployed("default", "api"));
        assert!(lock.recorded::<Queue>("default", "", "intake").is_none());
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
        QueueTable::created(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "job-intake",
        );
        QueueTable::created(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "job-retry",
        );
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

    /// #3072: the lock does not list a Worker's static assets (their hashed
    /// names change with every page edit and nothing reads them). A lock that
    /// still carries an `assets` line reads, and the next write omits it.
    #[test]
    fn an_old_locks_assets_line_is_read_and_not_written_back() {
        let old = "version = 1\n\n[environments.default.workers.board]\ndeployed = true\nassets = [\"/_assets/board.1a2b.css\"]\n";
        let lock: DeployLock = toml::from_str(old).unwrap();
        assert!(lock.is_deployed("default", "board"));
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(!text.contains("assets"), "{text}");
        assert!(!text.contains("/_assets/"), "{text}");
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

    /// #2485: the structure behind each hash round-trips, is omitted when
    /// empty (so a ledger written without shapes is byte-identical), and is
    /// trusted only while it hashes to the hash recorded beside it.
    #[test]
    fn contract_shapes_round_trip_and_are_trusted_only_with_their_hash() {
        use bynk_check::contract_compat::{ServiceShape, Shape};
        let shape = ServiceShape {
            name: "charge".into(),
            params: vec![("order".into(), Shape::Base("Int".into()))],
            ret: Shape::Base("Bool".into()),
        };
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_deployed(
            "default",
            "shop-payment",
            Some(BTreeMap::from([("charge".to_string(), shape.hash())])),
        );
        assert!(
            !toml::to_string_pretty(&lock).unwrap().contains("shapes"),
            "no shapes recorded, none written"
        );
        lock.record_shapes(
            BTreeMap::from([("charge".to_string(), shape.to_text())]),
            BTreeMap::new(),
        );
        let read: DeployLock = toml::from_str(&toml::to_string_pretty(&lock).unwrap()).unwrap();
        assert_eq!(
            read.live_shape("default", "shop-payment", "charge"),
            Some(shape.clone())
        );
        assert_eq!(read.live_shape("default", "shop-payment", "other"), None);
        // A push that changes the contract without a fresh shape leaves a
        // stale one, which no longer hashes to the record and so is ignored.
        lock.record_deployed(
            "default",
            "shop-payment",
            Some(BTreeMap::from([("charge".to_string(), "0".repeat(16))])),
        );
        assert_eq!(lock.live_shape("default", "shop-payment", "charge"), None);
    }

    /// #3112: a revert replaces the Worker's contract hashes with the target's
    /// and drops nothing it can still classify: the target's shape is found by
    /// hash, from the push that recorded it.
    #[test]
    fn a_reverted_contract_still_resolves_its_shape_by_hash() {
        use bynk_check::contract_compat::{ServiceShape, Shape};
        let shape = |ret: &str| ServiceShape {
            name: "charge".into(),
            params: vec![("order".into(), Shape::Base("Int".into()))],
            ret: Shape::Base(ret.into()),
        };
        let (v1, v2) = (shape("Bool"), shape("Int"));
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        for s in [&v1, &v2] {
            lock.record_deployed(
                "default",
                "pay",
                Some(BTreeMap::from([("charge".to_string(), s.hash())])),
            );
            lock.record_shapes(
                BTreeMap::from([("charge".to_string(), s.to_text())]),
                BTreeMap::new(),
            );
        }
        assert_eq!(lock.live_shape("default", "pay", "charge"), Some(v2));
        // The revert: v1's hash again, no shape handed over.
        lock.record_deployed(
            "default",
            "pay",
            Some(BTreeMap::from([("charge".to_string(), v1.hash())])),
        );
        assert_eq!(lock.live_shape("default", "pay", "charge"), Some(v1));
        let read: DeployLock = toml::from_str(&toml::to_string_pretty(&lock).unwrap()).unwrap();
        assert_eq!(read, lock, "the table survives a write and read");
    }

    /// #3112: recording what is already filed changes nothing (the lock is
    /// committed, so a no-op push must not reorder it), and a target whose
    /// shape was pruned resolves to no shape, so its skew stays unclassified.
    #[test]
    fn recording_a_known_shape_changes_nothing_and_a_pruned_one_resolves_to_none() {
        use bynk_check::contract_compat::{ServiceShape, Shape};
        let shape = |n: usize| ServiceShape {
            name: "charge".into(),
            params: vec![(format!("p{n}"), Shape::Base("Int".into()))],
            ret: Shape::Base("Bool".into()),
        };
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        let push = |lock: &mut DeployLock, s: &ServiceShape| {
            lock.record_deployed(
                "default",
                "pay",
                Some(BTreeMap::from([("charge".to_string(), s.hash())])),
            );
            lock.record_shapes(
                BTreeMap::from([("charge".to_string(), s.to_text())]),
                BTreeMap::new(),
            );
        };
        push(&mut lock, &shape(0));
        push(&mut lock, &shape(1));
        let before = toml::to_string_pretty(&lock).unwrap();
        // Re-record the older, now unreferenced shape alongside the live one.
        lock.record_shapes(
            BTreeMap::from([
                ("a".to_string(), shape(0).to_text()),
                ("b".to_string(), shape(1).to_text()),
            ]),
            BTreeMap::new(),
        );
        assert_eq!(toml::to_string_pretty(&lock).unwrap(), before);
        // Enough later pushes prune shape 0: a revert to it classifies nothing.
        for n in 2..(RECENT_SHAPES + 4) {
            push(&mut lock, &shape(n));
        }
        lock.record_deployed(
            "default",
            "pay",
            Some(BTreeMap::from([("charge".to_string(), shape(0).hash())])),
        );
        assert_eq!(lock.live_shape("default", "pay", "charge"), None);
    }

    /// #3112: a ledger written with per-Worker shapes still reads, and its
    /// shapes move into the table rather than being lost on the next write.
    #[test]
    fn per_worker_shapes_from_an_older_ledger_are_folded_into_the_table() {
        use bynk_check::contract_compat::{ServiceShape, Shape};
        let shape = ServiceShape {
            name: "charge".into(),
            params: vec![("order".into(), Shape::Base("Int".into()))],
            ret: Shape::Base("Bool".into()),
        };
        let dir = std::env::temp_dir().join(format!("bynk-legacy-shapes-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bynk.deploy.lock");
        std::fs::write(
            &path,
            format!(
                "version = 1\n[environments.default.workers.pay]\ndeployed = true\n\
                 [environments.default.workers.pay.contracts]\ncharge = '{}'\n\
                 [environments.default.workers.pay.shapes]\ncharge = '{}'\n",
                shape.hash(),
                shape.to_text()
            ),
        )
        .unwrap();
        let lock = read_lock(&path).unwrap();
        assert_eq!(lock.live_shape("default", "pay", "charge"), Some(shape));
        let written = toml::to_string_pretty(&lock).unwrap();
        assert!(
            !written.contains(".shapes]"),
            "not written per Worker again"
        );
        assert!(written.contains("contract_shapes"));
        std::fs::remove_dir_all(&dir).ok();
    }

    /// #3112: shapes no record references are kept to a bound, oldest first;
    /// the live ones are never dropped.
    #[test]
    fn unreferenced_shapes_are_pruned_to_a_bound_and_live_ones_stay() {
        use bynk_check::contract_compat::{ServiceShape, Shape};
        let shape = |n: usize| ServiceShape {
            name: "charge".into(),
            params: vec![(format!("p{n}"), Shape::Base("Int".into()))],
            ret: Shape::Base("Bool".into()),
        };
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        for n in 0..(RECENT_SHAPES + 5) {
            let s = shape(n);
            lock.record_deployed(
                "default",
                "pay",
                Some(BTreeMap::from([("charge".to_string(), s.hash())])),
            );
            lock.record_shapes(
                BTreeMap::from([("charge".to_string(), s.to_text())]),
                BTreeMap::new(),
            );
        }
        // The live one plus the most recent RECENT_SHAPES others.
        assert_eq!(lock.contract_shapes.len(), RECENT_SHAPES + 1);
        let newest = shape(RECENT_SHAPES + 4);
        assert_eq!(lock.live_shape("default", "pay", "charge"), Some(newest));
        let oldest = shape(0).hash();
        assert!(lock.shape_for(&oldest).is_none(), "the oldest was pruned");
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
        QueueTable::created(
            lock.environments
                .entry(String::from("staging"))
                .or_default(),
            "jobs",
        );
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
        assert!(lock.recorded::<Queue>("default", "", "jobs").is_none());
        assert_eq!(
            lock.recorded::<Kv>("default", "api", "KV")
                .map(|kv| kv.id.as_str()),
            None
        );

        assert!(lock.is_deployed("staging", "api"));
        assert!(lock.recorded::<Queue>("staging", "", "jobs").is_some());
        assert_eq!(
            lock.recorded::<Kv>("staging", "api", "KV")
                .map(|kv| kv.id.as_str()),
            Some("kv-staging")
        );
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
        allocate_in(&mut lock, "default", "shop", "RATELIMIT_LOGIN");
        std::fs::write(dir.join(LOCK_FILE), toml::to_string_pretty(&lock).unwrap()).unwrap();
        assert!(materialise_deploy_state(&dir, "shop", &config, "default").unwrap());
        let text = std::fs::read_to_string(&config).unwrap();
        let id = ratelimit_seed("default", "shop", "RATELIMIT_LOGIN");
        assert!(
            text.contains(&format!(
                "namespace_id = \"{id}\" # set at deploy time\nsimple = {{ limit = 20, period = 60 }}"
            )),
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
        lock.record::<Kv>(
            "default",
            "shop-cart",
            "KV_SESSIONS",
            KvNamespace::of("sess"),
        );
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

        lock.record::<Kv>(
            "default",
            "shop-cart",
            "KV_PRICE_CACHE",
            KvNamespace::of("price"),
        );
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

    /// #3513 A: `bynk dev` binds an mTLS certificate by the id the last deploy recorded for the
    /// environment, since the binding runs on Cloudflare and takes a real id. One no deploy has
    /// linked is reported naming it, and the certificate's *name* is never left in
    /// `certificate_id` for a linked one.
    #[test]
    fn materialise_mtls_for_dev_binds_a_linked_certificate_by_its_recorded_id() {
        let dir = scratch_lock_dir("materialise-mtls");
        let config = dir.join("wrangler.toml");
        let text = "[[mtls_certificates]]\nbinding = \"MTLS_BACKEND\"\ncertificate_id = \"acme-client\"\n\n[[mtls_certificates]]\nbinding = \"MTLS_OTHER\"\ncertificate_id = \"ops\"\n";
        std::fs::write(&config, text).unwrap();
        // A Worker with no certificate is left alone.
        let plain = dir.join("plain.toml");
        std::fs::write(&plain, "name = \"x\"\n").unwrap();
        assert_eq!(
            materialise_mtls_for_dev(&dir, "shop", &plain, "default"),
            Ok(false)
        );
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record::<MtlsCertificate>(
            "default",
            "shop",
            "MTLS_BACKEND",
            MtlsCertificateLink {
                certificate: "acme-client".into(),
                certificate_id: "c-123".into(),
            },
        );
        std::fs::write(dir.join(LOCK_FILE), toml::to_string_pretty(&lock).unwrap()).unwrap();
        let original = std::fs::read_to_string(&config).unwrap();
        // The linked binding is bound; the other is named and dropped from the development copy.
        let err = materialise_mtls_for_dev(&dir, "shop", &config, "default")
            .expect_err("an unlinked certificate is reported");
        assert!(
            err.contains("`MTLS_OTHER`") && !err.contains("`MTLS_BACKEND`"),
            "{err}"
        );
        assert!(err.contains("bynk deploy --env default"), "{err}");
        let out = std::fs::read_to_string(&config).unwrap();
        assert!(out.contains("certificate_id = \"c-123\""), "{out}");
        assert!(
            !out.contains("MTLS_OTHER") && !out.contains("\"ops\""),
            "{out}"
        );
        // Another environment has linked nothing, so each binding is dropped.
        std::fs::write(&config, original).unwrap();
        let other = materialise_mtls_for_dev(&dir, "shop", &config, "staging").unwrap_err();
        assert!(
            other.contains("`MTLS_OTHER`") && other.contains("staging"),
            "{other}"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    /// #3301: `bynk dev -- --remote` binds a stored secret by the store id the
    /// last deploy recorded for the environment. An unlinked one is refused
    /// naming it, and the store's *name* is never left in `store_id`, where
    /// Cloudflare would take it as an id.
    #[test]
    fn materialise_deploy_state_binds_a_linked_stored_secret_by_its_recorded_id() {
        let dir = scratch_lock_dir("materialise-stored-secret");
        let config = dir.join("wrangler.toml");
        let text = "[[secrets_store_secrets]]\nbinding = \"SECRET_STRIPE_KEY\"\nstore_id = \"payments\"\nsecret_name = \"stripe-live\"\n";
        std::fs::write(&config, text).unwrap();
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        let link = |store_id: &str| StoredSecretLink {
            store: "payments".into(),
            store_id: store_id.into(),
            secret: "stripe-live".into(),
        };
        // Linked only under `staging`: the default environment is refused.
        lock.record::<StoredSecret>("staging", "shop-cart", "SECRET_STRIPE_KEY", link("store-s"));
        std::fs::write(dir.join(LOCK_FILE), toml::to_string_pretty(&lock).unwrap()).unwrap();
        let err = materialise_deploy_state(&dir, "shop-cart", &config, "default")
            .expect_err("an unlinked stored secret must not pass");
        assert!(
            err.contains("`SECRET_STRIPE_KEY`")
                && err.contains("`shop-cart`")
                && err.contains("bynk deploy --env default"),
            "{err}"
        );
        assert_eq!(std::fs::read_to_string(&config).unwrap(), text);

        // Each environment binds its own recorded id.
        assert!(materialise_deploy_state(&dir, "shop-cart", &config, "staging").unwrap());
        let parsed: toml::Table = std::fs::read_to_string(&config).unwrap().parse().unwrap();
        let entry = &parsed["secrets_store_secrets"][0];
        assert_eq!(entry["store_id"].as_str(), Some("store-s"));
        assert_eq!(entry["secret_name"].as_str(), Some("stripe-live"));
        let _ = std::fs::remove_dir_all(dir);
    }

    /// S4b (#1967): a project with only the bare `Kv` writes the ledger it
    /// wrote before instances existed, byte for byte: no `kv_instances` table.
    #[test]
    fn a_bare_kv_ledger_is_written_as_it_was_before_instances() {
        let pre_s4b = "version = 1\n\n[environments.default.kv.api]\nid = \"abc\"\n";
        let lock: DeployLock = toml::from_str(pre_s4b).expect("a pre-S4b ledger reads");
        assert_eq!(
            lock.recorded::<Kv>("default", "api", "KV")
                .map(|kv| kv.id.as_str()),
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
        lock.record::<Kv>(
            "default",
            "shop-cart",
            "KV_SESSIONS",
            KvNamespace::of("sess"),
        );
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(
            text.contains("[environments.default.kv_instances.shop-cart.KV_SESSIONS]"),
            "{text}"
        );
        assert_eq!(
            lock.recorded::<Kv>("default", "shop-cart", "KV_SESSIONS")
                .map(|kv| kv.id.as_str()),
            Some("sess")
        );
        assert_eq!(
            lock.recorded::<Kv>("default", "shop-cart", "KV")
                .map(|kv| kv.id.as_str()),
            Some("ns-id")
        );
        assert_eq!(
            lock.recorded::<Kv>("default", "shop-cart", "KV_OTHER")
                .map(|kv| kv.id.as_str()),
            None
        );
        assert_eq!(
            toml::from_str::<DeployLock>(&text).unwrap(),
            lock,
            "the instance table round-trips"
        );
        lock.forget::<Kv>("default", "shop-cart", "KV_SESSIONS");
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

    // ---- P4a (#2602): D1 databases ----

    #[test]
    fn a_removed_database_is_a_reported_orphan_that_prune_leaves_alone() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1", None),
        );
        lock.record::<D1>(
            "default",
            "shop",
            "D1_OLD",
            D1Record::of("shop-old", "id-2", None),
        );
        let resources = BTreeMap::from([(
            "shop".to_string(),
            Resources::default().d1_databases(&[("D1_ORDERS", "shop-orders")]),
        )]);
        let orphans = find_orphans(&lock, "default", &resources);
        assert_eq!(
            orphans.d1,
            [OrphanD1 {
                worker: "shop".into(),
                binding: "D1_OLD".into(),
                database: "shop-old".into(),
            }]
        );
        // A database holds data: `--prune` neither offers nor attempts it.
        assert!(!orphans.has_prunable());
        // With its Worker gone, every database it had is reported.
        assert_eq!(find_orphans(&lock, "default", &BTreeMap::new()).d1.len(), 2);
        // A database a binding stopped using stays recorded and reported,
        // though the binding is live; recording the same one again does not.
        lock.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders", "id-1", None),
        );
        lock.record::<D1>(
            "default",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders-v2", "id-3", None),
        );
        let orphans = find_orphans(&lock, "default", &resources);
        assert!(
            orphans
                .d1
                .iter()
                .any(|o| o.binding == "D1_ORDERS" && o.database == "shop-orders"),
            "{orphans:?}"
        );
        assert_eq!(
            lock.recorded::<D1>("default", "shop", "D1_ORDERS")
                .unwrap()
                .superseded
                .len(),
            1
        );
    }

    fn r2_lock() -> DeployLock {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record::<R2>(
            "default",
            "shop",
            "R2_ARCHIVES",
            R2Bucket::of("shop-archives", None),
        );
        lock.record::<R2>(
            "default",
            "shop",
            "R2_OLD",
            R2Bucket::of("shop-old", Some("eu")),
        );
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
            lock.recorded::<R2>("default", "shop", "R2_OLD"),
            Some(&R2Bucket {
                name: "shop-old".into(),
                jurisdiction: Some("eu".into()),
                superseded: Vec::new()
            })
        );
        assert!(lock.recorded::<R2>("staging", "shop", "R2_OLD").is_none());
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
        lock.record::<R2>(
            "staging",
            "shop",
            "R2_ARCHIVES",
            R2Bucket::of("shop-archives-staging", None),
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

    // P4a (#2602): `dev --remote` binds a recorded database by name and id.
    const D1_CONFIG: &str = "name = \"shop\"\n\n[[d1_databases]]\nbinding = \"D1_ORDERS\"\ndatabase_name = \"shop-orders\"\ndatabase_id = \"<D1_ORDERS_ID>\" # set at deploy time\n";

    #[test]
    fn dev_remote_binds_a_recorded_database_and_refuses_an_unrecorded_one() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record::<D1>(
            "staging",
            "shop",
            "D1_ORDERS",
            D1Record::of("shop-orders-staging", "id-7", None),
        );
        let (root, config) = r2_project("d1-staging", D1_CONFIG, &lock);
        assert_eq!(
            materialise_deploy_state(&root, "shop", &config, "staging"),
            Ok(true)
        );
        let text = std::fs::read_to_string(&config).unwrap();
        assert!(
            text.contains("database_name = \"shop-orders-staging\"")
                && text.contains("database_id = \"id-7\""),
            "{text}"
        );
        let _ = std::fs::remove_dir_all(root);
        // Nothing recorded under `default`: refused, naming the remedy.
        let (root, config) = r2_project("d1-default", D1_CONFIG, &lock);
        let err = materialise_deploy_state(&root, "shop", &config, "default").unwrap_err();
        assert!(
            err.contains("remote D1 database `D1_ORDERS` for `shop` has not been provisioned")
                && err.contains("bynk deploy --env default"),
            "{err}"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn dev_remote_leaves_the_default_environments_bucket_as_generated() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record::<R2>(
            "default",
            "shop",
            "R2_ARCHIVES",
            R2Bucket::of("shop-archives", None),
        );
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
        lock.record::<R2>(
            "default",
            "shop",
            "R2_ARCHIVES",
            R2Bucket::of("shop-archives", None),
        );
        // Recording the same bucket again changes nothing.
        lock.record::<R2>(
            "default",
            "shop",
            "R2_ARCHIVES",
            R2Bucket::of("shop-archives", None),
        );
        assert!(
            lock.recorded::<R2>("default", "shop", "R2_ARCHIVES")
                .unwrap()
                .superseded
                .is_empty()
        );
        lock.record::<R2>(
            "default",
            "shop",
            "R2_ARCHIVES",
            R2Bucket::of("shop-archives", Some("eu")),
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
        lock.record::<R2>(
            "default",
            "shop",
            "R2_ARCHIVES",
            R2Bucket::of("shop-archives", Some("eu")),
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
        QueueTable::created(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "jobs",
        );
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
    fn a_queue_a_context_only_sends_to_is_not_orphaned() {
        // P2 (#2578): a recorded queue no context consumes but one sends to.
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        QueueTable::created(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "order-events",
        );
        let resources = BTreeMap::from([(
            "shop".into(),
            Resources::default().produces(&["order-events"]),
        )]);
        assert!(find_orphans(&lock, "default", &resources).queues.is_empty());
    }

    #[test]
    fn a_queue_no_context_declares_anymore_is_orphaned() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        QueueTable::created(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "stale-jobs",
        );
        let resources = BTreeMap::from([("orders".into(), Resources::default())]);

        let orphans = find_orphans(&lock, "default", &resources);
        assert_eq!(orphans.queues, names(&["stale-jobs"]));
    }

    #[test]
    fn a_queue_only_reused_is_never_orphaned() {
        // #2579: a consumed queue another Worker owns is recorded so the plan
        // says `reuse`, but dropping the consumer must not make it prunable.
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        assert!(QueueTable::reused(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "theirs"
        ));
        assert!(!QueueTable::reused(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "theirs"
        ));
        QueueTable::created(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "ours",
        );
        let resources = BTreeMap::from([("orders".into(), Resources::default())]);

        assert!(
            lock.recorded::<Queue>("default", "", "theirs").is_some(),
            "the plan still says reuse"
        );
        let orphans = find_orphans(&lock, "default", &resources);
        assert_eq!(orphans.queues, names(&["ours"]));

        // Recreated by a later run (deleted out-of-band), it becomes Bynk's.
        assert!(QueueTable::created(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "theirs"
        ));
        let orphans = find_orphans(&lock, "default", &resources);
        assert_eq!(orphans.queues, names(&["ours", "theirs"]));
    }

    #[test]
    fn a_ledger_with_reused_queues_round_trips() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        QueueTable::reused(
            lock.environments
                .entry(String::from("default"))
                .or_default(),
            "theirs",
        );
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("reused_queues"));
        assert_eq!(toml::from_str::<DeployLock>(&text).unwrap(), lock);
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

    /// #3565: the fan-in move set of `design/tracks/agent-context-moves.md` §4.1, part-way
    /// through phase 2.
    fn fan_in_move() -> AgentMoveRecord {
        let entry = |context: &str, class: &str, from: &str| AgentMoveEntry {
            context: context.into(),
            classes: vec![class.into()],
            from_worker: from.into(),
            from_script: from.into(),
            to_worker: "core".into(),
            to_script: "core".into(),
        };
        AgentMoveRecord {
            move_id: "3f9c2a71".into(),
            step: MoveStep::Committing,
            started: "2026-11-27T10:15:02Z".into(),
            bynk: "0.0.0".into(),
            done: vec!["shop-orders".into()],
            moves: vec![
                entry("shop.orders", "Cart", "shop-orders"),
                entry("shop.billing", "Ledger", "shop-billing"),
            ],
            deployments: BTreeMap::from([
                ("core".into(), "none".into()),
                ("shop-billing".into(), "d-2".into()),
                ("shop-orders".into(), "d-1".into()),
            ]),
        }
    }

    /// #3565 (D5): a ledger with a move under way is written as version 2 and reads back with
    /// the record intact; once the last move closes it is version 1 again, as before moves.
    #[test]
    fn an_open_move_is_written_as_version_2_and_round_trips() {
        let path = scratch_lock_path("open-move");
        let mut lock: DeployLock = toml::from_str("version = 1\n").unwrap();
        let env = lock.environments.entry("default".into()).or_default();
        env.agent_moves.push(fan_in_move());
        env.transferred_classes.insert(
            "shop-orders".into(),
            BTreeMap::from([("Cart".into(), "core".into())]),
        );
        write_lock(&path, &lock).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("version = 2\n"), "{text}");
        assert!(
            text.contains("[[environments.default.agent_moves]]")
                && text.contains("move = \"3f9c2a71\"")
                && text.contains("step = \"committing\"")
                && text.contains("core = \"none\""),
            "{text}"
        );
        // A bynk from before moves accepts only version 1 (`read_lock`'s check before #3565),
        // so it refuses this file instead of reading past the record.
        let written: toml::Value = toml::from_str(&text).unwrap();
        assert_ne!(written["version"].as_integer(), Some(lock_version().into()));
        let read = read_lock(&path).unwrap();
        assert_eq!(read.environments, lock.environments);
        assert!(read.has_open_move());

        let mut closed = read;
        closed
            .environments
            .get_mut("default")
            .unwrap()
            .agent_moves
            .clear();
        write_lock(&path, &closed).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("version = 1\n"), "{text}");
        assert!(!text.contains("agent_moves"), "{text}");
        let _ = std::fs::remove_file(path);
    }

    /// #3565: a ledger with no move writes neither new field, so it is the file it was.
    #[test]
    fn a_ledger_without_a_move_is_written_as_before() {
        let path = scratch_lock_path("no-move");
        // (`kv` is always written, as it was before moves.)
        let before = "version = 1\n\n[environments.default]\nqueues = [\"jobs\"]\n\n[environments.default.kv]\n";
        std::fs::write(&path, before).unwrap();
        let lock = read_lock(&path).unwrap();
        write_lock(&path, &lock).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        let _ = std::fs::remove_file(path);
    }

    /// #3565 (D5): version 2 belongs to a ledger mid-move, so one that records no move is
    /// corrupt, said with its own message; any other version is still unsupported.
    #[test]
    fn a_version_2_ledger_with_no_move_is_corrupt() {
        let path = scratch_lock_path("v2-no-move");
        std::fs::write(&path, "version = 2\n").unwrap();
        let e = read_lock(&path).unwrap_err();
        assert!(
            e.contains("says version 2") && e.contains("records no move") && e.contains("corrupt"),
            "{e}"
        );
        std::fs::write(&path, "version = 3\n").unwrap();
        assert_eq!(
            read_lock(&path).unwrap_err(),
            "unsupported deploy lock version 3"
        );
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

    /// #3562: a ledger written before groups has no `contexts`, still reads, and
    /// is written back unchanged; a group's Worker records its members.
    #[test]
    fn a_pre_groups_ledger_reads_and_a_groups_worker_records_its_members() {
        let before = "version = 1\n\n[environments.default.workers.shop-orders]\ndeployed = true\n\n[environments.default.workers.shop-orders.contracts]\ncharge = \"abc\"\n";
        let mut lock: DeployLock = toml::from_str(before).unwrap();
        assert!(lock.is_deployed("default", "shop-orders"));
        assert!(lock.members_of("default", "shop-orders").is_empty());
        assert!(
            !toml::to_string_pretty(&lock).unwrap().contains("contexts"),
            "a Worker with no members writes no `contexts`"
        );
        lock.record_deployed("default", "core", Some(BTreeMap::new()));
        lock.record_members(
            "default",
            "core",
            &["shop.a".to_string(), "shop.b".to_string()],
        );
        let text = toml::to_string_pretty(&lock).unwrap();
        assert!(text.contains("contexts = ["), "{text}");
        let again: DeployLock = toml::from_str(&text).unwrap();
        assert_eq!(again.members_of("default", "core"), ["shop.a", "shop.b"]);
        assert!(again.members_of("default", "shop-orders").is_empty());
        assert!(again.members_of("default", "unrecorded").is_empty());
        assert_eq!(again, lock);
    }

    /// #3562: an orphaned Worker the ledger recorded as a group's is reported with
    /// its members; one it recorded without any (a context's own Worker, or a
    /// pre-groups ledger) is reported as before.
    #[test]
    fn an_orphaned_groups_members_are_named() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_deployed("default", "gone-group", Some(BTreeMap::new()));
        lock.record_members(
            "default",
            "gone-group",
            &["shop.a".to_string(), "shop.b".to_string()],
        );
        lock.record_deployed("default", "gone-alone", Some(BTreeMap::new()));
        let orphans = find_orphans(&lock, "default", &BTreeMap::new());
        assert_eq!(orphans.workers, ["gone-alone", "gone-group"]);
        assert_eq!(
            orphans.worker_contexts,
            BTreeMap::from([(
                "gone-group".to_string(),
                vec!["shop.a".to_string(), "shop.b".to_string()]
            )])
        );
        let shown = orphans.as_named_in("staging");
        assert!(
            shown.worker_contexts.contains_key("gone-group-staging"),
            "{:?}",
            shown.worker_contexts
        );
    }

    /// #3562: a renamed group is named by its script, not its key, where the
    /// account's name is wanted; a Worker with no recorded script is named by its key.
    #[test]
    fn a_renamed_groups_script_is_recorded_and_shown() {
        let mut lock = DeployLock {
            version: 1,
            ..Default::default()
        };
        lock.record_deployed("default", "core", Some(BTreeMap::new()));
        lock.record_script("default", "core", "core-script");
        lock.record_deployed("default", "plain", Some(BTreeMap::new()));
        lock.record_script("default", "plain", "plain");
        assert_eq!(lock.script_of("default", "core"), "core-script");
        assert_eq!(lock.script_of("default", "plain"), "plain");
        assert_eq!(lock.script_of("default", "unrecorded"), "unrecorded");
        let text = toml::to_string_pretty(&lock).unwrap();
        assert_eq!(text.matches("script =").count(), 1, "{text}");
        assert_eq!(toml::from_str::<DeployLock>(&text).unwrap(), lock);
        let orphans = find_orphans(&lock, "default", &BTreeMap::new());
        assert_eq!(
            orphans.as_named_in("staging").workers,
            ["core-script-staging", "plain-staging"]
        );
    }
}
