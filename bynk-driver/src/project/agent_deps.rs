use bynk_check::symbols::UnitTable;
use bynk_emit::emit_model::ExtraImport;
use bynk_emit::emit_model::ProviderNamespaces;
use bynk_emit::emit_model::instantiate_provider_ts_expr;
use std::collections::{BTreeMap, BTreeSet, HashMap};

use bynk_check::project_model::{AdapterBinding, UnitInfo, resolve_consume_prefix};
use bynk_model::CapRefIr;
use bynk_model::lower_handler_given_ir;
use bynk_project::worker_layout::WorkerLayout;

/// #527: the DO-side deps plan for one workers context. Capability providers
/// cannot cross the DO wire (`{ args, deps }` is JSON — a provider's methods
/// die in serialisation), so the generated Durable Object reconstructs its
/// agents' `given` deps *inside* the DO from the same wiring compose uses.
#[derive(Debug, Default, Clone)]
pub struct AgentDepsPlan {
    /// agent name → TS object-literal expression for its `given` deps
    /// (e.g. `{ Clock: new bynk__binding.ClockProvider() }`).
    pub exprs: HashMap<String, String>,
    /// agent name → the namespace aliases its expression spells (`bynk__binding`,
    /// `handlers_shop_orders`). The emitter declares them with the text, so each module's imports
    /// follow its tree (1A.36b-2).
    pub names: HashMap<String, Vec<String>>,
    /// Imports the expressions need in `handlers.ts` (binding modules,
    /// other Workers' handlers). Same relative depth as `compose.ts`.
    pub imports: Vec<ExtraImport>,
}

