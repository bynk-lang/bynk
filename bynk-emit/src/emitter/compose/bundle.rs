//! The Bundle composition root (`compose.ts`, `composeApp()`).
//!
//! Moved here from `bynk-driver/src/project.rs` by back-half paydown 1B.1a
//! (Decision B of #3928): it is the Bundle target's production output.
//! Since 1B.1b it reads the units, their dependencies and their cross-context
//! capabilities from the [`ComposePlan`]; the order of a unit's `deps` entries,
//! the spelling of each provider and the `__exec` condition are this
//! renderer's own (divergences 1, 2, 3 and 4 of #3928).

use std::collections::{BTreeSet, HashMap};

use crate::emit_model::*;
use crate::emitter;
use crate::emitter::compose::events::isolated_delivery;
use crate::emitter::compose::{ComposePlan, empty_deps};
use crate::ts_build::{call, const_, ident, member, method_call, str_lit};
use bynk_check::project_model::AdapterBinding;
use bynk_check::symbols::UnitTable;
use bynk_project::paths::commons_dir_for;
use bynk_ts::{TsArrowBody, TsDecl, TsExpr, TsParam, TsProgram, TsStmt, TsSwitchCase, TsType};

/// Where `compose.ts` imports the runtime from (it sits at the out root).
///
/// The import is built from the finished tree (`derived_runtime_names`) and
/// inserted after the banner, so this is the one place the specifier is
/// spelled.
const COMPOSE_RUNTIME_SPECIFIER: &str = "./runtime.js";

// -- Small tree-construction helpers (#1327) ------------------------------
/// Divergence 1 of #3928, the Bundle's entry order: sort key for a `{ns}Deps` entry, mirroring the pre-conversion code's own
/// `Vec<String>::sort()` over the fully rendered `"{key}: {value}"` text
/// rather than the bare key alone (review of #1328: `deps_entries.sort_by(|a,
/// b| a.0.cmp(&b.0))` is NOT equivalent — it silently reorders whenever one
/// key is a strict prefix of another, e.g. `Db`/`Db2`). The old text sort's
/// tie-break, for a key that is a strict prefix of another, was whatever byte
/// immediately follows it in the longer key compared against the shorter
/// key's own literal `':'` — since capability names are `[A-Za-z][A-Za-z0-9_
/// ]*`, only a digit-suffixed prefix collision (`'0'`-`'9'` are all below
/// `':'`, 0x3A) flips the order: `Db2: ...` sorted before `Db: ...` because
/// `'2'` (0x32) < `':'` (0x3A). Appending `:` to each bare key reproduces
/// that exact tie-break without rendering each entry's value just to sort
/// it — every other key relationship (no shared prefix, or a
/// letter/underscore-suffixed prefix, both above `':'`) is unaffected, since
/// the comparison never reaches the appended `:`.
fn deps_entry_sort_key(key: &str) -> String {
    format!("{key}:")
}

/// #2683: a `call` handler that calls `within` (or sends) reads `deps.__exec`.
/// The bundle has no platform execution context, so what it is handed is kept
/// alive by the process; a fault in it, with no handler left to raise it to, is
/// reported by the same reporter `e.detach()` uses (#3541), not a logger of the
/// shim's own. Divergence 4 of #3928: the Bundle gives it to a unit whose
/// `call` handler needs one (the Workers `compose` to any handler), and as
/// `__bynkProcessExec()`, not the request's `exec`.
fn bundle_process_exec_entry(needs_exec: bool) -> Option<(String, TsExpr)> {
    needs_exec.then(|| {
        (
            "__exec".to_string(),
            call(ident("__bynkProcessExec"), Vec::new()),
        )
    })
}

