//! One binding model per build (back-half paydown 1C.1a, Decisions A to E of
//! #4031).
//!
//! [`WorkerBindings`] holds every binding each context's Worker declares, in
//! the order its `wrangler.toml` lists them, and each Worker's own list (its
//! members' merged). It is derived once from the [`ComposePlan`] and the
//! checked unit tables by [`WorkerBindings::derive`]; nothing else computes
//! which bindings a Worker has.
//!
//! The vocabulary ([`BindingSource`], [`WorkerBinding`]) stays in
//! `bynk-check`, which needs it for the duplicate-binding diagnostics. The
//! model, its derivation and its order are the emitter's. The order is a
//! stable sort by `rank`, one exhaustive `match`, so a new kind of binding
//! fails to compile until it is placed, rather than being silently left out of
//! `wrangler.toml`.

use std::collections::{BTreeMap, HashMap, HashSet};

use bynk_check::firstparty;
use bynk_check::symbols::{DerivedInstance, UnitTable};
use bynk_project::worker_layout::WorkerLayout;

pub use bynk_check::worker_bindings::{BindingSource, WorkerBinding};

use super::compose::{ComposePlan, ComposeUnit};
use super::wrangler::EVENTS_FANOUT_CLASS_NAME;

/// Every binding of every Worker in a build.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkerBindings {
    /// Each context's bindings, in the order its `wrangler.toml` lists them.
    contexts: BTreeMap<String, Vec<WorkerBinding>>,
    /// Each Worker's (by [`WorkerLayout::dir`]): its members' merged, an
    /// identical declaration one binding (#3562), in the manifest's member
    /// order.
    workers: BTreeMap<String, Vec<WorkerBinding>>,
}

impl WorkerBindings {
    /// Derive every context's bindings from the plan's facts and the unit
    /// tables' agents and derived instances, then each Worker's.
    pub fn derive(
        plan: &ComposePlan,
        unit_tables: &HashMap<String, UnitTable>,
        layout: &WorkerLayout,
    ) -> Self {
        let mut contexts = BTreeMap::new();
        for (name, unit) in plan.contexts() {
            let Some(table) = unit_tables.get(name) else {
                continue;
            };
            contexts.insert(name.clone(), context_bindings(name, unit, table, plan));
        }
        let workers = workers_of(&contexts, layout);
        Self { contexts, workers }
    }

    /// A model holding exactly `workers`' bindings, each keyed as both a
    /// context and a Worker directory: what a reader of the model is tested
    /// against, without compiling a project.
    #[doc(hidden)]
    pub fn of_workers(workers: BTreeMap<String, Vec<WorkerBinding>>) -> Self {
        Self {
            contexts: workers.clone(),
            workers,
        }
    }

    /// The bindings of context `context`'s Worker; none for a name that is not
    /// a context.
    pub fn of(&self, context: &str) -> &[WorkerBinding] {
        self.contexts.get(context).map_or(&[], Vec::as_slice)
    }

    /// The bindings of the Worker in directory `dir`: its members' merged.
    pub fn of_worker(&self, dir: &str) -> &[WorkerBinding] {
        self.workers.get(dir).map_or(&[], Vec::as_slice)
    }

    /// Every context's bindings, by context name: what the duplicate and group
    /// checks read, and what the integration harness binds on each
    /// participant's `env`.
    pub fn by_context(&self) -> &BTreeMap<String, Vec<WorkerBinding>> {
        &self.contexts
    }

    /// #3477: a container's image is built from a directory of the project,
    /// which `wrangler` reaches from the Worker's output directory. Sets each
    /// container binding's path back to the project root, in every context's
    /// list and every Worker's, once for the build.
    pub fn locate_containers(&mut self, root_from_worker: &str) {
        for binding in self.contexts.values_mut().chain(self.workers.values_mut()) {
            for binding in binding {
                if let BindingSource::ContainerInstance {
                    root_from_worker: slot,
                    ..
                } = &mut binding.source
                {
                    root_from_worker.clone_into(slot);
                }
            }
        }
    }
}

