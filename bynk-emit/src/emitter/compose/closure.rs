//! A context's in-process closure: the units whose providers its compose would
//! instantiate (back-half paydown 1C.1a, Decision C of #4031).
//!
//! The walk reads the provider graph, whose acyclicity the earlier checks
//! establish, so it runs only on an otherwise clean program. [`super::plan`]
//! records its results as facts (`bare_kv`, `bare_singletons`), and the Bundle
//! build's env threading calls [`native_platforms_of_context`] directly.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use bynk_check::firstparty::Platform;
use bynk_check::project_model::handler_cross_caps;
use bynk_check::symbols::UnitTable;

use crate::emit_model::{
    ProviderNamespaces, instantiate_provider_ts_expr, parse_bare_singleton_marker,
};

/// v0.19 (decision 0017): the native platforms a context's **in-process
/// closure** commits it to: every unit whose provider its compose would
/// instantiate (local providers' `given` recursion plus the capabilities its
/// handlers reference), mapped through [`bynk_check::firstparty::platform_of`].
/// Each platform carries an exemplar unit for the diagnostic message. Service
/// `consumes` edges (RPC under `workers`) do not contribute: only the
/// provider-instantiation walk, which is in-process by construction.
#[allow(clippy::too_many_arguments)]
pub fn native_platforms_of_context(
    ctx: &str,
    table: &UnitTable,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // S4a (#1966): count derived capabilities' base units. `true` for what
    // threads `env`; `false` for whether the *bare* platform resource (the
    // `KV` namespace) is used, which a named instance alone must not imply.
    with_derived: bool,
) -> BTreeMap<Platform, String> {
    let referenced = referenced_units_of_context(
        ctx,
        table,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_flattened,
        with_derived,
    );
    let mut out = BTreeMap::new();
    for unit in referenced {
        if let Some(p) = bynk_check::firstparty::platform_of(&unit) {
            out.entry(p).or_insert(unit);
        }
    }
    out
}

/// #3300: the one-per-Worker capabilities (`Version`) context `ctx`'s closure
/// reaches bare, as `(unit, capability)`, in order: the bare-resources walk's
/// [`crate::emit_model::bare_singleton_marker`]s.
pub fn bare_singletons_of_context(
    ctx: &str,
    table: &UnitTable,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
) -> Vec<(String, String)> {
    referenced_units_of_context(
        ctx,
        table,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
        unit_flattened,
        false,
    )
    .iter()
    .filter_map(|entry| {
        parse_bare_singleton_marker(entry).map(|(u, c)| (u.to_string(), c.to_string()))
    })
    .collect()
}

/// The units (and, on the bare walk, bare-singleton markers) context `ctx`'s
/// providers and cross-context capabilities instantiate.
fn referenced_units_of_context(
    ctx: &str,
    table: &UnitTable,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    with_derived: bool,
) -> BTreeSet<String> {
    // Arc F slice 2 (#1452): repointed at the tree-native twin; only the
    // `referenced` side effect matters here, the returned `TsExpr` is
    // discarded.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    let namespaces = if with_derived {
        ProviderNamespaces::Bundle
    } else {
        ProviderNamespaces::BareResources
    };
    for cap in table.providers.keys() {
        let _ = instantiate_provider_ts_expr(
            ctx,
            cap,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            namespaces,
            None,
            None,
            &mut referenced,
        );
    }
    let consumed = unit_consumes.get(ctx).cloned().unwrap_or_default();
    let aliases = unit_consumes_aliases.get(ctx).cloned().unwrap_or_default();
    let flattened = unit_flattened.get(ctx).cloned().unwrap_or_default();
    for (key, cctx) in handler_cross_caps(table, ctx, &consumed, &aliases, &flattened) {
        let _ = instantiate_provider_ts_expr(
            &cctx,
            &key,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            namespaces,
            None,
            None,
            &mut referenced,
        );
    }
    if with_derived {
        referenced.extend(table.derived.values().map(|d| d.base_unit.clone()));
    }
    referenced
}
