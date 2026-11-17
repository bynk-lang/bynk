//! A Worker's binding names must be unique (`bynk.context.duplicate_binding`).
//!
//! A Worker's bindings come from derivations that know nothing of each other:
//! the KV namespace's fixed [`crate::firstparty::KV_BINDING_NAME`], a consumed
//! context's name (`a.b` → `A_B`), an agent's class name (`OrderEntity` →
//! `ORDER_ENTITY`) and a derived capability's (`Billing` →
//! `SERVICE_BILLING`). Two of them can meet: an agent `Kv` in a context that
//! consumes `bynk.cloudflare { Kv }` is `KV` twice, and an agent
//! `ServiceBilling` and a `capability Billing = Service` are both
//! `SERVICE_BILLING`. Wrangler takes such a
//! config without complaint and one binding hides the other, so the clash is
//! a compile error. The names themselves are never adjusted to avoid it,
//! because deployed Workers depend on them.
//!
//! The names are the emitter's (`bynk_emit::emitter::wrangler::
//! worker_bindings`, the list `wrangler.toml` is written from); this module
//! owns the check over them and the diagnostic.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;

use bynk_project::ParsedFile;
use bynk_syntax::ast::{CommonsItem, ServiceProtocol, SourceUnit};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::firstparty;
use crate::project_model::ErrorSink;

/// A declaration's name in screaming snake case (`OrderEntity` →
/// `ORDER_ENTITY`): the casing of an agent's binding and of a derived
/// capability's. One function, so the two kinds of name the duplicate check
/// compares cannot drift apart (review of #1968).
pub fn screaming_snake(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
    }
    out
}

/// S4a (#1966): the Worker binding name of a derived capability, the kind's
/// prefix then the capability name in [`screaming_snake`] case (`Sessions` →
/// `KV_SESSIONS`, `PriceCache` → `KV_PRICE_CACHE`).
pub fn instance_binding_name(prefix: &str, capability: &str) -> String {
    format!("{prefix}_{}", screaming_snake(capability))
}