/// Context `name`'s bindings: what it calls, what subscribes to its events, the
/// bare resources its closure reaches, its derived instances, the implicit
/// idempotency database, and its agents and fan-out.
fn context_bindings(
    name: &str,
    unit: &ComposeUnit,
    table: &UnitTable,
    plan: &ComposePlan,
) -> Vec<WorkerBinding> {
    // A Worker holds a Service Binding to each context it calls, and to each
    // context subscribing to an event it publishes (Events track, slice 0,
    // spine #936, ADR 0284): the reverse direction of an ordinary `consumes`
    // edge, which nothing upstream gives the publisher. #1961: a consumed
    // context is bound only when called, never merely for being consumed, or a
    // subscriber and its publisher would bind each other.
    let mut targets: Vec<String> = unit.called.clone();
    targets.extend(
        plan.events
            .published_by(name)
            .flat_map(|(_, subs)| subs.iter().map(|s| s.context.clone())),
    );
    targets.sort();
    targets.dedup();
    let agents: Vec<String> = table.agents.keys().cloned().collect();
    // The instances in capability order; the implicit idempotency database
    // after them.
    let mut instances: Vec<(&String, WorkerBinding)> = table
        .derived
        .iter()
        // L7 (#2414): Workers Cache is a Worker-wide `[cache]` setting, not an
        // `env` binding.
        .filter(|(_, d)| !d.is_edge_cache())
        .map(|(cap, d)| (cap, instance_binding(cap, d)))
        .collect();
    instances.sort_by(|a, b| a.0.cmp(b.0));
    let mut instances: Vec<WorkerBinding> = instances.into_iter().map(|(_, b)| b).collect();
    instances.extend(dead_letter_bindings(table));
    if unit.idempotency {
        instances.push(bynk_check::worker_bindings::idempotency_binding());
    }
    let singletons: Vec<WorkerBinding> = unit
        .bare_singletons
        .iter()
        .map(|c| singleton_binding(c))
        .collect();
    ordered(
        &agents,
        &targets,
        unit.bare_kv,
        instances,
        singletons,
        unit.publishes,
    )
}

/// #3878: the producer binding of each queue consumer's dead-letter queue, in queue order, by the
/// consumer's own name (see [`dead_letter_binding`]).
fn dead_letter_bindings(table: &UnitTable) -> Vec<WorkerBinding> {
    bynk_check::symbols::queue_deliveries(table)
        .into_iter()
        .filter_map(|d| {
            let queue = d.dead_letter?;
            Some(WorkerBinding {
                name: crate::emitter::workers_entry::dead_letter_binding(&d.queue),
                source: BindingSource::QueueDeadLetter { queue },
            })
        })
        .collect()
}

