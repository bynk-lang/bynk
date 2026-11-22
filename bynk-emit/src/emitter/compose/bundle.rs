//! The Bundle composition root (`compose.ts`, `composeApp()`).
//!
//! Moved here from `bynk-driver/src/project.rs` unchanged by back-half
//! paydown 1B.1a (Decision B of #3928): it is the Bundle target's production
//! output, and everything it reads is reachable from `bynk-emit`.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::emit_model::*;
use crate::emitter;
use bynk_check::checker::ExprId;
use bynk_check::project_model::{self, AdapterBinding, handler_cross_caps};
use bynk_check::symbols::UnitTable;
use bynk_model::EventSubscriberShape;
use bynk_project::paths::commons_dir_for;
use bynk_project::roots::UnitKind;
use bynk_ts::{TsDecl, TsExpr, TsParam, TsProgram, TsStmt, TsType};

/// v0.54 (#655): whether a context's services declare an `on call … by c: Caller`
/// handler, whose emitted `deps` carries the calling context's qualified name as
/// its `CallerId` identity (ADR 0092); in bundle mode the compose root supplies
/// that name to `__makeSurface`, mirroring the `X-Bynk-Caller` header a Worker
/// reads at its entry. Delegates to the *same*
/// [`any_service_binds_caller`](crate::emitter::any_service_binds_caller) the
/// emitter's `emit_make_surface` calls, so the compose root and the surface can
/// never disagree on which providers take the extra `__caller` argument.
fn context_binds_caller(table: &UnitTable) -> bool {
    crate::emitter::any_service_binds_caller(table.services.values(), &table.actors)
}

// -- Small tree-construction helpers (#1327) ------------------------------
/// Sort key for a `{ns}Deps` entry, mirroring the pre-conversion code's own
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
/// shim's own. `None` when no handler of the context needs one.
fn bundle_process_exec_entry(table: &UnitTable) -> Option<(String, TsExpr)> {
    table
        .services
        .values()
        .any(|s| {
            s.handlers
                .iter()
                .any(|h| crate::emitter::call_handler_needs_exec(&table.exec_plan, h))
        })
        .then(|| ("__exec".to_string(), ident("__bynkProcessExec()")))
}

/// The composition root's import of `__bynkProcessExec` from the runtime.
fn process_exec_import() -> Vec<TsStmt> {
    emitter::runtime_files::runtime_import_stmts(
        false,
        &["__bynkProcessExec".to_string()],
        "./runtime.js",
    )
}

/// #3815: a unit's event names in name order. `table.events` is a `HashMap`,
/// so walking it directly put the Bundle `__eventsDispatch` cases in a
/// different order from one build to the next; the Workers fan-out walks a
/// `BTreeMap`, so the two targets now agree.
fn event_names_in_order(table: &UnitTable) -> Vec<&String> {
    let mut names: Vec<&String> = table.events.keys().collect();
    names.sort();
    names
}

