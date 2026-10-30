//! Per-Worker binding inputs, shared by the Workers build and the
//! duplicate-binding check.
//!
//! The check itself (`bynk.context.duplicate_binding`) is
//! [`bynk_check::worker_bindings::phase_duplicate_bindings`]; the names are the
//! emitter's ([`worker_bindings`]). The inputs here are the ones the Workers
//! build passes to `emit_wrangler_toml`, built by the same helpers, so the
//! check sees exactly the bindings the build would write.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bynk_check::checker::{Callee, ExprId};
use bynk_check::firstparty::{self, Platform};

use super::{
    BuildTarget, ErrorSink, ParsedFile, UnitKind, UnitTable, native_platforms_of_context,
    unit_table_uses_emit,
};
use crate::emitter::agent_cross_context_targets;
use crate::emitter::wrangler::{BindingSource, WorkerBinding, worker_bindings};

/// v0.19 (decision 0025): each context whose in-process closure reaches a
/// platform-native unit, with those platforms. Drives env threading (bundle)
/// and the per-Worker Env/`wrangler.toml` resource derivation (workers).
pub(super) fn contexts_native(
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    with_derived: bool,
) -> HashMap<String, BTreeMap<Platform, String>> {
    kinds
        .iter()
        .filter(|(_, k)| **k == UnitKind::Context)
        .filter_map(|(name, _)| {
            let table = unit_tables.get(name)?;
            let native = native_platforms_of_context(
                name,
                table,
                unit_tables,
                unit_consumes,
                unit_consumes_aliases,
                unit_flattened,
                with_derived,
            );
            (!native.is_empty()).then(|| (name.clone(), native))
        })
        .collect()
}

/// v0.19 (C1): a Worker needs the KV namespace binding when its in-process
/// closure reaches the cloudflare adapter.
pub(super) fn needs_kv(
    context_native: &HashMap<String, BTreeMap<Platform, String>>,
    ctx: &str,
) -> bool {
    context_native
        .get(ctx)
        .is_some_and(|n| n.values().any(|u| u == firstparty::CLOUDFLARE_UNIT))
}

/// The contexts a Worker holds a Service Binding to: those whose services it
/// calls, plus every context subscribing to an event it publishes (Events
/// track, slice 0, spine #936, ADR 0284). A subscriber `consumes` the
/// publisher for the event *type*; nothing upstream gives the publisher a
/// binding back to the subscriber, so its Worker needs one added here: the
/// reverse direction of an ordinary `consumes` edge.
///
/// #1961: a consumed context is bound only when called, never merely for
/// being consumed. A subscriber consumes its publisher for the event type
/// alone, and binding that `consumes` edge as well made the two Workers bind
/// each other — a cycle no upload order satisfies, since Cloudflare refuses a
/// Worker whose bound target does not exist yet. An adapter is never called
/// (it may not declare a `service`), so it never appears here.
fn service_targets(
    called: &[String],
    own_event_routes: &BTreeMap<String, Vec<(String, String)>>,
) -> Vec<String> {
    let mut targets: BTreeSet<String> = called.iter().cloned().collect();
    targets.extend(
        own_event_routes
            .values()
            .flatten()
            .map(|(sub_ctx, _)| sub_ctx.clone()),
    );
    targets.into_iter().collect()
}

/// This context's own published events → their subscribers, sliced from the
/// project-wide table.
pub(super) fn own_event_routes(
    ctx: &str,
    event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
) -> BTreeMap<String, Vec<(String, String)>> {
    event_subscribers
        .iter()
        .filter(|((owner, _), _)| owner == ctx)
        .map(|((_, name), subs)| (name.clone(), subs.clone()))
        .collect()
}