/// The binding of derived instance `cap`, by its base capability: every
/// instantiable kind by name, with no catch-all, so a new kind cannot silently
/// bind as a KV namespace (P3, #2342).
fn instance_binding(cap: &str, d: &DerivedInstance) -> WorkerBinding {
    let capability = cap.to_string();
    let source = match (d.base_unit.as_str(), d.base_cap.as_str()) {
        (firstparty::CLOUDFLARE_UNIT, "Service") => BindingSource::ForeignService {
            capability,
            worker: d.worker().unwrap_or_default().to_string(),
        },
        (firstparty::CLOUDFLARE_UNIT, "R2") => BindingSource::R2Instance {
            capability,
            jurisdiction: d.jurisdiction().map(str::to_string),
        },
        (firstparty::CLOUDFLARE_UNIT, "RateLimit") => {
            let (limit, period) = d.rate_limit().unwrap_or_default();
            BindingSource::RateLimitInstance {
                capability,
                limit,
                period,
            }
        }
        (firstparty::CLOUDFLARE_UNIT, "Analytics") => {
            BindingSource::AnalyticsDataset { capability }
        }
        (firstparty::CLOUDFLARE_UNIT, "D1") => BindingSource::D1Database { capability },
        (firstparty::CLOUDFLARE_UNIT, "Queue") => BindingSource::QueueProducer {
            capability,
            queue: d.queue().unwrap_or_default().to_string(),
        },
        (firstparty::CLOUDFLARE_UNIT, "StoredSecret") => BindingSource::StoredSecret {
            capability,
            store: d.store().unwrap_or_default().to_string(),
            secret: d.stored_secret().unwrap_or_default().to_string(),
        },
        (firstparty::CLOUDFLARE_UNIT, "Postgres") => BindingSource::Postgres { capability },
        (firstparty::CLOUDFLARE_UNIT, "MySql") => BindingSource::MySql {
            capability,
            tls: d.tls().map(str::to_string),
        },
        (firstparty::CLOUDFLARE_UNIT, "Email") => BindingSource::Email {
            capability,
            to: d.email_to().map(str::to_string),
            allowed_recipients: d.email_allowed_recipients().to_vec(),
            allowed_senders: d.email_allowed_senders().to_vec(),
        },
        (firstparty::CLOUDFLARE_UNIT, "Vectorize") => BindingSource::VectorizeIndex {
            capability,
            dimensions: d.vectorize_dimensions().unwrap_or_default(),
            metric: d.vectorize_metric().unwrap_or_default().to_string(),
            indexed: d.vectorize_indexed().to_vec(),
        },
        (firstparty::CLOUDFLARE_UNIT, "Container") => BindingSource::ContainerInstance {
            class_name: bynk_check::containers::container_class_name(&capability),
            capability,
            build: bynk_check::containers::normalise_build(d.container_build().unwrap_or_default()),
            root_from_worker: String::new(),
            port: d.container_port().unwrap_or_default(),
            sleep_after: d.container_sleep_after(),
            instance_type: d.container_instance_type().map(str::to_string),
            forward: d.container_forward().to_vec(),
        },
        (firstparty::CLOUDFLARE_UNIT, "AiSearch") => BindingSource::AiSearchInstance {
            capability,
            instance: d.ai_search_instance().unwrap_or_default().to_string(),
        },
        (firstparty::CLOUDFLARE_UNIT, "MtlsCertificate") => BindingSource::MtlsCertificate {
            capability,
            certificate: d.mtls_certificate().unwrap_or_default().to_string(),
        },
        (firstparty::CLOUDFLARE_UNIT, "Kv") => BindingSource::KvInstance { capability },
        (unit, base) => {
            unreachable!("`{unit}.{base}` is instantiable but has no binding kind")
        }
    };
    WorkerBinding {
        name: d.binding.clone(),
        source,
    }
}

/// The order of the bindings of one Worker: Service Bindings by target, then
/// those to foreign Workers by capability, then the KV namespaces, the other
/// instances by kind, the one-per-Worker bindings, and the Durable Objects by
/// class name. `wrangler.toml` lists exactly this order, the Worker's `Env`
/// interface types it, and the duplicate check reads it, so a new kind of
/// binding is declared, typed and checked from one place (#1944). Each kind
/// derives its name independently, so two can meet (an agent `Kv` and the KV
/// namespace are both `KV`); names are never adjusted to avoid that, because
/// deployed Workers depend on them. The duplicate check runs first.
///
/// `agents` are the context's agents (any order), `targets` the contexts it
/// binds (any order), `instances` its derived instances in capability order and
/// `singletons` the one-per-Worker bindings in capability order.
fn ordered(
    agents: &[String],
    targets: &[String],
    bare_kv: bool,
    instances: Vec<WorkerBinding>,
    singletons: Vec<WorkerBinding>,
    publishes: bool,
) -> Vec<WorkerBinding> {
    let mut targets: Vec<&String> = targets.iter().collect();
    targets.sort();
    let mut bindings: Vec<WorkerBinding> =
        targets.into_iter().map(|t| service_binding(t)).collect();
    if bare_kv {
        bindings.push(kv_binding());
    }
    bindings.extend(instances);
    bindings.extend(singletons);
    // Events track, slice 0 (spine #936, ADR 0284): a context whose handlers
    // emit gets its own fan-out DO among the agents', in the order of the
    // class names. Sources by provenance, not by name: an agent that happened
    // to share the fan-out's class name is still an agent.
    let mut objects: Vec<(&str, WorkerBinding)> = agents
        .iter()
        .map(|class_name| (class_name.as_str(), agent_binding(class_name)))
        .collect();
    if publishes {
        objects.push((EVENTS_FANOUT_CLASS_NAME, events_fanout_binding()));
    }
    objects.sort_by(|a, b| a.0.cmp(b.0));
    bindings.extend(objects.into_iter().map(|(_, binding)| binding));
    // Stable, so inside a rank the input's order stands: services by target,
    // instances by capability, the implicit database after the others.
    bindings.sort_by_key(|b| rank(&b.source));
    bindings
}