/// What put a binding into a Worker's `wrangler.toml`: the source a
/// `bynk.context.duplicate_binding` diagnostic names.
///
/// Each variant's doc also says whether a handler's `deps.env` type exposes
/// it on Workers (#1950, `bynk_emit::emitter::wrangler::handler_bindings`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingSource {
    /// A `[[services]]` Service Binding to another context's Worker: a
    /// consumed context whose services the context calls (#1961 — consuming
    /// one for its types or events alone binds nothing), or one subscribing to
    /// an event it publishes. Handlers see a called context's binding; never a
    /// subscriber's, which only the events fan-out calls.
    Service { context: String },
    /// The `[[kv_namespaces]]` binding the cloudflare adapter's `Kv` reads.
    /// Never seen by handlers: they reach KV through the `WorkersKv` provider,
    /// which takes the Worker's `env` at compose time.
    Kv,
    /// S4a (#1966): the `[[kv_namespaces]]` binding of a derived capability
    /// (`capability Sessions = Kv`), one per instance. Never seen by handlers:
    /// they reach it through the instance's `WorkersKv`.
    KvInstance { capability: String },
    /// Interop I5 (#1972): the `[[services]]` binding of a `Service`
    /// instance (`capability Billing = Service @worker("legacy-billing")`), a
    /// Service Binding to a Worker outside the project. Never seen by
    /// handlers: they reach it through the instance's `WorkersService`.
    ForeignService { capability: String, worker: String },
    /// P5 (#2323): the `[[r2_buckets]]` binding of an `R2` instance
    /// (`capability Archives = R2`), one bucket per instance, pinned to
    /// `jurisdiction` when it names one. Never seen by handlers: they reach
    /// it through the instance's `WorkersR2`.
    R2Instance {
        capability: String,
        jurisdiction: Option<String>,
    },
    /// Standard-library P3 (#2342): the `[[ratelimits]]` binding of a
    /// `RateLimit` instance (`capability LoginLimit = RateLimit @limit(100)
    /// @period(60.seconds)`), allowing `limit` calls per key every `period`
    /// seconds. Never seen by handlers: they reach it through the instance's
    /// `WorkersRateLimit`.
    RateLimitInstance {
        capability: String,
        limit: i64,
        period: i64,
    },
    /// Standard-library P2 (#2578): the `[[queues.producers]]` binding of a
    /// `Queue` instance (`capability OrderEvents = Queue @name("order-events")`),
    /// sending to `queue` as written (`bynk deploy` environment-qualifies it).
    /// Never seen by handlers: they reach it through the instance's
    /// `WorkersQueue`.
    QueueProducer { capability: String, queue: String },
    /// Standard-library P3b (#2582): the `[[analytics_engine_datasets]]`
    /// binding of an `Analytics` instance (`capability Signups = Analytics`),
    /// one dataset per instance under a derived name. Never seen by handlers:
    /// they reach it through the instance's `WorkersAnalytics`.
    AnalyticsDataset { capability: String },
    /// Standard-library P4a (#2602): the `[[d1_databases]]` binding of a `D1`
    /// instance (`capability Orders = D1`), one database per instance under a
    /// derived name, its id minted by `bynk deploy`. Never seen by handlers:
    /// they reach it through the instance's `WorkersD1`.
    D1Database { capability: String },
    /// #3301: the `[[secrets_store_secrets]]` binding of a `StoredSecret`
    /// instance (`capability StripeKey = StoredSecret @store("payments")
    /// @secret("stripe-live")`), reading `secret` of the Secrets Store named
    /// `store`. The store is linked by `bynk deploy`, which resolves its id,
    /// and never created. Never seen by handlers: they reach it through the
    /// instance's `WorkersStoredSecret`.
    StoredSecret {
        capability: String,
        store: String,
        secret: String,
    },
    /// #3302: the `[[send_email]]` binding of an `Email` instance
    /// (`capability Mail = Email @allowedSenders([…])`), with the restrictions
    /// its annotations carry. Never seen by handlers: they reach it through
    /// the instance's `WorkersEmail`.
    Email {
        capability: String,
        to: Option<String>,
        allowed_recipients: Vec<String>,
        allowed_senders: Vec<String>,
    },
    /// Hyperdrive H1 (#3318): the `[[hyperdrive]]` binding of a `Postgres`
    /// instance (`capability Main = Postgres`). Its `id` is a placeholder until
    /// the Hyperdrive configuration exists (created by hand until H2). Never
    /// seen by handlers: they reach it through the instance's `WorkersPostgres`.
    Postgres { capability: String },
    /// Hyperdrive H5 (#3322): the `[[hyperdrive]]` binding of a `MySql`
    /// instance (`capability Legacy = MySql`), the same stanza as `Postgres`'s.
    /// `tls` is its `@tls("…")` when it carries one. Never seen by handlers:
    /// they reach it through the instance's `WorkersMySql`.
    MySql {
        capability: String,
        tls: Option<String>,
    },
    /// #3479: the `[[vectorize]]` binding of a `Vectorize` instance
    /// (`capability Docs = Vectorize @dimensions(768) @metric("cosine")`), one
    /// index per instance under a derived name that `bynk deploy`
    /// environment-qualifies and creates with `dimensions` and `metric`, which
    /// Cloudflare cannot change afterwards. `indexed` is each `@indexed`
    /// entry as written (`name:type`). Never seen by handlers: they reach it
    /// through the instance's `WorkersVectorize`.
    VectorizeIndex {
        capability: String,
        dimensions: i64,
        metric: String,
        indexed: Vec<String>,
    },
    /// #3477: the Durable Object namespace of a `Container` instance
    /// (`capability Renderer = Container @build("…") @port(8080)`): a
    /// `[[durable_objects.bindings]]` entry for the generated class
    /// `class_name` (`__Container_Renderer`), its `exports` entry, and a
    /// `[[containers]]` entry whose image is built from `build`, the
    /// project-relative directory of its `Dockerfile`. `sleep_after`
    /// (seconds), `instance_type` and `forward` are applied when the class
    /// starts the container, not in config. Never seen by handlers: they reach
    /// it through the instance's `WorkersContainer`.
    ContainerInstance {
        capability: String,
        class_name: String,
        build: String,
        // The path from the Worker's output directory (`<out>/workers/<worker>`)
        // back to the project root (`../../..` for the default `out`), which
        // the driver fills in from `[build] out`; empty until then.
        root_from_worker: String,
        port: i64,
        sleep_after: Option<i64>,
        instance_type: Option<String>,
        forward: Vec<String>,
    },
    /// #3480: the `[[ai_search]]` binding of an `AiSearch` instance
    /// (`capability Kb = AiSearch @instance("support-docs")`), naming the
    /// account's instance `instance` as written, in every environment. The
    /// instance is linked by `bynk deploy`, which checks it exists, and never
    /// created. Never seen by handlers: they reach it through the instance's
    /// `WorkersAiSearch`.
    AiSearchInstance {
        capability: String,
        instance: String,
    },
    /// #3513 A: the `[[mtls_certificates]]` binding of an `MtlsCertificate`
    /// instance (`capability Backend = MtlsCertificate @certificate("acme-client")`),
    /// naming the account's client certificate `certificate` as written, in every
    /// environment. The certificate is linked by `bynk deploy`, which resolves its id and
    /// never uploads it. Never seen by handlers: they reach it through the instance's
    /// `WorkersMtls`.
    MtlsCertificate {
        capability: String,
        certificate: String,
    },
    /// #3300: the binding of a one-per-Worker capability of `bynk.cloudflare`
    /// (`Version`'s `[version_metadata]`), declared when the Worker's
    /// closure reaches the bare capability. Never seen by handlers: they
    /// reach it through the capability's provider (`WorkersVersion`), which
    /// takes the Worker's `env` at compose time.
    Singleton { capability: String },
    /// A `[[durable_objects.bindings]]` entry for one of the context's agents.
    /// A handler that constructs an agent sees every local agent's binding.
    Agent { class_name: String },
    /// The `[[durable_objects.bindings]]` entry for the events fan-out class
    /// of a context whose handlers emit. Never seen by handlers: they reach it
    /// through `deps.__eventsDispatch`.
    EventsFanout { class_name: String },
}