/// Every binding the context's Worker declares
/// ([`worker_bindings`]), from the inputs the build and the duplicate check
/// both use: the called contexts ([`super::called_cross_context_services`]'s
/// keys) plus event subscribers, whether the closure reaches the KV namespace,
/// and whether its handlers emit.
pub(super) fn context_bindings(
    table: &UnitTable,
    called: &[String],
    own_event_routes: &BTreeMap<String, Vec<(String, String)>>,
    needs_kv: bool,
    uses_emit: bool,
) -> Vec<WorkerBinding> {
    let consumes = service_targets(called, own_event_routes);
    let agents: Vec<String> = table.agents.keys().cloned().collect();
    let mut instances: Vec<(&String, WorkerBinding)> = table
        .derived
        .iter()
        // L7 (#2414): Workers Cache is a Worker-wide `[cache]` setting, not
        // an `env` binding.
        .filter(|(_, d)| !d.is_edge_cache())
        .map(|(cap, d)| {
            // P3 (#2342): every instantiable kind by name, with no catch-all,
            // so a new kind can't silently bind as a KV namespace.
            let capability = cap.clone();
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
                (firstparty::CLOUDFLARE_UNIT, "Queue") => BindingSource::QueueProducer {
                    capability,
                    queue: d.queue().unwrap_or_default().to_string(),
                },
                (firstparty::CLOUDFLARE_UNIT, "Kv") => BindingSource::KvInstance { capability },
                (unit, base) => {
                    unreachable!("`{unit}.{base}` is instantiable but has no binding kind")
                }
            };
            (
                cap,
                WorkerBinding {
                    name: d.binding.clone(),
                    source,
                },
            )
        })
        .collect();
    instances.sort_by(|a, b| a.0.cmp(b.0));
    let instances: Vec<WorkerBinding> = instances.into_iter().map(|(_, b)| b).collect();
    worker_bindings(&agents, &consumes, needs_kv, &instances, uses_emit)
}

/// Every context's bindings ([`context_bindings`]), as its `wrangler.toml`
/// would list them: what the duplicate check reads, and what the integration
/// harness binds on each participant's `env` (#1959). Only on an otherwise
/// clean program: the KV input walks the provider graph, whose acyclicity the
/// earlier checks establish.
#[allow(clippy::too_many_arguments)]
pub(super) fn all_context_bindings(
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    unit_callees: &HashMap<String, HashMap<ExprId, Callee>>,
    // `bynk_check::symbols::discover_event_subscribers`' table.
    event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
) -> BTreeMap<String, Vec<WorkerBinding>> {
    // Only `needs_kv` reads it: the bare resources, not named instances.
    let context_native = contexts_native(
        kinds,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_flattened,
        false,
    );
    let mut bindings = BTreeMap::new();
    for (ctx, kind) in kinds {
        if *kind != UnitKind::Context {
            continue;
        }
        let Some(table) = unit_tables.get(ctx) else {
            continue;
        };
        bindings.insert(
            ctx.clone(),
            context_bindings(
                table,
                &called_contexts(
                    table,
                    unit_consumes
                        .get(ctx)
                        .map(Vec::as_slice)
                        .unwrap_or_default(),
                    unit_consumes_aliases.get(ctx),
                    kinds,
                    unit_callees.get(ctx),
                ),
                &own_event_routes(ctx, event_subscribers),
                needs_kv(&context_native, ctx),
                unit_table_uses_emit(table, unit_callees.get(ctx)),
            ),
        );
    }
    bindings
}

/// The consumed contexts a context's Worker binds for calling, in name order:
/// those whose services its bodies call (the keys of
/// [`super::called_cross_context_services`]), plus those its agents call
/// ([`agent_cross_context_targets`]) — a handler constructing an agent hands
/// it its own `deps`, whose `env` type names them (#1960, #1961).
pub(super) fn called_contexts(
    table: &UnitTable,
    consumes: &[String],
    aliases: Option<&HashMap<String, String>>,
    kinds: &BTreeMap<String, UnitKind>,
    callees: Option<&HashMap<ExprId, Callee>>,
) -> Vec<String> {
    let mut called: BTreeSet<String> =
        super::called_cross_context_services(table, consumes, callees)
            .into_keys()
            .collect();
    called.extend(agent_cross_context_targets(table, |chain| {
        let target = aliases
            .and_then(|a| a.get(chain))
            .cloned()
            .or_else(|| consumes.iter().find(|c| *c == chain).cloned())?;
        (kinds.get(&target) == Some(&UnitKind::Context)).then_some(target)
    }));
    called.into_iter().collect()
}

/// `bynk.context.duplicate_binding`: on a Workers build, each context's
/// bindings ([`all_context_bindings`]), checked by
/// [`bynk_check::worker_bindings::phase_duplicate_bindings`].
pub(super) fn phase_duplicate_bindings(
    target: BuildTarget,
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    bindings: &BTreeMap<String, Vec<WorkerBinding>>,
    errors: &mut ErrorSink,
) {
    if target != BuildTarget::Workers {
        return;
    }
    bynk_check::worker_bindings::phase_duplicate_bindings(parsed, groups, bindings, errors);
}