/// Where a kind of binding falls in a Worker's list. One exhaustive `match`:
/// a new [`BindingSource`] does not compile until it has a place.
fn rank(source: &BindingSource) -> u8 {
    match source {
        BindingSource::Service { .. } => 0,
        // Interop I5 (#1972): Service Bindings to foreign Workers, beside the
        // ones to project contexts.
        BindingSource::ForeignService { .. } => 1,
        BindingSource::Kv => 2,
        BindingSource::KvInstance { .. } => 3,
        // P5 (#2323): the R2 buckets, after the KV namespaces.
        BindingSource::R2Instance { .. } => 4,
        // P3 (#2342): the rate limiters, after the buckets.
        BindingSource::RateLimitInstance { .. } => 5,
        // P2 (#2578): the queue producers, after the rate limiters.
        BindingSource::QueueProducer { .. } | BindingSource::QueueDeadLetter { .. } => 6,
        // P3b (#2582): the Analytics Engine datasets, after the producers.
        BindingSource::AnalyticsDataset { .. } => 7,
        // P4a (#2602): the D1 databases, after the datasets.
        BindingSource::D1Database { .. } => 8,
        // #3301: the Secrets Store secrets, after the databases.
        BindingSource::StoredSecret { .. } => 9,
        // Hyperdrive H1 (#3318): after the Secrets Store secrets.
        BindingSource::Postgres { .. } | BindingSource::MySql { .. } => 10,
        // #3302: the outbound email bindings.
        BindingSource::Email { .. } => 11,
        // #3479: the Vectorize indexes, after the email bindings.
        BindingSource::VectorizeIndex { .. } => 12,
        // #3477: the containers' Durable Object namespaces, after the indexes.
        BindingSource::ContainerInstance { .. } => 13,
        // #3480: the AI Search instances.
        BindingSource::AiSearchInstance { .. } => 14,
        // #3513 A: the mTLS certificates, after the AI Search instances.
        BindingSource::MtlsCertificate { .. } => 15,
        // #3300: the one-per-Worker bindings.
        BindingSource::Singleton { .. } => 16,
        // The agents' Durable Objects and the events fan-out, by class name.
        BindingSource::Agent { .. } | BindingSource::EventsFanout { .. } => 17,
    }
}

// #1958: the constructors below are the only callers of the two name
// functions, so every place that declares, types or reads a binding (the
// emitted `env.X`, the fan-out's routing table, the integration harness)
// gets its name from the one constructor for its kind. A constructor is
// `pub(crate)` exactly when something outside this file reads its name.

/// The Service Binding to context `target`'s Worker.
pub fn service_binding(target: &str) -> WorkerBinding {
    WorkerBinding {
        name: consumed_binding_name(target),
        source: BindingSource::Service {
            context: target.to_string(),
        },
    }
}

