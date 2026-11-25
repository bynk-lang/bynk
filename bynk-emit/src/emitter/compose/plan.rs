//! The composition plan (back-half paydown 1B.1b, Decisions C, D and E of
//! #3928): what an application composes, with no spelling and no per-target
//! entry order.
//!
//! Two layers. [`ComposeFacts::from_tables`] is the thin builder: it reads the
//! checked project tables (`UnitTable`, `exec_plan`, the consume and flatten
//! maps) into plain data. [`ComposePlan::decide`] is a pure function over that
//! data, so a composition decision is unit-tested with hand-built
//! [`ComposeFacts`] and no fixture.
//!
//! The plan records the units, each unit's dependency edges and keys, which
//! units need `__exec` or the caller, and the cross-capability facts. It holds
//! no binding type (Decision D of #3928). It does record the facts that imply
//! a binding (1C.1a, Decision C of #4031): the contexts a unit calls, whether
//! its closure reaches the bare KV namespace, the bare one-per-Worker
//! capabilities it reaches, and whether it calls `Idempotency`.
//! `WorkerBindings::derive` reads those facts and nothing else about the
//! checked program's calls.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use bynk_check::checker::{Callee, ExprId};
use bynk_check::firstparty;
use bynk_check::project_model::{
    self, composed_in_bundle_root, handler_cross_caps, provider_cross_caps,
};
use bynk_check::symbols::UnitTable;
use bynk_model::EventSubscriberShape;
use bynk_project::roots::UnitKind;

use super::closure::{bare_singletons_of_context, native_platforms_of_context};
use super::events::{EventPlan, EventSubscription};

/// Whether a unit's service handlers hand work to the execution context
/// (`detach`, `within` or `race`, #2719). The two targets read different
/// predicates, so both are facts (divergence 4 of #3928).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExecNeeds {
    /// Any service handler needs it: the Workers `compose` takes the request's
    /// execution context and threads it into `deps.__exec`.
    pub any_handler: bool,
    /// An `on call` handler needs it: the Bundle root hands `__exec` to the
    /// surface's `call` handlers only.
    pub call_handler: bool,
}

/// What the builder reads off one unit's checked table. Plain data.
#[derive(Debug, Clone, Default)]
pub struct ComposeUnitFacts {
    pub is_context: bool,
    /// The units it consumes, in declaration order.
    pub consumes: Vec<String>,
    /// Alias -> consumed unit.
    pub aliases: HashMap<String, String>,
    pub has_services: bool,
    pub has_agents: bool,
    /// A service declares an `on call ... by c: Caller` handler (ADR 0092).
    pub binds_caller: bool,
    /// A handler publishes an event (`Events.emit`).
    pub publishes: bool,
    pub exec: ExecNeeds,
    /// Own providers' capability keys, in dependency order.
    pub providers: Vec<String>,
    /// Derived capabilities' keys (S4a), sorted.
    pub derived: Vec<String>,
    /// `deps_key -> consumed unit` for what handlers and agents `given`.
    pub handler_cross_caps: BTreeMap<String, String>,
    /// `deps_key -> consumed unit` for what own providers `given`.
    pub provider_cross_caps: BTreeMap<String, String>,
    /// The shape of each `from Events` service the unit hosts, by service name.
    pub event_shapes: BTreeMap<String, EventSubscriberShape>,
    /// The consumed contexts its bodies and agents call, in name order. A
    /// Service Binding exists for a context that is called, never merely
    /// consumed (#1961).
    pub called: Vec<String>,
    /// Its closure reaches the bare KV namespace (the cloudflare adapter's
    /// `Kv`), so its Worker declares `KV`. A named instance does not imply it.
    pub bare_kv: bool,
    /// The one-per-Worker `bynk.cloudflare` capabilities its closure reaches
    /// bare (#3300), in name order.
    pub bare_singletons: Vec<String>,
    /// A body calls `Idempotency`, so its Worker declares the implicit
    /// database that holds the claims (W30, #2840).
    pub idempotency: bool,
    /// #3899: each `Queue` instance whose consumer is signed and admits this context, by capability
    /// key, with the consumer's context: the instance signs what it sends.
    pub signed_queues: BTreeMap<String, String>,
}