/// W30 (#2840): the `capability` of the `D1Database` source of a Worker's
/// implicit idempotency database. Not an identifier, so no project's own derived
/// `D1` capability can carry it: whether a binding is the implicit one is
/// decided by its source, never by its name (a project's `capability
/// Idempotency = D1` is bound as `D1_IDEMPOTENCY` too).
pub const IMPLICIT_IDEMPOTENCY_CAPABILITY: &str = "@idempotency";

/// W30 (#2840): the binding of a Worker's implicit idempotency database, named
/// [`IDEMPOTENCY_BINDING`](crate::d1_migrations::IDEMPOTENCY_BINDING).
pub fn idempotency_binding() -> WorkerBinding {
    WorkerBinding {
        name: crate::d1_migrations::IDEMPOTENCY_BINDING.to_string(),
        source: BindingSource::D1Database {
            capability: IMPLICIT_IDEMPOTENCY_CAPABILITY.to_string(),
        },
    }
}

impl BindingSource {
    /// The source in a diagnostic's words.
    pub fn describe(&self) -> String {
        match self {
            Self::Service { context } => format!("the Service Binding to context `{context}`"),
            Self::Kv => format!(
                "the KV namespace binding of `{}`'s `Kv`",
                firstparty::CLOUDFLARE_UNIT
            ),
            Self::KvInstance { capability } => {
                format!("the KV namespace binding of derived capability `{capability}`")
            }
            Self::ForeignService { capability, worker } => format!(
                "the Service Binding of derived capability `{capability}` (to Worker `{worker}`)"
            ),
            Self::R2Instance { capability, .. } => {
                format!("the R2 bucket binding of derived capability `{capability}`")
            }
            Self::RateLimitInstance { capability, .. } => {
                format!("the rate limiting binding of derived capability `{capability}`")
            }
            Self::QueueProducer { capability, queue } => format!(
                "the queue producer binding of derived capability `{capability}` (to queue `{queue}`)"
            ),
            Self::AnalyticsDataset { capability } => {
                format!("the Analytics Engine binding of derived capability `{capability}`")
            }
            Self::D1Database { capability } if capability == IMPLICIT_IDEMPOTENCY_CAPABILITY => {
                "the implicit idempotency database of a Worker that calls `Idempotency.claim`"
                    .to_string()
            }
            Self::D1Database { capability } => {
                format!("the D1 database binding of derived capability `{capability}`")
            }
            Self::StoredSecret {
                capability, store, ..
            } => format!(
                "the Secrets Store binding of derived capability `{capability}` (to store `{store}`)"
            ),
            Self::Email { capability, .. } => {
                format!("the `[[send_email]]` binding of derived capability `{capability}`")
            }
            Self::VectorizeIndex { capability, .. } => {
                format!("the `[[vectorize]]` binding of derived capability `{capability}`")
            }
            Self::ContainerInstance { capability, .. } => {
                format!(
                    "the Durable Object binding of the container of derived capability `{capability}`"
                )
            }
            Self::AiSearchInstance {
                capability,
                instance,
            } => format!(
                "the `[[ai_search]]` binding of derived capability `{capability}` (to instance `{instance}`)"
            ),
            Self::MtlsCertificate {
                capability,
                certificate,
            } => format!(
                "the `[[mtls_certificates]]` binding of derived capability `{capability}` (to certificate `{certificate}`)"
            ),
            Self::Postgres { capability } | Self::MySql { capability, .. } => {
                format!("the `[[hyperdrive]]` binding of derived capability `{capability}`")
            }
            Self::Singleton { capability } => format!(
                "the `{capability}` binding of `{}`",
                firstparty::CLOUDFLARE_UNIT
            ),
            Self::Agent { class_name } => {
                format!("the Durable Object binding of agent `{class_name}`")
            }
            Self::EventsFanout { .. } => {
                "the Durable Object binding of the events fan-out".to_string()
            }
        }
    }
}

/// One binding a Worker declares: the name its `env` field and its
/// `wrangler.toml` entry share, and where that name came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerBinding {
    pub name: String,
    pub source: BindingSource,
}