/// The bare KV namespace binding the cloudflare adapter's `Kv` reads. Its one
/// TypeScript read is the default of `WorkersKv`'s `binding` parameter in
/// `bynk-std`'s `cloudflare.binding.ts`, pinned to `KV_BINDING_NAME` by a test.
pub(crate) fn kv_binding() -> WorkerBinding {
    WorkerBinding {
        name: bynk_check::firstparty::KV_BINDING_NAME.to_string(),
        source: BindingSource::Kv,
    }
}

/// #3300: the binding of `bynk.cloudflare`'s one-per-Worker capability
/// `capability` (`Version` → `CF_VERSION_METADATA`), by the registry's fixed
/// name. Its one TypeScript read is the default of its provider's `binding`
/// parameter in `cloudflare.binding.ts`.
pub fn singleton_binding(capability: &str) -> WorkerBinding {
    let singleton =
        bynk_check::firstparty::singleton(bynk_check::firstparty::CLOUDFLARE_UNIT, capability)
            .unwrap_or_else(|| panic!("`{capability}` is not a one-per-Worker capability"));
    WorkerBinding {
        name: singleton.binding.to_string(),
        source: BindingSource::Singleton {
            capability: capability.to_string(),
        },
    }
}

/// The Durable Object binding of agent `class_name`.
pub(crate) fn agent_binding(class_name: &str) -> WorkerBinding {
    WorkerBinding {
        name: agent_binding_name(class_name),
        source: BindingSource::Agent {
            class_name: class_name.to_string(),
        },
    }
}

/// The Durable Object binding of the events fan-out class.
pub(crate) fn events_fanout_binding() -> WorkerBinding {
    WorkerBinding {
        name: agent_binding_name(EVENTS_FANOUT_CLASS_NAME),
        source: BindingSource::EventsFanout {
            class_name: EVENTS_FANOUT_CLASS_NAME.to_string(),
        },
    }
}

/// Service Binding identifier for a consumed context: uppercase with
/// underscores. `commerce.payment` → `COMMERCE_PAYMENT`.
fn consumed_binding_name(target: &str) -> String {
    target.replace('.', "_").to_uppercase()
}

/// Durable Object binding identifier for an agent class. We use the
/// class name in screaming snake case so handlers can grab it by a
/// predictable name (`OrderEntity` → `ORDER_ENTITY`), the casing derived
/// capabilities' bindings share.
fn agent_binding_name(class_name: &str) -> String {
    bynk_check::worker_bindings::screaming_snake(class_name)
}

/// #1950: the bindings a handler's `deps.env` type exposes on Workers — the
/// projection of the Worker's [`WorkerBindings`] list that handlers reach directly, which
/// `BindingSource`'s variant docs state per kind. `consumed` is the consumed
/// contexts the handler calls (#1961), `agents` every local agent when the
/// handler constructs one (else empty); order as in the Worker's list.
///
/// bynk_ts::TsExpr by the same constructors, so each name and source is the Worker's
/// own: a called context is never an adapter (an adapter may not declare a
/// `service`, `bynk.adapter.disallowed_item`), and the Worker binds every
/// context any of its bodies calls, so it has a Service Binding; every local
/// agent has a Durable Object binding. The
/// KV namespace, the events fan-out and subscribers' Service Bindings never
/// reach handlers.
pub(crate) fn handler_bindings(consumed: &[String], agents: &[String]) -> Vec<WorkerBinding> {
    let mut consumed: Vec<&String> = consumed.iter().collect();
    consumed.sort();
    let mut agents: Vec<&String> = agents.iter().collect();
    agents.sort();
    consumed
        .into_iter()
        .map(|t| service_binding(t))
        .chain(agents.into_iter().map(|a| agent_binding(a)))
        .collect()
}