/// Everything [`ComposePlan::decide`] needs, as plain data.
#[derive(Debug, Clone, Default)]
pub struct ComposeFacts {
    /// Every unit that has a checked table, by name.
    pub units: BTreeMap<String, ComposeUnitFacts>,
    /// Whether a Bundle build has a composition root at all
    /// (`composition_root_trigger`).
    pub bundle_root: bool,
    /// `bynk_check::symbols::discover_event_subscribers`' table: each event's
    /// subscribers as `(context, service)`, keyed `(publishing context, event)`.
    pub event_subscribers: BTreeMap<(String, String), Vec<(String, String)>>,
}

impl ComposeFacts {
    /// Read the facts off the checked project tables.
    #[allow(clippy::too_many_arguments)]
    pub fn from_tables(
        kinds: &BTreeMap<String, UnitKind>,
        unit_consumes: &HashMap<String, Vec<String>>,
        unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
        unit_tables: &HashMap<String, UnitTable>,
        unit_callees: &HashMap<String, HashMap<ExprId, Callee>>,
        unit_flattened: &HashMap<String, HashMap<String, String>>,
        event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
        unit_event_subscriber_shapes: &HashMap<String, HashMap<String, EventSubscriberShape>>,
    ) -> Self {
        let bundle_root = project_model::composition_root_trigger(
            kinds,
            unit_consumes,
            unit_consumes_aliases,
            unit_tables,
            unit_callees,
            unit_flattened,
        )
        .is_some();
        let mut units = BTreeMap::new();
        for (name, table) in unit_tables {
            let consumes = unit_consumes.get(name).cloned().unwrap_or_default();
            let aliases = unit_consumes_aliases.get(name).cloned().unwrap_or_default();
            let flattened = unit_flattened.get(name).cloned().unwrap_or_default();
            let mut derived: Vec<String> = table.derived.keys().cloned().collect();
            derived.sort();
            let handlers = || table.services.values().flat_map(|s| s.handlers.iter());
            let is_context = kinds.get(name) == Some(&UnitKind::Context);
            let (called, bare_kv, bare_singletons) = if is_context {
                binding_facts(
                    name,
                    table,
                    kinds,
                    (&consumes, &aliases),
                    (
                        unit_tables,
                        unit_consumes,
                        unit_consumes_aliases,
                        unit_flattened,
                    ),
                    unit_callees.get(name),
                )
            } else {
                Default::default()
            };
            units.insert(
                name.clone(),
                ComposeUnitFacts {
                    is_context,
                    has_services: composed_in_bundle_root(table),
                    has_agents: !table.agents.is_empty(),
                    binds_caller: crate::emitter::any_service_binds_caller(
                        table.services.values(),
                        &table.actors,
                    ),
                    publishes: crate::emit_model::unit_table_uses_emit(
                        table,
                        unit_callees.get(name),
                    ),
                    exec: ExecNeeds {
                        any_handler: handlers().any(|h| table.exec_plan.handler_needs(h)),
                        call_handler: handlers()
                            .any(|h| crate::emitter::call_handler_needs_exec(&table.exec_plan, h)),
                    },
                    providers: crate::emitter::topo_order_providers(&table.providers),
                    derived,
                    handler_cross_caps: handler_cross_caps(
                        table, name, &consumes, &aliases, &flattened,
                    ),
                    provider_cross_caps: provider_cross_caps(
                        table, name, &consumes, &aliases, &flattened,
                    ),
                    event_shapes: unit_event_subscriber_shapes
                        .get(name)
                        .map(|shapes| {
                            shapes
                                .iter()
                                .map(|(s, shape)| (s.clone(), *shape))
                                .collect()
                        })
                        .unwrap_or_default(),
                    called,
                    bare_kv,
                    bare_singletons,
                    idempotency: crate::emit_model::unit_table_uses_idempotency(
                        table,
                        unit_callees.get(name),
                    ),
                    signed_queues: signed_queues(name, table, unit_tables),
                    consumes,
                    aliases,
                },
            );
        }
        Self {
            units,
            bundle_root,
            event_subscribers: event_subscribers.clone(),
        }
    }
}

/// A consumed context's service surface, as the consumer's `deps` carries it
/// on the Bundle target (divergence 6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsumedSurface {
    /// The consumer's key: the alias, else the target's last name segment.
    pub key: String,
    /// The consumed unit.
    pub target: String,
    /// The target has an `on call ... by c: Caller` handler, so the consumer
    /// needs a surface built with its own name rather than the shared one.
    pub per_caller: bool,
}