/// The bindings in `bindings` whose name an earlier one already took, each
/// paired with that earlier one, in list order.
pub fn duplicate_bindings(bindings: &[WorkerBinding]) -> Vec<(&WorkerBinding, &WorkerBinding)> {
    let mut first: HashMap<&str, &WorkerBinding> = HashMap::new();
    let mut duplicates = Vec::new();
    for binding in bindings {
        match first.get(binding.name.as_str()) {
            Some(earlier) => duplicates.push((*earlier, binding)),
            None => {
                first.insert(&binding.name, binding);
            }
        }
    }
    duplicates
}

/// `bynk.context.duplicate_binding`: each context's Worker declares every
/// binding name once. `bindings` holds each context's bindings as its
/// `wrangler.toml` would list them. One error per clash, at the declaration
/// that brought in the second binding (an agent, or a `consumes` clause),
/// preferring one the author wrote over `consumes bynk.cloudflare`, naming
/// both sources. A clash between two sources with no declaration of their own
/// (Service Bindings added for event subscribers) is reported at the
/// context's name.
pub fn phase_duplicate_bindings(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    bindings: &BTreeMap<String, Vec<WorkerBinding>>,
    errors: &mut ErrorSink,
) {
    for (ctx, ctx_bindings) in bindings {
        let indices = groups.get(ctx).map(Vec::as_slice).unwrap_or_default();
        for (earlier, later) in duplicate_bindings(ctx_bindings) {
            // At the declaration the author can change: the later source's,
            // else the earlier's, but never the `bynk.cloudflare` clause
            // while the other source has a declaration of its own.
            let mut sources = [later, earlier];
            sources.sort_by_key(|b| {
                matches!(
                    b.source,
                    BindingSource::Kv | BindingSource::Singleton { .. }
                )
            });
            let (file, span) = sources
                .iter()
                .find_map(|b| declared_at(parsed, indices, b))
                .or_else(|| context_header(parsed, indices))
                .map_or((None, Span::default()), |(file, span)| (Some(file), span));
            errors.push_for(
                file.as_deref(),
                CompileError::new(
                    "bynk.context.duplicate_binding",
                    span,
                    format!(
                        "the Worker for `{ctx}` would declare two bindings named `{}`: {} and {}",
                        later.name,
                        earlier.source.describe(),
                        later.source.describe(),
                    ),
                )
                .with_note(
                    "a Worker's bindings share one `env`, so each name must be unique; binding names follow agent, context and derived capability names, so rename the declaration behind one of them",
                ),
            );
        }
    }
}

/// Whether two identical declarations of `source` in different members of a
/// group are one shared binding. A resource is identified by its declaration
/// (a capability and its options), so identical ones share. An agent is not: its
/// binding holds only the class's name, but its class is the code of the context
/// that declares it, so two members' agents of one name are two classes.
fn shareable(source: &BindingSource) -> bool {
    !matches!(source, BindingSource::Agent { .. })
}

/// The bindings of one Worker from its members' (#3562): each name once, at its
/// first declaration. Two members declaring one name is one binding when the
/// declarations are identical; when they differ it is the error
/// [`phase_group_binding_collisions`] reports, so a build that reaches the emit
/// has none.
pub fn merge_group_bindings<'a>(
    members: impl IntoIterator<Item = &'a [WorkerBinding]>,
) -> Vec<WorkerBinding> {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut merged: Vec<WorkerBinding> = Vec::new();
    for bindings in members {
        for binding in bindings {
            if seen.insert(binding.name.as_str()) {
                merged.push(binding.clone());
            }
        }
    }
    merged
}