/// Each Worker's bindings: its members' merged, an identical declaration one
/// binding (#3562). A group's members are in the manifest's order; every other
/// context is its own Worker.
fn workers_of(
    contexts: &BTreeMap<String, Vec<WorkerBinding>>,
    layout: &WorkerLayout,
) -> BTreeMap<String, Vec<WorkerBinding>> {
    let mut by_dir: BTreeMap<String, Vec<&str>> = BTreeMap::new();
    for ctx in contexts.keys() {
        by_dir.entry(layout.dir(ctx)).or_default().push(ctx);
    }
    by_dir
        .into_keys()
        .map(|dir| {
            let order: Vec<String> = layout.group_members(&dir).map_or_else(
                || {
                    contexts
                        .keys()
                        .filter(|c| layout.dir(c) == dir)
                        .cloned()
                        .collect()
                },
                <[String]>::to_vec,
            );
            let merged = merge_group_bindings(
                order
                    .iter()
                    .filter_map(|c| contexts.get(c))
                    .map(Vec::as_slice),
            );
            (dir, merged)
        })
        .collect()
}

/// The bindings of one Worker from its members' (#3562): each name once, at its
/// first declaration. Two members declaring one name is one binding when the
/// declarations are identical; when they differ it is the error
/// [`bynk_check::worker_bindings::phase_group_binding_collisions`] reports, so a build that reaches the emit
/// has none.
fn merge_group_bindings<'a>(
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

#[cfg(test)]
mod tests {
    use super::*;

    fn names(bindings: &[WorkerBinding]) -> Vec<&str> {
        bindings.iter().map(|b| b.name.as_str()).collect()
    }

    fn instance(name: &str, source: BindingSource) -> WorkerBinding {
        WorkerBinding {
            name: name.to_string(),
            source,
        }
    }

    fn cap(c: &str) -> String {
        c.to_string()
    }

    /// The fan-out's binding name, which its class name (`__BynkEventsFanout`)
    /// gives a leading `___`.
    fn fanout() -> String {
        events_fanout_binding().name
    }

    #[test]
    fn services_are_in_target_order_and_before_everything_else() {
        let bindings = ordered(
            &[cap("Cart")],
            &[cap("shop.orders"), cap("shop.billing")],
            true,
            vec![],
            vec![],
            false,
        );
        assert_eq!(
            names(&bindings),
            ["SHOP_BILLING", "SHOP_ORDERS", "KV", "CART"]
        );
    }