/// Events track, slice 0 (spine #936): the `__eventsDispatch` closure of a
/// publishing context. Bundle/node mode has no isolate boundary to cross, so
/// this dispatches in-process, directly into each subscriber's own `on event`
/// handler (`.event`, the object method `emit_service` gives
/// `HandlerKind::Event`), reusing whatever deps that subscriber context builds
/// in the same `composeApp` (referenced by name; the arrow's body is not
/// evaluated until well after every `const ...Deps` has run, so declaration
/// order does not matter). A publisher with no subscribers still gets the
/// field, as its type is required, with an empty switch.
///
/// Each delivery is the shared failure-isolated one (ADR 0284,
/// [`isolated_delivery`]); whether it carries the envelope is the plan's
/// per-subscription `wants_envelope` (Decision F of #3928).
fn events_dispatch(plan: &ComposePlan, publisher: &str) -> TsExpr {
    // #3815: the plan's events are in name order, so the cases do not vary from
    // one build to the next (the table they came from is a `HashMap`).
    let cases: Vec<TsSwitchCase> = plan
        .events
        .published_by(publisher)
        .map(|(event, subs)| {
            let mut body: Vec<TsStmt> = subs
                .iter()
                .map(|sub| {
                    let sub_ns = sub.context.replace('.', "_");
                    // P7.2: deferred, not narrowed. The event's bare name is not
                    // necessarily in scope at this dispatch site
                    // (cross-context: publisher and subscriber are different
                    // units), the same qualification problem
                    // `ts_type_ref_qualified_ts_type` exists to solve for
                    // handler wrappers elsewhere in this crate, so the payload
                    // stays `any`.
                    let mut args = vec![as_any(member(ident("ev"), "payload"))];
                    if sub.wants_envelope {
                        args.push(member(ident("ev"), "envelope"));
                    }
                    args.push(ident(format!("{sub_ns}Deps")));
                    let delivery = call(
                        member(member(ident(sub_ns), sub.service.as_str()), "event"),
                        args,
                    );
                    isolated_delivery(delivery, str_lit(sub.service.as_str()))
                })
                .collect();
            body.push(TsStmt::break_stmt(None));
            TsSwitchCase {
                labels: vec![str_lit(event)],
                body,
                default_braced: false,
                case_braced: true,
                span: None,
            }
        })
        .collect();
    let dispatch = TsStmt::for_of(
        "ev",
        ident("events"),
        TsStmt::block(
            vec![TsStmt::switch_stmt(
                member(ident("ev"), "type"),
                cases,
                None,
            )],
            None,
        ),
        None,
    );
    TsExpr::Arrow {
        params: vec![TsParam::new(
            "events".to_string(),
            Some(TsType::named_with_args(
                "globalThis.Array",
                vec![TsType::named(crate::emitter::EVENTS_WIRE_EVENT_TS_TYPE)],
            )),
        )],
        is_async: true,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Multiline(vec![dispatch])),
    }
}

fn as_any(expr: TsExpr) -> TsExpr {
    TsExpr::As {
        expr: Box::new(expr),
        ty: TsType::named("any"),
    }
}