/// Build the [`AgentDepsPlan`] for context `name`, or `None` when no local
/// agent has `given` capabilities.
///
/// `split`: the context spans several files, each emitted as its own module
/// under `handlers/` (#1820, [`worker_file_source_path`](bynk_project::paths::worker_file_source_path)). The imports are
/// then one directory deeper, and this context's own providers are no longer
/// in the agent's module, so they are reached through `handlers.ts` like
/// another context's. That import is circular (`handlers.ts` re-exports the
/// agent's module), which is safe: the expressions are evaluated in the
/// Durable Object's fetch dispatch, never while the modules load.
pub(super) fn plan_agent_given_deps(
    name: &str,
    unit_info: &BTreeMap<String, UnitInfo>,
    adapter_bindings: &HashMap<String, AdapterBinding>,
    split: bool,
    // #2137: whether a `system` harness stands this context up (test build).
    harness_clock: bool,
    layout: &WorkerLayout,
) -> Option<AgentDepsPlan> {
    let info = unit_info.get(name)?;
    info.table.agents.values().next()?;
    let unit_tables: HashMap<String, UnitTable> = unit_info
        .iter()
        .map(|(n, i)| (n.clone(), i.table.clone()))
        .collect();
    let unit_consumes: HashMap<String, Vec<String>> = unit_info
        .iter()
        .map(|(n, i)| (n.clone(), i.consumes.clone()))
        .collect();
    let unit_consumes_aliases: HashMap<String, HashMap<String, String>> = unit_info
        .iter()
        .map(|(n, i)| (n.clone(), i.aliases.clone()))
        .collect();
    let unit_flattened: HashMap<String, HashMap<String, String>> = unit_info
        .iter()
        .map(|(n, i)| (n.clone(), i.flattened.clone()))
        .collect();

    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let mut exprs: HashMap<String, String> = HashMap::new();
    let mut names: HashMap<String, Vec<String>> = HashMap::new();
    // P6.35 (design/tracks/the-ir.md §6a): no explicit `&AgentDecl` annotation
    // needed — the loop body's own `a.handlers` field access below already
    // pins the element type through inference.
    let mut agents: Vec<_> = info.table.agents.iter().collect();
    agents.sort_by_key(|(n, _)| (*n).clone());
    for (agent, a) in agents {
        // #1187's slice 6 (Agent/Service given wiring): reads bynk-emit::ir's
        // own CapRefIr (lower_handler_given_ir — a standalone reader mirroring
        // lower_provider_given_ir, #1200) instead of walking the raw AST
        // CapRef directly. `caps` stays keyed by bare name (declaration-order-
        // first dedup across every handler), just storing CapRefIr instead of
        // CapRef.
        let mut caps: std::collections::BTreeMap<String, CapRefIr> =
            std::collections::BTreeMap::new();
        for h in &a.handlers {
            for g in lower_handler_given_ir(h) {
                // Events track, slice 0 (spine #936): see the matching skip
                // in `handler_cross_caps` — no `EventsProvider` exists for
                // compose (or a synthesised DO's own reconstructed deps) to
                // build.
                if g.name == "Events"
                    && info.flattened.get(g.name.as_str()).map(String::as_str) == Some("bynk")
                {
                    continue;
                }
                caps.entry(g.name.clone()).or_insert(g);
            }
        }
        if caps.is_empty() {
            continue;
        }
        // Arc F slice 2 (#1452): `instantiate_provider_ts_expr` — the
        // tree-native twin `emit_worker_compose`/`emit_composition_root`
        // already use (#1321/#1327) — builds the same `new {ns}.{Class}(...)`
        // shape as a real `bynk_ts::TsExpr`, byte-identical once printed
        // (both share every recursive call/text-building step below the
        // top level; only this per-agent object wrapper is new). Boundary-
        // prints once, here, into `exprs: HashMap<String, String>` — the
        // struct's own shape and `emit_agent`'s `TsExpr::Ident(expr.clone())`
        // caller (`emitter/emit.rs`) both stay unchanged, the narrower of
        // the two options the issue named.
        let mut agent_referenced: BTreeSet<String> = BTreeSet::new();
        let entries: Vec<(String, bynk_ts::TsExpr)> = caps
            .iter()
            .map(|(key, g)| {
                let target_ctx = match &g.context {
                    Some(p) => resolve_consume_prefix(p, &info.consumes, &info.aliases)
                        .unwrap_or_else(|| name.to_string()),
                    None => info
                        .flattened
                        .get(key.as_str())
                        .cloned()
                        .unwrap_or_else(|| name.to_string()),
                };
                let expr = instantiate_provider_ts_expr(
                    &target_ctx,
                    key,
                    &unit_tables,
                    &unit_consumes,
                    &unit_consumes_aliases,
                    &unit_flattened,
                    ProviderNamespaces::Workers { harness_clock },
                    Some("env"),
                    None,
                    &mut agent_referenced,
                );
                (key.clone(), expr)
            })
            .collect();
        referenced.extend(agent_referenced.iter().cloned());
        names.insert(agent.clone(), agent_referenced.into_iter().collect());
        exprs.insert(
            agent.clone(),
            bynk_ts::print_expr(&bynk_ts::TsExpr::object(entries)),
        );
    }
    if exprs.is_empty() {
        return None;
    }
    // Providers of *this* context live in the same module (`handlers.ts`), so
    // their compose-namespace prefix drops. Not for a split context: there
    // they are imported from `handlers.ts` like any other context's (`../`
    // up from `handlers/` to the Worker's own directory, then back down).
    let self_ns = format!("handlers_{}.", name.replace('.', "_"));
    if !split {
        for e in exprs.values_mut() {
            *e = e.replace(&self_ns, "");
        }
        referenced.remove(name);
    }
    let up = if split { "../" } else { "" };
    // The aliases each agent's expression spells: the namespace of every unit it references,
    // bar this context's own where the module holds its providers itself (`self_ns` above).
    let alias_of = |u: &str| {
        let ns = u.replace('.', "_");
        if adapter_bindings.contains_key(u) {
            format!("{ns}__binding")
        } else {
            format!("handlers_{ns}")
        }
    };
    let names: HashMap<String, Vec<String>> = names
        .into_iter()
        .map(|(agent, units)| {
            let aliases = units
                .iter()
                .filter(|u| split || u.as_str() != name)
                .map(|u| alias_of(u))
                .collect();
            (agent, aliases)
        })
        .collect();
    let mut imports = Vec::new();
    for u in &referenced {
        let ns = u.replace('.', "_");
        if let Some(b) = adapter_bindings.get(u) {
            let module = bynk_emit::emitter::ts_specifier(&b.output_path.with_extension("js"));
            imports.push(ExtraImport::Namespace {
                alias: format!("{ns}__binding"),
                from: format!("{up}../../{module}"),
            });
        } else {
            // A split context's modules sit one directory below its Worker's.
            let spec = layout.handlers_specifier(name, u);
            let spec = if split {
                format!("../{}", spec.strip_prefix("./").unwrap_or(&spec))
            } else {
                spec
            };
            imports.push(ExtraImport::Namespace {
                alias: format!("handlers_{ns}"),
                from: spec,
            });
        }
    }
    Some(AgentDepsPlan {
        exprs,
        names,
        imports,
    })
}