    #[test]
    fn every_kind_is_in_emission_order() {
        let at = |name: &str, source| instance(name, source);
        // Fed in capability order, as `context_bindings` feeds them: the sort
        // alone puts the kinds in rank order.
        let instances = vec![
            at(
                "MTLS_A",
                BindingSource::MtlsCertificate {
                    capability: cap("A"),
                    certificate: cap("c"),
                },
            ),
            at(
                "AI_B",
                BindingSource::AiSearchInstance {
                    capability: cap("B"),
                    instance: cap("i"),
                },
            ),
            at(
                "CONTAINER_C",
                BindingSource::ContainerInstance {
                    capability: cap("C"),
                    class_name: cap("C"),
                    build: cap("."),
                    root_from_worker: String::new(),
                    port: 80,
                    sleep_after: None,
                    instance_type: None,
                    forward: vec![],
                },
            ),
            at(
                "VEC_D",
                BindingSource::VectorizeIndex {
                    capability: cap("D"),
                    dimensions: 3,
                    metric: cap("cosine"),
                    indexed: vec![],
                },
            ),
            at(
                "MAIL_E",
                BindingSource::Email {
                    capability: cap("E"),
                    to: None,
                    allowed_recipients: vec![],
                    allowed_senders: vec![],
                },
            ),
            at(
                "PG_F",
                BindingSource::Postgres {
                    capability: cap("F"),
                },
            ),
            at(
                "SECRET_G",
                BindingSource::StoredSecret {
                    capability: cap("G"),
                    store: cap("s"),
                    secret: cap("x"),
                },
            ),
            at(
                "D1_H",
                BindingSource::D1Database {
                    capability: cap("H"),
                },
            ),
            bynk_check::worker_bindings::idempotency_binding(),
            at(
                "DS_I",
                BindingSource::AnalyticsDataset {
                    capability: cap("I"),
                },
            ),
            at(
                "Q_J",
                BindingSource::QueueProducer {
                    capability: cap("J"),
                    queue: cap("q"),
                },
            ),
            at(
                "RL_K",
                BindingSource::RateLimitInstance {
                    capability: cap("K"),
                    limit: 1,
                    period: 60,
                },
            ),
            at(
                "R2_L",
                BindingSource::R2Instance {
                    capability: cap("L"),
                    jurisdiction: None,
                },
            ),
            at(
                "KV_M",
                BindingSource::KvInstance {
                    capability: cap("M"),
                },
            ),
            at(
                "FOREIGN_N",
                BindingSource::ForeignService {
                    capability: cap("N"),
                    worker: cap("w"),
                },
            ),
        ];
        let bindings = ordered(
            &[cap("Zed"), cap("Alpha")],
            &[cap("shop.a")],
            true,
            instances,
            vec![singleton_binding("Version")],
            true,
        );
        assert_eq!(
            names(&bindings),
            [
                "SHOP_A",
                "FOREIGN_N",
                "KV",
                "KV_M",
                "R2_L",
                "RL_K",
                "Q_J",
                "DS_I",
                "D1_H",
                // The implicit idempotency database is last among the
                // databases, as it is pushed after the sorted instances.
                bynk_check::d1_migrations::IDEMPOTENCY_BINDING,
                "SECRET_G",
                "PG_F",
                "MAIL_E",
                "VEC_D",
                "CONTAINER_C",
                "AI_B",
                "MTLS_A",
                "CF_VERSION_METADATA",
                "ALPHA",
                "ZED",
                fanout().as_str(),
            ]
        );
    }

    #[test]
    fn an_agent_sharing_the_fanout_class_name_is_still_an_agent() {
        let bindings = ordered(
            &[cap(EVENTS_FANOUT_CLASS_NAME)],
            &[],
            false,
            vec![],
            vec![],
            true,
        );
        assert_eq!(bindings.len(), 2);
        assert!(matches!(bindings[0].source, BindingSource::Agent { .. }));
        assert!(matches!(
            bindings[1].source,
            BindingSource::EventsFanout { .. }
        ));
    }

    /// #1950: the handler projection is drawn from the Worker's own list, each
    /// binding identical (name and source, a Service Binding and an agent's
    /// alike), and leaves out the KV namespace and the events fan-out.
    #[test]
    fn handler_bindings_are_a_subset_of_the_worker_bindings() {
        let worker = ordered(
            &[cap("OrderEntity")],
            &[cap("shop.payment"), cap("audit")],
            true,
            vec![],
            vec![],
            true,
        );
        let handler = handler_bindings(&[cap("shop.payment")], &[cap("OrderEntity")]);
        assert_eq!(names(&handler), ["SHOP_PAYMENT", "ORDER_ENTITY"]);
        for binding in &handler {
            assert!(worker.contains(binding), "{binding:?} not in {worker:?}");
        }
    }

    /// A consumed context `kv` and the KV namespace are both `KV`: the later
    /// one is paired with the earlier.
    #[test]
    fn a_shared_name_pairs_the_later_binding_with_the_earlier() {
        let bindings = ordered(&[], &[cap("kv")], true, vec![], vec![], false);
        let duplicates = bynk_check::worker_bindings::duplicate_bindings(&bindings);
        assert_eq!(duplicates.len(), 1);
        let (earlier, later) = duplicates[0];
        assert_eq!(
            earlier.source,
            BindingSource::Service { context: cap("kv") }
        );
        assert_eq!(later.source, BindingSource::Kv);
    }