#[allow(clippy::too_many_arguments)]
pub fn emit_composition_root(
    plan: &ComposePlan,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_tables: &HashMap<String, UnitTable>,
    adapter_bindings: &HashMap<String, AdapterBinding>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // v0.19 (decision 0025, D1): when the program's closure reaches a
    // platform-native unit, composeApp takes an optional `env` and threads it
    // to env-taking first-party providers. A bundle on Cloudflare is a single
    // Worker with `env` at its entry; native-free programs emit the v0.18
    // no-parameter signature unchanged.
    thread_env: bool,
) -> Option<TsProgram> {
    if !plan.bundle_root {
        return None;
    }

    // The composeApp body is built first so the provider expressions can
    // record every unit namespace they reference (v0.18: an external
    // provider's `given` may pull in *another* adapter's binding — the
    // transitive given-closure — which must then be imported).
    let mut referenced_units: BTreeSet<String> = BTreeSet::new();

    let compose_params: Vec<TsParam> = if thread_env {
        vec![TsParam::optional(
            "env".to_string(),
            Some(TsType::named("unknown")),
        )]
    } else {
        Vec::new()
    };
    let env_ident = if thread_env { Some("env") } else { None };

    let mut body: Vec<TsStmt> = Vec::new();

    // The plan's order puts a consumed context before its consumer, and holds
    // only the contexts the root composes: a context's deps object exists only
    // to feed its `__makeSurface`, so a capability-only context (no services)
    // needs neither (v0.15). The bundle's `bynk.capability.not_provided` check
    // skips the same ones.
    for ctx_name in &plan.bundle_order {
        let unit = &plan.units[ctx_name];
        let ns = ctx_name.replace('.', "_");

        // Divergence 2 of #3928, provider spelling: each provider is one
        // inline expression, recursing through its `given` (the Workers
        // `compose` builds a `const` per provider in dependency order).
        let mut deps_entries: Vec<(String, TsExpr)> = unit
            .providers
            .iter()
            .map(|cap| {
                let expr = instantiate_provider_ts_expr(
                    ctx_name,
                    cap,
                    unit_tables,
                    unit_consumes,
                    unit_consumes_aliases,
                    unit_flattened,
                    ProviderNamespaces::Bundle,
                    env_ident,
                    None, // Bundle mode has no inbound request (Decision A)
                    &mut referenced_units,
                );
                (cap.clone(), expr)
            })
            .collect();
        // v0.15: cross-context capabilities used directly by handlers become
        // top-level deps fields, instantiated from the providing context.
        // Divergence 3 of #3928, the cross-capability set: the handlers' only.
        // A provider's own cross-context `given` is reached through that
        // provider's recursive expression, not as a top-level field (the
        // Workers `compose` also lifts the providers').
        for (key, cctx) in &unit.handler_cross_caps {
            let expr = instantiate_provider_ts_expr(
                cctx,
                key,
                unit_tables,
                unit_consumes,
                unit_consumes_aliases,
                unit_flattened,
                ProviderNamespaces::Bundle,
                env_ident,
                None, // Bundle mode has no inbound request (Decision A)
                &mut referenced_units,
            );
            deps_entries.push((key.clone(), expr));
        }
        // S4a (#1966): each derived capability's instance.
        for cap in &unit.derived {
            let expr = instantiate_provider_ts_expr(
                ctx_name,
                cap,
                unit_tables,
                unit_consumes,
                unit_consumes_aliases,
                unit_flattened,
                ProviderNamespaces::Bundle,
                env_ident,
                None,
                &mut referenced_units,
            );
            deps_entries.push((cap.clone(), expr));
        }
        if unit.publishes {
            deps_entries.push((
                "__eventsDispatch".to_string(),
                events_dispatch(plan, ctx_name),
            ));
        }
        if let Some(entry) = bundle_process_exec_entry(unit.exec.call_handler) {
            deps_entries.push(entry);
        }
        deps_entries.sort_by_cached_key(|(k, _)| deps_entry_sort_key(k));

        // Divergence 6 of #3928, the consumed surfaces: only the Bundle's
        // `deps` carries a consumed context's surface (the Workers reach it
        // through a Service Binding).
        let mut surface_entries: Vec<(String, TsExpr)> = Vec::new();
        for surface in &unit.surfaces {
            let t_ns = surface.target.replace('.', "_");
            // v0.54 (#655): a consumed context with an `on call … by c: Caller`
            // handler needs the *caller's* qualified name (this context) threaded
            // into that handler's deps as its `CallerId` identity (ADR 0092). The
            // shared `{t_ns}Surface` (built for the top-level entry with the
            // provider's own name) would carry the wrong caller, so build a
            // per-consumer surface instead. A caller-free provider keeps the
            // shared instance — byte-unchanged.
            let entry = if surface.per_caller {
                method_call(
                    ident(t_ns.clone()),
                    "__makeSurface",
                    vec![ident(format!("{t_ns}Deps")), str_lit(ctx_name.as_str())],
                )
            } else {
                ident(format!("{t_ns}Surface"))
            };
            surface_entries.push((surface.key.clone(), entry));
        }
        if !surface_entries.is_empty() {
            deps_entries.push(("surface".to_string(), TsExpr::object(surface_entries)));
        }
        // `{  }` for no entries: see `compose::empty_deps`.
        let deps_init = if deps_entries.is_empty() {
            empty_deps()
        } else {
            TsExpr::object(deps_entries)
        };
        body.push(const_(format!("{ns}Deps"), deps_init));
        if unit.has_services {
            // The top-level entry addresses the context directly; there is no
            // calling context, so a `by c: Caller` handler reached this way reads
            // the context's own qualified name (a stable, non-empty `CallerId`
            // within the single-trust-domain bundle).
            let mut make_surface_args = vec![ident(format!("{ns}Deps"))];
            if unit.binds_caller {
                make_surface_args.push(str_lit(ctx_name.as_str()));
            }
            body.push(const_(
                format!("{ns}Surface"),
                method_call(ident(ns.clone()), "__makeSurface", make_surface_args),
            ));
        }
    }

    // #1327: the pre-conversion code unconditionally wrote one blank line
    // between the last `const ...Deps`/`const ...Surface` and the `return`
    // (`out.push('\n')`, run once regardless of how many contexts the loop
    // above actually pushed) — `TsStmtKind::Blank` (#1323) is the tree's own
    // equivalent, needed here since the printer's own "blank line between
    // top-level declarations" policy only separates entries in
    // `TsProgram.stmts` itself, not statements inside one function body.
    body.push(TsStmt::blank(None));

    // Export per-context surfaces under a top-level object.
    let mut return_entries: Vec<(String, TsExpr)> = Vec::new();
    for (ctx_name, unit) in plan.contexts() {
        if !unit.has_services {
            continue;
        }
        let ns = ctx_name.replace('.', "_");
        let key = ctx_name.rsplit('.').next().unwrap_or(ctx_name.as_str());
        return_entries.push((key.to_string(), ident(format!("{ns}Surface"))));
    }
    body.push(TsStmt::return_stmt(
        Some(TsExpr::multiline_object(return_entries)),
        None,
    ));

    // Assemble the header now that the body has recorded which units its
    // provider expressions reference.
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment("composition root", None));
    // The runtime import is what the finished tree uses (Decision G of #3928),
    // inserted here once the function is built.
    let runtime_import_at = program.stmts.len();

    // Import every context as a namespace.
    for (ctx_name, _) in plan.contexts() {
        let dir = emitter::ts_specifier(&commons_dir_for(ctx_name));
        let ns = ctx_name.replace('.', "_");
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: ns,
                from: format!("./{dir}.js"),
            },
            None,
        ));
    }
    // v0.17: import each consumed adapter's binding module — the external
    // provider classes live there, not in the adapter's interface module.
    // v0.18: plus every adapter the provider expressions referenced through
    // the transitive given-closure (an adapter's external provider may depend
    // on another adapter's capability, spec §4.5).
    let mut consumed_adapters: Vec<String> = plan
        .contexts()
        .flat_map(|(_, unit)| unit.consumes.iter().cloned())
        .chain(referenced_units.iter().cloned())
        .filter(|t| adapter_bindings.contains_key(t))
        .collect();
    consumed_adapters.sort();
    consumed_adapters.dedup();
    for adapter in &consumed_adapters {
        let ns = adapter.replace('.', "_");
        let module =
            emitter::ts_specifier(&adapter_bindings[adapter].output_path.with_extension("js"));
        program.push(TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: format!("{ns}__binding"),
                from: format!("./{module}"),
            },
            None,
        ));
    }

    program.push(TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::Function {
            name: "composeApp".to_string(),
            generics: Vec::new(),
            params: compose_params,
            return_type: None,
            body,
            is_async: false,
            inline: false,
        })),
        None,
    ));

    let names = emitter::runtime_header::derived_runtime_names(&program.stmts[runtime_import_at..]);
    for (i, stmt) in
        emitter::runtime_files::runtime_import_stmts(false, &names, COMPOSE_RUNTIME_SPECIFIER)
            .into_iter()
            .enumerate()
    {
        program.stmts.insert(runtime_import_at + i, stmt);
    }
    // A namespace is imported for every unit the closure reaches; the tree keeps the ones it uses.
    program.prune_unused_imports();
    Some(program)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review of #1328: the pre-conversion code sorted the fully rendered
    /// `"{key}: {value}"` text, so a key that is a strict prefix of another
    /// (`Db`/`Db2`) sorted by the shorter key's own `':'` losing to the
    /// longer key's next byte, a digit — `Db2: ...` sorted before `Db:
    /// ...`. A bare-key sort (`"Db" < "Db2"`) gets this backwards.
    /// `deps_entry_sort_key` must reproduce the original order exactly.
    #[test]
    fn deps_entry_sort_key_reproduces_the_pre_conversion_full_text_sort_order() {
        let mut keys = vec!["Db2".to_string(), "Db".to_string()];
        keys.sort_by_cached_key(|k| deps_entry_sort_key(k));
        assert_eq!(
            keys,
            vec!["Db2".to_string(), "Db".to_string()],
            "Db2 must sort before Db, matching the old full-text sort"
        );

        // A letter/underscore-suffixed prefix collision is unaffected —
        // matches plain key ordering both before and after this fix.
        let mut keys = vec!["Db_pool".to_string(), "Db".to_string()];
        keys.sort_by_cached_key(|k| deps_entry_sort_key(k));
        assert_eq!(keys, vec!["Db".to_string(), "Db_pool".to_string()]);

        // No shared prefix at all: ordinary alphabetical order, unaffected.
        let mut keys = vec!["Queue".to_string(), "Cache".to_string()];
        keys.sort_by_cached_key(|k| deps_entry_sort_key(k));
        assert_eq!(keys, vec!["Cache".to_string(), "Queue".to_string()]);
    }
}