/// One unit of the plan: facts, not text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposeUnit {
    pub is_context: bool,
    /// Consumed units, in declaration order (a dependency edge list, not a
    /// rendering order).
    pub consumes: Vec<String>,
    pub has_services: bool,
    pub has_agents: bool,
    pub binds_caller: bool,
    pub publishes: bool,
    pub exec: ExecNeeds,
    /// Own providers' capability keys, in dependency order.
    pub providers: Vec<String>,
    pub derived: Vec<String>,
    pub handler_cross_caps: BTreeMap<String, String>,
    pub provider_cross_caps: BTreeMap<String, String>,
    /// Consumed contexts that have services, sorted by target.
    pub surfaces: Vec<ConsumedSurface>,
    /// For each `from Events` service the unit hosts: whether its `on event`
    /// handler is handed the event's envelope (`wants_envelope`).
    pub subscriber_envelopes: BTreeMap<String, bool>,
    /// The consumed contexts its bodies and agents call, in name order
    /// ([`ComposeUnitFacts::called`]).
    pub called: Vec<String>,
    /// Its closure reaches the bare KV namespace.
    pub bare_kv: bool,
    /// The bare one-per-Worker capabilities its closure reaches.
    pub bare_singletons: Vec<String>,
    /// A body calls `Idempotency`.
    pub idempotency: bool,
    /// The `Queue` instances that sign what they send, by capability key
    /// ([`ComposeUnitFacts::signed_queues`]).
    pub signed_queues: BTreeMap<String, String>,
}

impl ComposeUnit {
    /// Whether the `on event` handler of `service`, a `from Events` service this
    /// unit hosts, is handed the envelope. Every hosted events service has a
    /// shape, so a miss is an internal error and not a `false`: the callee would
    /// still insert its positional envelope parameter and bind `deps` into it.
    pub fn hosted_wants_envelope(&self, unit: &str, service: &str) -> bool {
        *self.subscriber_envelopes.get(service).unwrap_or_else(|| {
            panic!(
                "bynk internal error: the events service `{service}` of unit `{unit}` has no \
                 recorded subscriber shape"
            )
        })
    }

    /// Every cross-context capability the unit uses, handlers' and providers'
    /// together; a key both name keeps the handlers' source. Which of the two
    /// sets a target instantiates at the top level is the renderer's choice
    /// (divergence 3).
    pub fn cross_caps(&self) -> BTreeMap<String, String> {
        let mut all = self.handler_cross_caps.clone();
        for (key, unit) in &self.provider_cross_caps {
            all.entry(key.clone()).or_insert_with(|| unit.clone());
        }
        all
    }
}

/// What an application composes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposePlan {
    /// Every unit with a checked table.
    pub units: BTreeMap<String, ComposeUnit>,
    /// The Bundle root's contexts in dependency order, a consumed context
    /// before its consumer: those it composes (it has services). Workers have
    /// no cross-unit order, as a Service Binding reaches the other Worker.
    pub bundle_order: Vec<String>,
    /// Whether a Bundle build has a composition root.
    pub bundle_root: bool,
    /// Who subscribes to what, with each subscription's envelope decision.
    pub events: EventPlan,
}

impl ComposePlan {
    /// Build the plan from the checked project tables.
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        kinds: &BTreeMap<String, UnitKind>,
        unit_consumes: &HashMap<String, Vec<String>>,
        unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
        unit_tables: &HashMap<String, UnitTable>,
        unit_callees: &HashMap<String, HashMap<ExprId, Callee>>,
        unit_flattened: &HashMap<String, HashMap<String, String>>,
        event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
        unit_event_subscriber_shapes: &HashMap<String, HashMap<String, EventSubscriberShape>>,
    ) -> Self {
        Self::decide(&ComposeFacts::from_tables(
            kinds,
            unit_consumes,
            unit_consumes_aliases,
            unit_tables,
            unit_callees,
            unit_flattened,
            event_subscribers,
            unit_event_subscriber_shapes,
        ))
    }

    /// The units that are contexts, by name.
    pub fn contexts(&self) -> impl Iterator<Item = (&String, &ComposeUnit)> {
        self.units.iter().filter(|(_, u)| u.is_context)
    }

    /// The pure decision: units, their surfaces, and the Bundle order.
    pub fn decide(facts: &ComposeFacts) -> Self {
        let units = facts
            .units
            .iter()
            .map(|(name, f)| (name.clone(), Self::decide_unit(f, facts)))
            .collect();
        Self {
            units,
            bundle_order: bundle_order(facts),
            bundle_root: facts.bundle_root,
            events: decide_events(facts),
        }
    }

    fn decide_unit(f: &ComposeUnitFacts, facts: &ComposeFacts) -> ComposeUnit {
        ComposeUnit {
            is_context: f.is_context,
            consumes: f.consumes.clone(),
            has_services: f.has_services,
            has_agents: f.has_agents,
            binds_caller: f.binds_caller,
            publishes: f.publishes,
            exec: f.exec,
            providers: f.providers.clone(),
            derived: f.derived.clone(),
            handler_cross_caps: f.handler_cross_caps.clone(),
            provider_cross_caps: f.provider_cross_caps.clone(),
            surfaces: surfaces(f, facts),
            subscriber_envelopes: f
                .event_shapes
                .iter()
                .map(|(service, shape)| (service.clone(), shape.wants_envelope()))
                .collect(),
            called: f.called.clone(),
            bare_kv: f.bare_kv,
            bare_singletons: f.bare_singletons.clone(),
            idempotency: f.idempotency,
            signed_queues: f.signed_queues.clone(),
        }
    }
}