    /// #3562: a name two members of a group both declare is one binding, at its
    /// first declaration.
    #[test]
    fn a_group_merges_an_identical_declaration_into_one_binding() {
        let kv = |name: &str| {
            instance(
                name,
                BindingSource::KvInstance {
                    capability: cap(name),
                },
            )
        };
        let first = vec![kv("A"), kv("B")];
        let second = vec![kv("B"), kv("C")];
        let merged = merge_group_bindings([first.as_slice(), second.as_slice()]);
        assert_eq!(names(&merged), ["A", "B", "C"]);
    }

    fn plan_with(units: Vec<(&str, super::super::compose::ComposeUnitFacts)>) -> ComposePlan {
        ComposePlan::decide(&super::super::compose::ComposeFacts {
            units: units.into_iter().map(|(n, f)| (n.to_string(), f)).collect(),
            ..Default::default()
        })
    }

    fn context(
        f: impl FnOnce(&mut super::super::compose::ComposeUnitFacts),
    ) -> super::super::compose::ComposeUnitFacts {
        let mut facts = super::super::compose::ComposeUnitFacts {
            is_context: true,
            has_services: true,
            ..Default::default()
        };
        f(&mut facts);
        facts
    }

    #[test]
    fn derive_reads_the_plans_facts() {
        let plan = plan_with(vec![
            (
                "shop",
                context(|f| {
                    f.called = vec![cap("billing")];
                    f.bare_kv = true;
                    f.bare_singletons = vec![cap("Version")];
                    f.idempotency = true;
                    f.publishes = true;
                }),
            ),
            ("billing", context(|_| {})),
        ]);
        let tables: HashMap<String, UnitTable> = [
            (cap("shop"), UnitTable::default()),
            (cap("billing"), UnitTable::default()),
        ]
        .into();
        let model = WorkerBindings::derive(&plan, &tables, &WorkerLayout::identity());
        assert_eq!(
            names(model.of("shop")),
            [
                "BILLING",
                "KV",
                bynk_check::d1_migrations::IDEMPOTENCY_BINDING,
                "CF_VERSION_METADATA",
                fanout().as_str(),
            ]
        );
        assert!(model.of("billing").is_empty());
        assert!(model.of("nowhere").is_empty());
        assert_eq!(model.by_context().len(), 2);
        assert_eq!(model.of_worker("shop"), model.of("shop"));
    }

    #[test]
    fn an_event_subscriber_is_bound_by_its_publisher() {
        let mut plan = plan_with(vec![
            ("shop", context(|f| f.publishes = true)),
            ("audit", context(|_| {})),
        ]);
        plan.events.subscriptions.insert(
            (cap("shop"), cap("OrderPlaced")),
            vec![super::super::compose::EventSubscription {
                context: cap("audit"),
                service: cap("Audit"),
                wants_envelope: false,
            }],
        );
        let tables: HashMap<String, UnitTable> = [
            (cap("shop"), UnitTable::default()),
            (cap("audit"), UnitTable::default()),
        ]
        .into();
        let model = WorkerBindings::derive(&plan, &tables, &WorkerLayout::identity());
        assert_eq!(names(model.of("shop")), ["AUDIT", fanout().as_str()]);
        // The reverse direction is not bound: a cycle no upload order satisfies.
        assert!(model.of("audit").is_empty());
    }

    #[test]
    fn locating_containers_reaches_every_list() {
        let container = instance(
            "BOX",
            BindingSource::ContainerInstance {
                capability: cap("Box"),
                class_name: cap("Box"),
                build: cap("."),
                root_from_worker: String::new(),
                port: 80,
                sleep_after: None,
                instance_type: None,
                forward: vec![],
            },
        );
        let mut model = WorkerBindings {
            contexts: [(cap("shop"), vec![container.clone()])].into(),
            workers: [(cap("shop"), vec![container])].into(),
        };
        model.locate_containers("../../..");
        for list in [model.of("shop"), model.of_worker("shop")] {
            let BindingSource::ContainerInstance {
                root_from_worker, ..
            } = &list[0].source
            else {
                panic!("a container");
            };
            assert_eq!(root_from_worker, "../../..");
        }
    }
}