#[allow(clippy::too_many_arguments)]
pub fn emit_composition_root(
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_callees: &HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
    // P6.x (#1232): see `EventSubscriberShape`'s own doc comment. Read by
    // `wants_envelope` below instead of walking a *different, already-
    // consumed* unit's raw `UnitTable` directly.
    unit_event_subscriber_shapes: &HashMap<String, HashMap<String, EventSubscriberShape>>,
    adapter_bindings: &HashMap<String, AdapterBinding>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // v0.19 (decision 0025, D1): when the program's closure reaches a
    // platform-native unit, composeApp takes an optional `env` and threads it
    // to env-taking first-party providers. A bundle on Cloudflare is a single
    // Worker with `env` at its entry; native-free programs emit the v0.18
    // no-parameter signature unchanged.
    thread_env: bool,
    // Events track, slice 0 (spine #936): the project-wide subscriber table,
    // computed once by the caller (Workers mode needs the same table for its
    // own per-Worker fan-out wiring, so it is shared rather than rebuilt).
    event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
) -> Option<TsProgram> {
    project_model::composition_root_trigger(
        kinds,
        unit_consumes,
        unit_consumes_aliases,
        unit_tables,
        unit_callees,
        unit_flattened,
    )?;

    let mut contexts: Vec<&String> = groups
        .keys()
        .filter(|n| kinds.get(*n) == Some(&UnitKind::Context))
        .collect();
    contexts.sort();

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

    // Build each context's deps and surface in dependency-respecting order:
    // a context that consumes another must come after the consumed context,
    // so its `surface` field can reference the already-built surface.
    let mut ordered: Vec<String> = Vec::new();
    let mut visited: HashSet<String> = HashSet::new();
    fn visit(
        node: &str,
        unit_consumes: &HashMap<String, Vec<String>>,
        visited: &mut HashSet<String>,
        out: &mut Vec<String>,
    ) {
        if visited.contains(node) {
            return;
        }
        visited.insert(node.to_string());
        if let Some(targets) = unit_consumes.get(node) {
            for t in targets {
                visit(t, unit_consumes, visited, out);
            }
        }
        out.push(node.to_string());
    }
    for c in &contexts {
        visit(c, unit_consumes, &mut visited, &mut ordered);
    }

    let mut body: Vec<TsStmt> = Vec::new();
    // #3541: whether a context took the bundle's process execution context.
    let mut process_exec_used = false;

    for ctx_name in &ordered {
        if kinds.get(ctx_name.as_str()) != Some(&UnitKind::Context) {
            continue;
        }
        let Some(table) = unit_tables.get(ctx_name.as_str()) else {
            continue;
        };
        // A context's deps object exists only to feed its `__makeSurface`; a
        // capability-only context (no services) needs neither (v0.15). The
        // bundle's `bynk.capability.not_provided` check skips the same ones.
        if !project_model::composed_in_bundle_root(table) {
            continue;
        }
        let ns = ctx_name.replace('.', "_");

        let mut deps_entries: Vec<(String, TsExpr)> = table
            .providers
            .keys()
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
        {
            let consumed = unit_consumes
                .get(ctx_name.as_str())
                .cloned()
                .unwrap_or_default();
            let aliases = unit_consumes_aliases
                .get(ctx_name.as_str())
                .cloned()
                .unwrap_or_default();
            let flattened = unit_flattened
                .get(ctx_name.as_str())
                .cloned()
                .unwrap_or_default();
            for (key, cctx) in handler_cross_caps(table, &consumed, &aliases, &flattened) {
                let expr = instantiate_provider_ts_expr(
                    &cctx,
                    &key,
                    unit_tables,
                    unit_consumes,
                    unit_consumes_aliases,
                    unit_flattened,
                    ProviderNamespaces::Bundle,
                    env_ident,
                    None, // Bundle mode has no inbound request (Decision A)
                    &mut referenced_units,
                );
                deps_entries.push((key, expr));
            }
        }
        // S4a (#1966): each derived capability's instance.
        let mut derived: Vec<&String> = table.derived.keys().collect();
        derived.sort();
        for cap in derived {
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
        // Events track, slice 0 (spine #936): a context whose handlers emit
        // gets an `__eventsDispatch` closure built here — Bundle/node mode
        // has no isolate boundary to cross, so this dispatches in-process
        // directly into each subscriber's own `on event` handler (`.event`,
        // the object method `emit_service` gives `HandlerKind::Event`),
        // reusing whatever deps that subscriber context already builds in
        // this same loop (referenced by name; the arrow function body isn't
        // evaluated until well after every `const ...Deps` in `composeApp`
        // has run, so declaration order here doesn't matter). A publisher
        // with no subscribers still gets the field — its type is required —
        // just with an empty switch.
        if unit_table_uses_emit(table, unit_callees.get(ctx_name)) {
            let mut cases = String::new();
            for name in event_names_in_order(table) {
                let Some(subs) = event_subscribers.get(&(ctx_name.clone(), name.clone())) else {
                    continue;
                };
                // ADR 0284: subscriber failure isolation — one subscriber's
                // throw is caught and logged, not left to abort delivery to
                // its siblings or propagate into the already-committed
                // publishing handler. Mirrors the Cloudflare fan-out DO's own
                // per-subscriber try/catch (`emit_events_fanout_do`) so the
                // two targets agree on this guarantee, not just on delivery.
                let calls: Vec<String> = subs
                    .iter()
                    .map(|(sub_ctx, sub_svc)| {
                        let sub_ns = sub_ctx.replace('.', "_");
                        // Events track, slice 2 (spine #936): the envelope
                        // is only forwarded to a subscriber that declared
                        // the optional second `env: EventEnvelope`
                        // parameter — a subscriber that kept `on event(e:
                        // E)` sees no change to its call at all. Slice 4
                        // (#985): also forwarded when the subscriber's
                        // protocol carries a `via schema(N)` clause, even if
                        // undeclared — `emit_service` inserts a synthetic
                        // `env` parameter in that case, and needs the value
                        // to line up positionally.
                        let wants_envelope = unit_event_subscriber_shapes
                            .get(sub_ctx)
                            .and_then(|m| m.get(sub_svc))
                            .is_some_and(|shape| shape.two_param_handler || shape.schema_dispatch);
                        // P7.2: deferred, not narrowed — a first attempt used the
                        // event's own bare name directly (`EventDecl::as_type_decl`
                        // keys the synthetic `TypeDecl` on it), on the theory that
                        // an event doubles as a named type. It broke real
                        // `tsc --strict` fixtures ("Cannot find name") — the bare
                        // name isn't necessarily in scope at this dispatch site
                        // (cross-context: publisher and subscriber are different
                        // units), the same qualification problem
                        // `ts_type_ref_qualified_ts_type` exists to solve for
                        // handler wrappers elsewhere in this crate. Needs the same
                        // kind of scoped qualification, not a bare name — more
                        // than a same-line fix.
                        let call_args = if wants_envelope {
                            "ev.payload as any, ev.envelope".to_string()
                        } else {
                            "ev.payload as any".to_string()
                        };
                        format!(
                            "try {{ await {sub_ns}.{sub_svc}.event({call_args}, {sub_ns}Deps); }} catch (e) {{ globalThis.console.error(\"EventsFanout delivery failed\", {{ event: ev.type, service: {sub_svc:?}, error: String(e) }}); }}"
                        )
                    })
                    .collect();
                cases.push_str(&format!("case {name:?}: {{ {} break; }} ", calls.join(" ")));
            }
            // Decision B (#1327): the closure's own body is a genuine block
            // statement (`for`/`switch`/`try`-`catch` nested), not an
            // expression. #1435 later gave `TsExpr::Arrow` a real block-body
            // shape (`TsArrowBody::Block`) — but that variant's own printer
            // reuses the compact-statement-list renderer `TsStmtKind::
            // InlineBlock` already shares (one physical line, semicolon-
            // separated top-level statements), which cannot flatten a nested
            // `for`/`switch` onto one line the way this closure's own real
            // content needs; new "flatten every nested statement to one
            // line" printer machinery for that would still be disproportionate
            // to what the 3 real fixtures reaching this closure need. Kept as
            // the same nested `format!` calls as before conversion, just fed
            // into a real `Arrow` node's `body` as one opaque `TsExpr::Ident`
            // — the same "opaque text carrier" precedent `workers.rs`'s/
            // `workers_entry.rs`'s own `claim_predicate_to_js` output already
            // uses.
            let dispatch_body =
                format!("{{ for (const ev of events) {{ switch (ev.type) {{ {cases}}} }} }}");
            let arrow = TsExpr::Arrow {
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
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(ident(dispatch_body)))),
            };
            deps_entries.push(("__eventsDispatch".to_string(), arrow));
        }
        if let Some(entry) = bundle_process_exec_entry(table) {
            process_exec_used = true;
            deps_entries.push(entry);
        }
        deps_entries.sort_by_cached_key(|(k, _)| deps_entry_sort_key(k));

        let mut surface_entries: Vec<(String, TsExpr)> = Vec::new();
        if let Some(targets) = unit_consumes.get(ctx_name.as_str()) {
            let aliases = unit_consumes_aliases
                .get(ctx_name.as_str())
                .cloned()
                .unwrap_or_default();
            let mut alias_for: HashMap<String, String> = HashMap::new();
            for (alias, target) in &aliases {
                alias_for.insert(target.clone(), alias.clone());
            }
            let mut sorted_targets = targets.clone();
            sorted_targets.sort();
            for t in &sorted_targets {
                let Some(other) = unit_tables.get(t) else {
                    continue;
                };
                if other.services.is_empty() {
                    continue;
                }
                let surface_key = alias_for
                    .get(t)
                    .cloned()
                    .unwrap_or_else(|| t.rsplit('.').next().unwrap_or(t.as_str()).to_string());
                let t_ns = t.replace('.', "_");
                // v0.54 (#655): a consumed context with an `on call … by c: Caller`
                // handler needs the *caller's* qualified name (this context) threaded
                // into that handler's deps as its `CallerId` identity (ADR 0092). The
                // shared `{t_ns}Surface` (built for the top-level entry with the
                // provider's own name) would carry the wrong caller, so build a
                // per-consumer surface instead. A caller-free provider keeps the
                // shared instance — byte-unchanged.
                let entry = if context_binds_caller(other) {
                    method_call(
                        ident(t_ns.clone()),
                        "__makeSurface",
                        vec![ident(format!("{t_ns}Deps")), str_lit(ctx_name.as_str())],
                    )
                } else {
                    ident(format!("{t_ns}Surface"))
                };
                surface_entries.push((surface_key, entry));
            }
        }
        if !surface_entries.is_empty() {
            deps_entries.push(("surface".to_string(), TsExpr::object(surface_entries)));
        }
        // #1327: the pre-conversion `format!("  const {ns}Deps = {{ {} }};",
        // deps_entries.join(", "))` template always has a space on each side
        // of its `{}` slot — with zero entries that literally produces
        // `"{  }"` (a *double* space), not the tight `"{}"` the ordinary
        // single-line `TsExpr::Object` empty-entries shortcut renders — the
        // same real, reachable quirk `workers.rs`'s own conversion (#1321)
        // found and carried for its own `deps` object, reachable here too (a
        // services-having context with no providers, no cross-caps, no
        // emit, and no consumed-service surface — `98_cross_context_call_
        // with_alias` and 6 other real fixtures hit exactly this).
        let deps_init = if deps_entries.is_empty() {
            ident("{  }")
        } else {
            TsExpr::object(deps_entries)
        };
        body.push(const_(format!("{ns}Deps"), deps_init));
        if !table.services.is_empty() {
            // The top-level entry addresses the context directly; there is no
            // calling context, so a `by c: Caller` handler reached this way reads
            // the context's own qualified name (a stable, non-empty `CallerId`
            // within the single-trust-domain bundle).
            let mut make_surface_args = vec![ident(format!("{ns}Deps"))];
            if context_binds_caller(table) {
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
    for ctx_name in &contexts {
        let Some(table) = unit_tables.get(ctx_name.as_str()) else {
            continue;
        };
        if table.services.is_empty() {
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
    if process_exec_used {
        program.stmts.extend(process_exec_import());
    }

    // Import every context as a namespace.
    for ctx_name in &contexts {
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
    let mut consumed_adapters: Vec<String> = unit_consumes
        .iter()
        .filter(|(name, _)| kinds.get(*name) == Some(&UnitKind::Context))
        .flat_map(|(_, targets)| targets.iter().cloned())
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