/// #3899: the unit's `Queue` instances that sign what they send, by capability key, each with the
/// context of the signed consumer that admits `unit`.
fn signed_queues(
    unit: &str,
    table: &UnitTable,
    unit_tables: &HashMap<String, UnitTable>,
) -> BTreeMap<String, String> {
    table
        .derived
        .iter()
        .filter(|(_, d)| d.base_cap == "Queue")
        .filter_map(|(cap, d)| {
            let consumer = bynk_check::queue_producers::signed_consumer_of(d.queue()?, unit, unit_tables)?;
            Some((cap.clone(), consumer))
        })
        .collect()
}

/// The binding facts of a context ([`ComposeUnitFacts::called`], `bare_kv` and
/// `bare_singletons`). The last two walk the provider graph, so the plan is
/// built only on a program whose earlier checks passed.
#[allow(clippy::type_complexity)]
fn binding_facts(
    name: &str,
    table: &UnitTable,
    kinds: &BTreeMap<String, UnitKind>,
    (consumes, aliases): (&[String], &HashMap<String, String>),
    (unit_tables, unit_consumes, unit_consumes_aliases, unit_flattened): (
        &HashMap<String, UnitTable>,
        &HashMap<String, Vec<String>>,
        &HashMap<String, HashMap<String, String>>,
        &HashMap<String, HashMap<String, String>>,
    ),
    callees: Option<&HashMap<ExprId, Callee>>,
) -> (Vec<String>, bool, Vec<String>) {
    let mut called: BTreeSet<String> =
        crate::emit_model::called_cross_context_services(table, consumes, callees)
            .into_keys()
            .collect();
    // A handler constructing an agent hands it its own `deps`, whose `env` type
    // names the contexts the agent calls (#1960, #1961).
    called.extend(crate::emitter::agent_cross_context_targets(
        table,
        |chain| {
            let target = aliases
                .get(chain)
                .cloned()
                .or_else(|| consumes.iter().find(|c| *c == chain).cloned())?;
            (kinds.get(&target) == Some(&UnitKind::Context)).then_some(target)
        },
    ));
    let bare_kv = native_platforms_of_context(
        name,
        table,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_flattened,
        false,
    )
    .values()
    .any(|u| u == firstparty::CLOUDFLARE_UNIT);
    let bare_singletons = bare_singletons_of_context(
        name,
        table,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_flattened,
    )
    .into_iter()
    .filter(|(unit, _)| unit == firstparty::CLOUDFLARE_UNIT)
    .map(|(_, cap)| cap)
    .collect();
    (called.into_iter().collect(), bare_kv, bare_singletons)
}

/// Each subscription takes the envelope its subscriber's shape wants. A
/// subscriber with no shape recorded is not handed one.
fn decide_events(facts: &ComposeFacts) -> EventPlan {
    let subscriptions = facts
        .event_subscribers
        .iter()
        .map(|(key, subs)| {
            let subs = subs
                .iter()
                .map(|(context, service)| EventSubscription {
                    context: context.clone(),
                    service: service.clone(),
                    wants_envelope: facts
                        .units
                        .get(context)
                        .and_then(|u| u.event_shapes.get(service))
                        .is_some_and(EventSubscriberShape::wants_envelope),
                })
                .collect();
            (key.clone(), subs)
        })
        .collect();
    EventPlan { subscriptions }
}