/// `bynk.deploy.binding_collision` (#3562): the members of a Worker group share
/// one `wrangler.toml` and one `env`, so a binding name two members both
/// declare is one binding when their declarations are identical (the same kind
/// and options, so the point of co-location is shared state) and an error when
/// they differ. `bindings` holds each context's bindings
/// ([`phase_duplicate_bindings`] reports a name one context repeats). One error
/// per clash, at the later member's declaration, naming both members and both
/// sources.
pub fn phase_group_binding_collisions(
    worker_groups: &bynk_project::worker_groups::WorkerGroups,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    bindings: &BTreeMap<String, Vec<WorkerBinding>>,
    errors: &mut ErrorSink,
) {
    group_worker_cycles(worker_groups, parsed, groups, bindings, errors);
    for (key, group) in worker_groups {
        group_service_names(key, &group.contexts, parsed, groups, errors);
        group_websockets(key, &group.contexts, parsed, groups, errors);
        crate::group_triggers::group_trigger_conflicts(
            key,
            &group.contexts,
            parsed,
            groups,
            errors,
        );
        let mut first: HashMap<&str, (&str, &WorkerBinding)> = HashMap::new();
        for ctx in &group.contexts {
            let indices = groups.get(ctx).map(Vec::as_slice).unwrap_or_default();
            for binding in bindings.get(ctx).into_iter().flatten() {
                let Some(&(earlier_ctx, earlier)) = first.get(binding.name.as_str()) else {
                    first.insert(&binding.name, (ctx, binding));
                    continue;
                };
                if earlier_ctx == ctx
                    || (shareable(&binding.source) && earlier.source == binding.source)
                {
                    continue;
                }
                let (file, span) = declared_at(parsed, indices, binding)
                    .or_else(|| context_header(parsed, indices))
                    .map_or((None, Span::default()), |(file, span)| (Some(file), span));
                errors.push_for(
                    file.as_deref(),
                    CompileError::new(
                        "bynk.deploy.binding_collision",
                        span,
                        format!(
                            "contexts `{earlier_ctx}` and `{ctx}` share Worker `{key}`, which would declare two different bindings named `{}`: {} (from `{earlier_ctx}`) and {} (from `{ctx}`)",
                            binding.name,
                            earlier.source.describe(),
                            binding.source.describe(),
                        ),
                    )
                    .with_note(if shareable(&binding.source) {
                        "members of one Worker share its bindings: give both contexts the same declaration so they share one binding, or rename one capability"
                    } else {
                        "a Durable Object class belongs to the context that declares it, whatever its name, so two members cannot each declare one with the same name: rename one of the agents"
                    }),
                );
            }
        }
    }
}

/// `bynk.deploy.service_name_conflict` (#3562): a service is reached by its name
/// (`/_bynk/call/<service>` carries no context), and a Worker answers every
/// member's services, so two members of one group cannot each declare a service
/// of one name. One error per repeat, at the later member's service.
fn group_service_names(
    key: &str,
    members: &[String],
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    errors: &mut ErrorSink,
) {
    let mut first: HashMap<String, &str> = HashMap::new();
    for ctx in members {
        let indices = groups.get(ctx).map(Vec::as_slice).unwrap_or_default();
        for &i in indices {
            let SourceUnit::Context(c) = parsed[i].unit() else {
                continue;
            };
            for item in &c.items {
                let CommonsItem::Service(service) = item else {
                    continue;
                };
                let name = &service.name.name;
                match first.get(name.as_str()) {
                    None => {
                        first.insert(name.clone(), ctx);
                    }
                    Some(earlier) if *earlier != ctx.as_str() => {
                        errors.push_for(
                            Some(&parsed[i].identity_path()),
                            CompileError::new(
                                "bynk.deploy.service_name_conflict",
                                service.name.span,
                                format!(
                                    "contexts `{earlier}` and `{ctx}` share Worker `{key}`, and both declare a service named `{name}`"
                                ),
                            )
                            .with_note(
                                "a Worker answers its members' services by name, so each service name must be unique within a group: rename one of the services, or put the contexts in different groups",
                            ),
                        );
                    }
                    Some(_) => {}
                }
            }
        }
    }
}