/// A consumed context that has services is a surface of the consumer's `deps`.
fn surfaces(f: &ComposeUnitFacts, facts: &ComposeFacts) -> Vec<ConsumedSurface> {
    let mut alias_for: HashMap<&String, &String> = HashMap::new();
    for (alias, target) in &f.aliases {
        alias_for.insert(target, alias);
    }
    let mut targets: Vec<&String> = f.consumes.iter().collect();
    targets.sort();
    targets
        .into_iter()
        .filter_map(|t| {
            let other = facts.units.get(t)?;
            if !other.has_services {
                return None;
            }
            let key = alias_for
                .get(t)
                .map(|a| (*a).clone())
                .unwrap_or_else(|| t.rsplit('.').next().unwrap_or(t.as_str()).to_string());
            Some(ConsumedSurface {
                key,
                target: t.clone(),
                per_caller: other.binds_caller,
            })
        })
        .collect()
}

/// A depth-first walk over `consumes`, so a consumed context comes before its
/// consumer; only contexts the root composes are kept.
fn bundle_order(facts: &ComposeFacts) -> Vec<String> {
    fn visit(
        node: &str,
        facts: &ComposeFacts,
        visited: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if !visited.insert(node.to_string()) {
            return;
        }
        if let Some(unit) = facts.units.get(node) {
            for t in &unit.consumes {
                visit(t, facts, visited, out);
            }
        }
        out.push(node.to_string());
    }
    let mut visited = HashSet::new();
    let mut walked = Vec::new();
    for (name, unit) in &facts.units {
        if unit.is_context {
            visit(name, facts, &mut visited, &mut walked);
        }
    }
    walked
        .into_iter()
        .filter(|n| {
            facts
                .units
                .get(n)
                .is_some_and(|u| u.is_context && u.has_services)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(consumes: &[&str]) -> ComposeUnitFacts {
        ComposeUnitFacts {
            is_context: true,
            has_services: true,
            consumes: consumes.iter().map(|s| s.to_string()).collect(),
            ..ComposeUnitFacts::default()
        }
    }

    fn facts(units: Vec<(&str, ComposeUnitFacts)>) -> ComposeFacts {
        ComposeFacts {
            units: units.into_iter().map(|(n, f)| (n.to_string(), f)).collect(),
            bundle_root: true,
            ..ComposeFacts::default()
        }
    }

    #[test]
    fn a_unit_with_no_dependencies_has_no_surfaces_and_no_caps() {
        let plan = ComposePlan::decide(&facts(vec![("shop", ctx(&[]))]));
        let unit = &plan.units["shop"];
        assert!(unit.surfaces.is_empty());
        assert!(unit.cross_caps().is_empty());
        assert!(!unit.exec.any_handler && !unit.exec.call_handler);
        assert_eq!(plan.bundle_order, vec!["shop".to_string()]);
    }

    #[test]
    fn a_dependency_chain_orders_a_consumed_context_before_its_consumer() {
        // `a` consumes `b`, `b` consumes `c`: the names sort the other way.
        let plan = ComposePlan::decide(&facts(vec![
            ("a", ctx(&["b"])),
            ("b", ctx(&["c"])),
            ("c", ctx(&[])),
        ]));
        assert_eq!(plan.bundle_order, ["c", "b", "a"]);
        assert_eq!(plan.units["a"].surfaces[0].target, "b");
        assert_eq!(plan.units["a"].surfaces[0].key, "b");
        assert_eq!(plan.units["b"].surfaces[0].target, "c");
    }

    #[test]
    fn a_surface_takes_the_alias_else_the_last_segment_and_skips_a_serviceless_unit() {
        let mut consumer = ctx(&["shop.billing", "shop.ledger", "shop.static"]);
        consumer
            .aliases
            .insert("pay".to_string(), "shop.billing".to_string());
        let mut serviceless = ctx(&[]);
        serviceless.has_services = false;
        let plan = ComposePlan::decide(&facts(vec![
            ("app", consumer),
            ("shop.billing", ctx(&[])),
            ("shop.ledger", ctx(&[])),
            ("shop.static", serviceless),
        ]));
        let keys: Vec<(&str, &str)> = plan.units["app"]
            .surfaces
            .iter()
            .map(|s| (s.key.as_str(), s.target.as_str()))
            .collect();
        assert_eq!(keys, [("pay", "shop.billing"), ("ledger", "shop.ledger")]);
        // A context without services is consumed but not composed.
        assert!(!plan.bundle_order.contains(&"shop.static".to_string()));
    }

    #[test]
    fn a_caller_binding_provider_makes_the_consumers_surface_per_caller() {
        let mut provider = ctx(&[]);
        provider.binds_caller = true;
        let plan = ComposePlan::decide(&facts(vec![
            ("app", ctx(&["svc", "plain"])),
            ("svc", provider),
            ("plain", ctx(&[])),
        ]));
        let by_target: HashMap<&str, bool> = plan.units["app"]
            .surfaces
            .iter()
            .map(|s| (s.target.as_str(), s.per_caller))
            .collect();
        assert!(by_target["svc"]);
        assert!(!by_target["plain"]);
        assert!(plan.units["svc"].binds_caller);
    }

    #[test]
    fn an_exec_unit_keeps_both_predicates_apart() {
        let mut any = ctx(&[]);
        any.exec = ExecNeeds {
            any_handler: true,
            call_handler: false,
        };
        let mut call = ctx(&[]);
        call.exec = ExecNeeds {
            any_handler: true,
            call_handler: true,
        };
        let plan = ComposePlan::decide(&facts(vec![("any", any), ("call", call)]));
        assert_eq!(
            plan.units["any"].exec,
            ExecNeeds {
                any_handler: true,
                call_handler: false
            }
        );
        assert!(plan.units["call"].exec.call_handler);
    }

    #[test]
    fn a_providers_given_adds_a_cross_capability_the_handlers_do_not_use() {
        let mut unit = ctx(&["bank"]);
        unit.handler_cross_caps
            .insert("Ledger".to_string(), "bank".to_string());
        unit.provider_cross_caps
            .insert("Audit".to_string(), "bank".to_string());
        // A key both name keeps the handlers' source.
        unit.provider_cross_caps
            .insert("Ledger".to_string(), "other".to_string());
        let plan = ComposePlan::decide(&facts(vec![("app", unit)]));
        let unit = &plan.units["app"];
        assert_eq!(
            unit.handler_cross_caps.keys().collect::<Vec<_>>(),
            ["Ledger"]
        );
        let all = unit.cross_caps();
        assert_eq!(all.keys().collect::<Vec<_>>(), ["Audit", "Ledger"]);
        assert_eq!(all["Ledger"], "bank");
        assert_eq!(all["Audit"], "bank");
    }

    #[test]
    fn a_subscription_takes_the_envelope_its_subscribers_shape_wants() {
        let shaped = |two_param_handler, schema_dispatch| {
            let mut unit = ctx(&[]);
            unit.event_shapes.insert(
                "On".to_string(),
                EventSubscriberShape {
                    two_param_handler,
                    schema_dispatch,
                },
            );
            unit
        };
        let mut f = facts(vec![
            ("pub", ctx(&[])),
            ("plain", shaped(false, false)),
            ("two", shaped(true, false)),
            ("schema", shaped(false, true)),
        ]);
        f.event_subscribers.insert(
            ("pub".to_string(), "E".to_string()),
            ["plain", "two", "schema", "unknown"]
                .iter()
                .map(|c| (c.to_string(), "On".to_string()))
                .collect(),
        );
        let plan = ComposePlan::decide(&f);
        let wants: Vec<(&str, bool)> = plan
            .events
            .published_by("pub")
            .flat_map(|(_, subs)| subs)
            .map(|s| (s.context.as_str(), s.wants_envelope))
            .collect();
        assert_eq!(
            wants,
            [
                ("plain", false),
                ("two", true),
                ("schema", true),
                ("unknown", false)
            ]
        );
        // The unit's own table of its subscribers agrees (the Workers wrapper reads it).
        assert!(plan.units["two"].subscriber_envelopes["On"]);
        assert!(!plan.units["plain"].subscriber_envelopes["On"]);
    }

    #[test]
    #[should_panic(
        expected = "the events service `On` of unit `shop` has no recorded subscriber shape"
    )]
    fn a_hosted_events_service_without_a_shape_is_an_internal_error() {
        let plan = ComposePlan::decide(&facts(vec![("shop", ctx(&[]))]));
        plan.units["shop"].hosted_wants_envelope("shop", "On");
    }
}