/// `bynk.deploy.group_cycle` (#3562): a Worker must be uploaded after the Workers
/// it binds to (ADR 0193 D1), and the language rules out a cycle among *contexts*
/// (`bynk.context.consumes_cycle`), but a group folds its members into one node,
/// so two groups can depend on each other with no cycle among their contexts
/// (`A1 -> B1` and `B2 -> A2`, with `A1`, `A2` in one group and `B1`, `B2` in
/// another). The graph is the Service Bindings the Workers would declare, read
/// from `bindings` like deploy's ordering, a context standing for its group's
/// Worker. An edge inside a group is a Worker binding to itself and is no edge.
/// One error per strongly connected set of Workers containing a group, showing one
/// representative cycle: the Workers and one context edge per hop, at the first
/// hop's consumer. A cycle among ungrouped Workers is deliberately not reported
/// here. `consumes_cycle` covers service calls, but `bindings` also holds the
/// publisher-to-subscriber edges, so an ungrouped event-edge cycle is a known gap
/// that predates this check.
fn group_worker_cycles(
    worker_groups: &bynk_project::worker_groups::WorkerGroups,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    bindings: &BTreeMap<String, Vec<WorkerBinding>>,
    errors: &mut ErrorSink,
) {
    // A context's Worker: its group's key, else the context's own name.
    let worker_of = |ctx: &str| -> String {
        worker_groups
            .iter()
            .find(|(_, g)| g.contexts.iter().any(|c| c == ctx))
            .map_or_else(|| ctx.to_string(), |(key, _)| key.clone())
    };
    // Worker -> (target Worker -> (consumer context, target context)), the first
    // context edge that makes each Worker edge.
    let mut edges: BTreeMap<String, BTreeMap<String, (String, String)>> = BTreeMap::new();
    for (ctx, list) in bindings {
        for binding in list {
            let BindingSource::Service { context: target } = &binding.source else {
                continue;
            };
            let (from, to) = (worker_of(ctx), worker_of(target));
            if from != to {
                edges
                    .entry(from)
                    .or_default()
                    .entry(to)
                    .or_insert_with(|| (ctx.clone(), target.clone()));
            }
        }
    }
    for scc in strongly_connected(&edges) {
        if !scc.iter().any(|w| worker_groups.contains_key(w)) {
            continue;
        }
        let Some(path) = cycle_through(&edges, &scc) else {
            continue;
        };
        let hops: Vec<String> = path
            .windows(2)
            .map(|pair| {
                let (consumer, target) = &edges[&pair[0]][&pair[1]];
                format!(
                    "`{}` to `{}` (`{consumer}` binds to `{target}`)",
                    pair[0], pair[1]
                )
            })
            .collect();
        let (consumer, _) = &edges[&path[0]][&path[1]];
        let indices = groups.get(consumer).map(Vec::as_slice).unwrap_or_default();
        let (file, span) = context_header(parsed, indices)
            .map_or((None, Span::default()), |(file, span)| (Some(file), span));
        errors.push_for(
            file.as_deref(),
            CompileError::new(
                "bynk.deploy.group_cycle",
                span,
                format!(
                    "the Workers {} bind to each other in a cycle: {}",
                    path[..path.len() - 1]
                        .iter()
                        .map(|w| format!("`{w}`"))
                        .collect::<Vec<_>>()
                        .join(", "),
                    hops.join(", ")
                ),
            )
            .with_note(
                "a Worker is uploaded after the Workers it binds to, so a cycle between Workers cannot be deployed. The contexts themselves are acyclic, but a group folds its members into one Worker: move a context out of its group, or into the other, so the Workers' dependencies run one way",
            ),
        );
    }
}

/// The strongly connected components of a graph with more than one node, each
/// sorted, in order of their first node (Tarjan's algorithm, so the report is
/// deterministic).
fn strongly_connected(
    edges: &BTreeMap<String, BTreeMap<String, (String, String)>>,
) -> Vec<Vec<String>> {
    struct State<'a> {
        edges: &'a BTreeMap<String, BTreeMap<String, (String, String)>>,
        index: BTreeMap<String, usize>,
        low: BTreeMap<String, usize>,
        on_stack: BTreeSet<String>,
        stack: Vec<String>,
        out: Vec<Vec<String>>,
    }
    fn visit(state: &mut State<'_>, node: &str) {
        let n = state.index.len();
        state.index.insert(node.to_string(), n);
        state.low.insert(node.to_string(), n);
        state.stack.push(node.to_string());
        state.on_stack.insert(node.to_string());
        let next: Vec<String> = state
            .edges
            .get(node)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        for to in next {
            if !state.index.contains_key(&to) {
                visit(state, &to);
                let low = state.low[node].min(state.low[&to]);
                state.low.insert(node.to_string(), low);
            } else if state.on_stack.contains(&to) {
                let low = state.low[node].min(state.index[&to]);
                state.low.insert(node.to_string(), low);
            }
        }
        if state.low[node] == state.index[node] {
            let mut component = Vec::new();
            while let Some(top) = state.stack.pop() {
                state.on_stack.remove(&top);
                let done = top == node;
                component.push(top);
                if done {
                    break;
                }
            }
            if component.len() > 1 {
                component.sort();
                state.out.push(component);
            }
        }
    }
    let mut state = State {
        edges,
        index: BTreeMap::new(),
        low: BTreeMap::new(),
        on_stack: BTreeSet::new(),
        stack: Vec::new(),
        out: Vec::new(),
    };
    let nodes: BTreeSet<&String> = edges
        .keys()
        .chain(edges.values().flat_map(|m| m.keys()))
        .collect();
    for node in nodes {
        if !state.index.contains_key(node) {
            visit(&mut state, node);
        }
    }
    state.out.sort();
    state.out
}

/// One cycle inside `scc`, as a path that starts and ends at the component's
/// first Worker: depth-first over the component's edges in name order.
fn cycle_through(
    edges: &BTreeMap<String, BTreeMap<String, (String, String)>>,
    scc: &[String],
) -> Option<Vec<String>> {
    fn walk(
        edges: &BTreeMap<String, BTreeMap<String, (String, String)>>,
        scc: &[String],
        path: &mut Vec<String>,
    ) -> bool {
        let here = path.last().expect("a path has a node").clone();
        for next in edges.get(&here).into_iter().flat_map(|m| m.keys()) {
            if !scc.contains(next) {
                continue;
            }
            if *next == path[0] {
                path.push(next.clone());
                return true;
            }
            if !path.contains(next) {
                path.push(next.clone());
                if walk(edges, scc, path) {
                    return true;
                }
                path.pop();
            }
        }
        false
    }
    let mut path = vec![scc.first()?.clone()];
    walk(edges, scc, &mut path).then_some(path)
}

/// `bynk.deploy.multiple_websocket_members` (#3562): a WebSocket upgrade is an
/// `Upgrade: websocket` request, which names no path or service the group could
/// route it by. A group with one member that declares `from websocket` services
/// passes every upgrade to that member, which is right; with two, the first
/// would take the upgrades meant for the second. So a group has at most one such
/// member. One error, at the second.
fn group_websockets(
    key: &str,
    members: &[String],
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    errors: &mut ErrorSink,
) {
    let declares_websocket = |ctx: &String| {
        groups.get(ctx).into_iter().flatten().any(|&i| {
            matches!(parsed[i].unit(), SourceUnit::Context(c) if c.items.iter().any(|item| {
                matches!(item, CommonsItem::Service(s) if matches!(s.protocol, ServiceProtocol::WebSocket { .. }))
            }))
        })
    };
    let mut hosts = members.iter().filter(|c| declares_websocket(c));
    let (Some(first), Some(second)) = (hosts.next(), hosts.next()) else {
        return;
    };
    let indices = groups.get(second).map(Vec::as_slice).unwrap_or_default();
    let (file, span) = context_header(parsed, indices)
        .map_or((None, Span::default()), |(file, span)| (Some(file), span));
    errors.push_for(
        file.as_deref(),
        CompileError::new(
            "bynk.deploy.multiple_websocket_members",
            span,
            format!(
                "contexts `{first}` and `{second}` share Worker `{key}`, and both declare a `from websocket` service"
            ),
        )
        .with_note(
            "a WebSocket upgrade carries no path or service name for a group to route it by, so a group has at most one member that accepts them: put the contexts in different groups, or give one of them its own Worker",
        ),
    );
}

/// Where the context declares the source of `binding`: an agent's name, or
/// the `consumes` clause naming a context (or `bynk.cloudflare`, for the KV
/// namespace). `None` for a source with no declaration of its own: the events
/// fan-out, or a Service Binding added for an event subscriber.
fn declared_at(
    parsed: &[ParsedFile],
    indices: &[usize],
    binding: &WorkerBinding,
) -> Option<(PathBuf, Span)> {
    indices.iter().find_map(|&i| {
        let SourceUnit::Context(c) = parsed[i].unit() else {
            return None;
        };
        let consumed = |target: &str| {
            c.consumes
                .iter()
                .find(|d| d.target.joined() == target)
                .map(|d| d.span)
        };
        let span = match &binding.source {
            BindingSource::Agent { class_name } => c.items.iter().find_map(|item| {
                let CommonsItem::Agent(a) = item else {
                    return None;
                };
                (a.name.name == *class_name).then_some(a.name.span)
            }),
            BindingSource::Service { context } => consumed(context),
            BindingSource::Kv | BindingSource::Singleton { .. } => {
                consumed(firstparty::CLOUDFLARE_UNIT)
            }
            BindingSource::KvInstance { capability }
            | BindingSource::ForeignService { capability, .. }
            | BindingSource::R2Instance { capability, .. }
            | BindingSource::RateLimitInstance { capability, .. }
            | BindingSource::QueueProducer { capability, .. }
            | BindingSource::AnalyticsDataset { capability }
            | BindingSource::StoredSecret { capability, .. }
            | BindingSource::Email { capability, .. }
            | BindingSource::Postgres { capability }
            | BindingSource::MySql { capability, .. }
            | BindingSource::VectorizeIndex { capability, .. }
            | BindingSource::ContainerInstance { capability, .. }
            | BindingSource::AiSearchInstance { capability, .. }
            | BindingSource::MtlsCertificate { capability, .. }
            | BindingSource::D1Database { capability } => c.items.iter().find_map(|item| {
                let CommonsItem::Capability(d) = item else {
                    return None;
                };
                (d.name.name == *capability && d.derived.is_some()).then_some(d.span)
            }),
            BindingSource::EventsFanout { .. } => None,
        }?;
        Some((parsed[i].identity_path(), span))
    })
}

/// The context's name in its first file: where a clash between two sources
/// with no declaration of their own is reported.
fn context_header(parsed: &[ParsedFile], indices: &[usize]) -> Option<(PathBuf, Span)> {
    indices.iter().find_map(|&i| {
        let SourceUnit::Context(c) = parsed[i].unit() else {
            return None;
        };
        Some((parsed[i].identity_path(), c.name.span))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, context: &str) -> WorkerBinding {
        WorkerBinding {
            name: name.to_string(),
            source: BindingSource::Service {
                context: context.to_string(),
            },
        }
    }

    #[test]
    fn distinct_names_do_not_clash() {
        assert!(duplicate_bindings(&[service("A", "a"), service("B", "b")]).is_empty());
    }

    /// Three bindings on one name are two clashes, each against the first.
    #[test]
    fn every_repeat_is_reported_against_the_first() {
        let bindings = [
            service("A_B", "a.b"),
            service("A_B", "a_b"),
            service("A_B", "x"),
        ];
        let duplicates = duplicate_bindings(&bindings);
        assert_eq!(duplicates.len(), 2);
        assert!(
            duplicates
                .iter()
                .all(|(earlier, _)| *earlier == &bindings[0])
        );
        assert_eq!(duplicates[1].1, &bindings[2]);
    }

    fn kv(name: &str, capability: &str) -> WorkerBinding {
        WorkerBinding {
            name: name.to_string(),
            source: BindingSource::KvInstance {
                capability: capability.to_string(),
            },
        }
    }

    fn r2(name: &str, jurisdiction: Option<&str>) -> WorkerBinding {
        WorkerBinding {
            name: name.to_string(),
            source: BindingSource::R2Instance {
                capability: "Archives".to_string(),
                jurisdiction: jurisdiction.map(str::to_string),
            },
        }
    }

    fn collisions(
        members: &[&str],
        bindings: &[(&str, Vec<WorkerBinding>)],
    ) -> Vec<(String, String)> {
        let table = format!(
            "[deploy.workers.core]\ncontexts = [{}]\n",
            members
                .iter()
                .map(|m| format!("\"{m}\""))
                .collect::<Vec<_>>()
                .join(", ")
        );
        let root = std::path::Path::new("/project");
        let overlay = HashMap::from([(root.join("bynk.toml"), table)]);
        let worker_groups =
            bynk_project::worker_groups::try_read_worker_groups_with(root, &overlay).unwrap();
        let bindings: BTreeMap<String, Vec<WorkerBinding>> = bindings
            .iter()
            .map(|(ctx, b)| (ctx.to_string(), b.clone()))
            .collect();
        let mut errors = ErrorSink::new();
        phase_group_binding_collisions(
            &worker_groups,
            &[],
            &BTreeMap::new(),
            &bindings,
            &mut errors,
        );
        errors
            .into_all()
            .into_iter()
            .map(|e| (e.error.category.to_string(), e.error.message))
            .collect()
    }

    /// Identical declarations in two members are one binding.
    #[test]
    fn identical_declarations_share_one_binding() {
        assert!(
            collisions(
                &["a.x", "a.y"],
                &[
                    ("a.x", vec![kv("KV_MAIN", "Main")]),
                    ("a.y", vec![kv("KV_MAIN", "Main")]),
                ],
            )
            .is_empty()
        );
    }

    /// The same name with different options is a collision, naming both members.
    #[test]
    fn differing_declarations_collide() {
        let found = collisions(
            &["a.x", "a.y"],
            &[
                ("a.x", vec![r2("ARCHIVES", None)]),
                ("a.y", vec![r2("ARCHIVES", Some("eu"))]),
            ],
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "bynk.deploy.binding_collision");
        assert!(
            found[0].1.contains("`a.x` and `a.y` share Worker `core`"),
            "{}",
            found[0].1
        );
    }

    /// A name one member repeats is the existing per-context error, not this one;
    /// a context outside the group is not checked against it.
    #[test]
    fn only_clashes_between_members_are_reported() {
        assert!(
            collisions(
                &["a.x", "a.y"],
                &[
                    (
                        "a.x",
                        vec![r2("ARCHIVES", None), r2("ARCHIVES", Some("eu"))]
                    ),
                    ("a.z", vec![r2("ARCHIVES", Some("eu"))]),
                ],
            )
            .is_empty()
        );
    }

    fn agent(name: &str) -> WorkerBinding {
        WorkerBinding {
            name: format!("AGENT_{}", name.to_uppercase()),
            source: BindingSource::Agent {
                class_name: name.to_string(),
            },
        }
    }

    /// An agent binding holds only the class name, so equal bindings can be two
    /// different classes: two members each declaring `Cart` collide, though
    /// their bindings compare equal.
    #[test]
    fn same_named_agents_in_two_members_collide() {
        let found = collisions(
            &["a.x", "a.y"],
            &[("a.x", vec![agent("Cart")]), ("a.y", vec![agent("Cart")])],
        );
        assert_eq!(found.len(), 1, "{found:?}");
        assert_eq!(found[0].0, "bynk.deploy.binding_collision");
        // Agents of different names do not.
        assert!(
            collisions(
                &["a.x", "a.y"],
                &[("a.x", vec![agent("Cart")]), ("a.y", vec![agent("Order")])],
            )
            .is_empty()
        );
    }
}
