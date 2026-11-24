//! The per-Worker emission (1B.2d, #4028 Decision F): each context's `compose.ts`,
//! entry point, events fan-out class, contract and secrets manifests, and its
//! Worker's `wrangler.toml` and sidecars, rendered from the binding model the
//! build derived once ([`WorkerBindings`](super::WorkerBindings)). The driver builds a [`WorkersEmit`]
//! from the checked project and its options and calls [`WorkersEmit::emit_all`];
//! it decides nothing about which bindings a Worker has, or where it is.

use super::{self as emitter};
use crate::emit_model::{Document, ImportExt, StagedFile};
use crate::emit_model::{
    called_cross_context_services, worker_assets_dir, worker_reaches_wasm_module,
};
use bynk_check::checker::ExprId;
use bynk_check::firstparty;
use bynk_check::project_model::AdapterBinding;
use bynk_check::schema_registry::AgentVersions;
use bynk_check::symbols::UnitTable;
use bynk_project::AttributedError;
use bynk_project::UnitKind;
use bynk_project::discovery::ParsedFile;
use bynk_project::worker_layout::WorkerLayout;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

/// #3581: the agent classes a `bynk dev` build's Worker answers the inspect route
/// for, sorted; empty unless the build asks for inspection.
fn inspect_classes(on: bool, table: &UnitTable) -> Vec<String> {
    let mut classes: Vec<String> = if on {
        table.agents.keys().cloned().collect()
    } else {
        Vec::new()
    };
    classes.sort();
    classes
}

/// Everything a context's Workers emit reads from the checked project and the
/// build's options (#3562, G2b): the same inputs the driver's emit loop used as
/// locals, gathered so [`WorkersEmit::emit_all`] produces every context's files
/// and the driver only supplies them.
#[derive(Clone, Copy)]
pub struct WorkersEmit<'a> {
    pub adapter_bindings: &'a HashMap<String, AdapterBinding>,
    pub agent_versions: &'a HashMap<String, AgentVersions>,
    pub asset_files: Option<&'a BTreeMap<String, Vec<u8>>>,
    pub asset_served: &'a BTreeMap<String, String>,
    pub bindings: &'a emitter::WorkerBindings,
    pub compatibility_flags: &'a [String],
    pub contracts_lock: &'a bynk_project::contracts_lock::ContractsLock,
    pub d1_migrations: &'a BTreeMap<String, Vec<bynk_check::d1_migrations::MigrationFile>>,
    pub compose_plan: &'a emitter::ComposePlan,
    pub groups: &'a BTreeMap<String, Vec<usize>>,
    pub harness_clock_contexts: &'a HashSet<String>,
    pub import_ext: ImportExt,
    pub inspect_agents: bool,
    pub kinds: &'a BTreeMap<String, UnitKind>,
    pub layout: &'a WorkerLayout,
    pub parsed: &'a [ParsedFile],
    pub trace_requests: bool,
    pub unit_callees: &'a HashMap<String, HashMap<ExprId, bynk_check::checker::Callee>>,
    pub unit_consumed_context_types: &'a HashMap<String, HashSet<String>>,
    pub unit_consumes: &'a HashMap<String, Vec<String>>,
    pub unit_consumes_aliases: &'a HashMap<String, HashMap<String, String>>,
    pub unit_flattened: &'a HashMap<String, HashMap<String, String>>,
    pub unit_tables: &'a HashMap<String, UnitTable>,
    pub unit_uses: &'a HashMap<String, Vec<String>>,
    pub workers_dev: Option<bool>,
}

impl WorkersEmit<'_> {
    /// The files of one context (a Worker's, or one member's of a group's):
    /// `compose.ts`, its entry, its fan-out class and its manifests, pushed
    /// onto `compiled`; contract-lock warnings onto `lock_warnings`. What it
    /// contributes to its Worker's `wrangler.toml` comes back as [`MemberParts`],
    /// for [`Self::finish_worker`].
    fn emit_member(
        &self,
        ctx_name: &String,
        table: &UnitTable,
        compiled: &mut Vec<StagedFile>,
        lock_warnings: &mut Vec<AttributedError>,
        inspect_lists: bool,
    ) -> MemberParts {
        let WorkersEmit {
            adapter_bindings,
            agent_versions,
            bindings: model,
            compose_plan,
            contracts_lock,
            groups,
            harness_clock_contexts,
            import_ext,
            inspect_agents,
            kinds,
            layout,
            parsed,
            trace_requests,
            unit_callees,
            unit_consumed_context_types,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            unit_tables,
            unit_uses,
            ..
        } = *self;
        let consumes_targets = unit_consumes.get(ctx_name).cloned().unwrap_or_default();
        // v0.177 (#643): the callee's own view of each of its `on call`
        // contracts, hashed from *its own* namespace — the same table
        // (`contract_types_for`) a caller reaches through
        // `consumed_types[ctx_name]`, so the two sides cannot disagree.
        let own_types = bynk_check::symbols::contract_types_for(
            ctx_name,
            unit_tables,
            unit_uses,
            unit_consumes,
        )
        .0;
        let own_contracts = bynk_check::contract::own_contract_hashes(table, &own_types);
        // #2554: the older contracts this callee also accepts, each
        // re-checked against the contract it has now.
        let accepted = bynk_check::contract_compat::own_accepted(
            table,
            &own_types,
            |service| contracts_lock.accepted(ctx_name, service),
            bynk_project::contracts_lock::ACCEPTED_CAP,
        );
        for (service, why) in &accepted.stale {
            lock_warnings.push(bynk_project::contracts_lock::stale_warning(
                ctx_name, service, why,
            ));
        }
        for (service, count) in &accepted.oversized {
            lock_warnings.push(bynk_project::contracts_lock::oversized_warning(
                ctx_name, service, *count,
            ));
        }
        let binding_modules: HashMap<String, String> = adapter_bindings
            .iter()
            .map(|(n, b)| {
                (
                    n.clone(),
                    emitter::ts_specifier(&b.output_path.with_extension("js")),
                )
            })
            .collect();
        // Locale capability track, slice 2 (#882, Decision A): real
        // negotiation is Cloudflare-only — `BuildTarget::Workers`
        // isn't itself restricted to `Platform::Cloudflare`, so a
        // hypothetical `--target workers --platform node` project
        // must not try to pass 3 args to Node's still-0-arg
        // `LocaleProvider`.
        let is_cloudflare_binding = adapter_bindings
            .get(firstparty::BYNK_UNIT)
            .is_some_and(|b| {
                b.output_path.file_name()
                    == Some(std::ffi::OsStr::new(
                        firstparty::Platform::Cloudflare.bynk_binding_filename(),
                    ))
            });
        let bundle = bynk_check::symbols::detect_context_message_bundle(
            ctx_name, unit_uses, groups, kinds, parsed,
        );
        use bynk_check::symbols::ContextMessageBundle as Bundle;
        let locale_bundle_info = match &bundle {
            Bundle::One(info) if is_cloudflare_binding => Some(info),
            Bundle::One(_) | Bundle::None | Bundle::Many(_) => None,
        };
        // #1187's slice 6 plumbing: computed once, reused by every
        // Workers-target emitter below that needs it.
        let ctx_uses_emit = planned_unit(compose_plan, ctx_name).publishes;
        // #1944: every binding this Worker declares, once — its `Env`
        // interface and its `wrangler.toml` are both built from it, and the
        // duplicate check reads the same list (1C.1b).
        let bindings = model.of(ctx_name);
        // What this context calls on each dependency (#1961): its contracts
        // manifest, below.
        let called =
            called_cross_context_services(table, &consumes_targets, unit_callees.get(ctx_name));
        // #2719: the context's handlers that hand work to the execution
        // context, from the checker's needs-context set.
        let ctx_uses_exec = planned_unit(compose_plan, ctx_name).exec.any_handler;
        // #2969: the file of each used commons that declares each of its
        // functions and constants, so `compose.ts` imports a commons
        // constant from its declaring module, as `handlers.ts` does,
        // when the commons is split across files.
        let used_decl_paths: HashMap<String, HashMap<String, PathBuf>> = unit_uses
            .get(ctx_name)
            .into_iter()
            .flatten()
            .filter_map(|u| {
                let indices = groups.get(u)?;
                let index = bynk_check::symbols::build_file_decl_index(indices, parsed);
                Some((u.clone(), index.fns))
            })
            .collect();
        let (compose_ts, needs_locale_request) = emitter::emit_worker_compose(
            ctx_name,
            compose_plan,
            self.layout,
            table,
            unit_tables,
            &binding_modules,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            locale_bundle_info,
            import_ext,
            bindings,
            harness_clock_contexts.contains(ctx_name),
            unit_uses
                .get(ctx_name)
                .map(Vec::as_slice)
                .unwrap_or_default(),
            &used_decl_paths,
        );
        // #2136: a context a `system` suite targets gets the test
        // build's scheduled-result hook. A deployable build strips
        // suites (`tests` off), so it never does.
        let system_driven = parsed
            .iter()
            .filter_map(ParsedFile::integration)
            .any(|d| d.target.joined() == *ctx_name);
        // #2252: the types this context takes from the contexts it
        // consumes, recorded when its handlers were checked.
        let no_consumed_types = HashSet::new();
        let consumed_types = unit_consumed_context_types
            .get(ctx_name)
            .unwrap_or(&no_consumed_types);
        let migratable = migratable_agents(ctx_name, table, agent_versions);
        let inspect_classes = inspect_classes(inspect_agents, table);
        let entry_ts = emitter::emit_worker_entry(
            ctx_name,
            self.layout,
            table,
            &own_contracts,
            &accepted.hashes,
            needs_locale_request,
            ctx_uses_emit,
            system_driven,
            consumed_types,
            &migratable,
            ctx_uses_exec,
            &bynk_check::symbols::entry_types_for(
                ctx_name,
                unit_tables,
                unit_uses,
                (unit_consumes, consumed_types),
            ),
            trace_requests,
            (&inspect_classes, inspect_lists),
        );
        // P6.x cutover slice 2 (#1191): collected here, not inside
        // `emit_wrangler_toml` itself, so that function's own file
        // needs no raw-AST match. P6.46 (#1137): the walk itself
        // relocated to `bynk_check::symbols::cron_and_queue_triggers`
        // — a pure function of `table`, `project.rs`'s own remaining
        // AST contact here was incidental to where the loop happened
        // to be written, not structural.
        let (crons, queues) = bynk_check::symbols::cron_and_queue_triggers(table);
        let queue_deliveries = bynk_check::symbols::queue_deliveries(table);
        // Arc C slice 4 (#1323): `emit_worker_entry` returns a real
        // `TsProgram` directly — the construction site's own
        // `Verbatim`/`NotYetConverted` wrap is gone, matching the
        // precedent `events_fanout.rs`'s and `workers.rs`'s own
        // construction sites already set. A group member's entry is its own
        // file, which the group's entry imports.
        compiled.push(StagedFile {
            output_path: layout.member_file(ctx_name, "index.ts"),
            document: Document::Ts(entry_ts),
            source_map: None,
            debug_metadata: None,
        });
        let mut classes: Vec<String> = table.agents.keys().cloned().collect();
        classes.sort();
        // Events track, slice 0: this context's fan-out Durable
        // Object — emitted only when it actually publishes (mirrors
        // `emit_worker_compose`'s own `unit_table_uses_emit` gate on
        // `deps.__eventsDispatch`, so the two never disagree about
        // whether `env.EVENTS_FANOUT` is real).
        // #3562 (G3-1): a group's members share one fan-out, which the group
        // emits from every publishing member's routes (`group_entry`).
        let in_group = layout.is_member(ctx_name);
        if ctx_uses_emit && !in_group {
            // Arc C's own first real conversion slice (#1317):
            // `emit_events_fanout_do` returns a real `TsProgram`
            // directly — the first `bynk-emit` construction site
            // that reaches `Document::Ts` with no `Verbatim` wrap.
            let fanout_program = emitter::emit_events_fanout_do(ctx_name, &compose_plan.events);
            compiled.push(StagedFile {
                output_path: layout.member_file(ctx_name, "events_fanout.ts"),
                document: Document::Ts(fanout_program),
                source_map: None,
                debug_metadata: None,
            });
            classes.push(emitter::wrangler::EVENTS_FANOUT_CLASS_NAME.to_string());
        }
        // A `bynk dev` build's registry class is exported once for the Worker, by
        // the member that answers the listing.
        if inspect_lists {
            classes.push("BynkInspectRegistry".to_string());
        }
        // Arc C slice 3 (#1321): `emit_worker_compose` returns a
        // real `TsProgram` directly — the second `bynk-emit`
        // construction site (after `events_fanout.ts`'s own,
        // #1317) that reaches `Document::Ts` with no `Verbatim`
        // wrap.
        compiled.push(StagedFile {
            output_path: layout.member_file(ctx_name, "compose.ts"),
            document: Document::Ts(compose_ts),
            source_map: None,
            debug_metadata: None,
        });
        let (contracts, secrets) =
            self.emit_contract_manifests(ctx_name, table, &called, &own_types, &accepted, compiled);
        MemberParts {
            context: ctx_name.clone(),
            crons,
            queues,
            queue_deliveries,
            addresses: bynk_check::email::routed_addresses(table),
            has_events: own_handles_events(table),
            has_migrate: !migratable.is_empty(),
            has_inspect: !inspect_classes.is_empty(),
            inspect_classes,
            classes,
            contracts,
            secrets,
            publishes: ctx_uses_emit && in_group,
        }
    }

    /// Whether `ctx_name`'s entry answers its Worker's `/_bynk/inspect/agents`
    /// listing (`bynk dev` only): the first member of its group that hosts an
    /// agent does, so the registry is listed once. False outside a group, where
    /// the one entry answers everything.
    fn lists_inspection(&self, ctx_name: &str) -> bool {
        let WorkersEmit {
            inspect_agents,
            layout,
            unit_tables,
            ..
        } = *self;
        let Some(members) = layout
            .is_member(ctx_name)
            .then(|| layout.group_members(&layout.dir(ctx_name)))
            .flatten()
        else {
            return false;
        };
        members
            .iter()
            .find(|m| {
                unit_tables
                    .get(m.as_str())
                    .is_some_and(|t| !inspect_classes(inspect_agents, t).is_empty())
            })
            .is_some_and(|first| first == ctx_name)
    }

    /// Every context's files, then every Worker's: a Worker is a context's own,
    /// or a group's members (in the order the manifest lists them).
    pub fn emit_all(
        &self,
        compiled: &mut Vec<StagedFile>,
        lock_warnings: &mut Vec<AttributedError>,
        staged_assets: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) {
        let WorkersEmit {
            kinds,
            layout,
            unit_tables,
            ..
        } = *self;
        let mut parts: BTreeMap<&String, (MemberParts, &UnitTable)> = BTreeMap::new();
        for (ctx_name, kind) in kinds {
            if *kind != UnitKind::Context {
                continue;
            }
            let Some(table) = unit_tables.get(ctx_name) else {
                continue;
            };
            let lists = self.lists_inspection(ctx_name);
            let member = self.emit_member(ctx_name, table, compiled, lock_warnings, lists);
            parts.insert(ctx_name, (member, table));
        }
        let mut done: HashSet<String> = HashSet::new();
        for ctx_name in parts.keys() {
            let dir = layout.dir(ctx_name);
            if !done.insert(dir.clone()) {
                continue;
            }
            let order = layout
                .group_members(&dir)
                .map_or_else(|| vec![(*ctx_name).clone()], <[String]>::to_vec);
            let worker: Vec<(&MemberParts, &UnitTable)> = order
                .iter()
                .filter_map(|c| parts.get(c))
                .map(|(member, table)| (member, *table))
                .collect();
            self.finish_worker(&worker, compiled, staged_assets);
        }
    }

    /// One Worker's whole-Worker files, from its members' parts: `wrangler.toml`
    /// (their bindings, crons, queues and addresses merged, an identical
    /// capability one binding), the staged binding files, the assets it serves,
    /// `containers.ts`, and, for a group, the entry that dispatches to its
    /// members. A context's own Worker is a group of one, whose entry is the
    /// member's own.
    fn finish_worker(
        &self,
        members: &[(&MemberParts, &UnitTable)],
        compiled: &mut Vec<StagedFile>,
        staged_assets: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) {
        let WorkersEmit {
            compatibility_flags,
            d1_migrations,
            layout,
            workers_dev,
            ..
        } = *self;
        let first = members[0].0.context.as_str();
        let bindings = self.bindings.of_worker(&layout.dir(first));
        let union = |of: fn(&MemberParts) -> &Vec<String>| -> Vec<String> {
            let mut all: Vec<String> = members.iter().flat_map(|(p, _)| of(p).clone()).collect();
            all.sort();
            all.dedup();
            all
        };
        let mut tombstones = emitter::ClassTombstones::default();
        for (_, table) in members {
            let one = emitter::ClassTombstones::of(table);
            tombstones.renamed.extend(one.renamed);
            tombstones.deleted.extend(one.deleted);
        }
        tombstones.renamed.sort();
        tombstones.deleted.sort();
        tombstones.deleted.dedup();
        let mut wrangler_doc = emitter::emit_wrangler_toml(
            first,
            layout,
            bindings,
            &union(|p| &p.crons),
            &union(|p| &p.queues),
            &members
                .iter()
                .flat_map(|(p, _)| p.queue_deliveries.iter().cloned())
                .collect::<Vec<_>>(),
            &union(|p| &p.addresses),
            compatibility_flags,
            workers_dev,
            &tombstones,
            d1_migrations,
        );
        stage_binding_files(bindings, layout, first, d1_migrations, compiled);
        let tables: Vec<(&str, &UnitTable)> = members
            .iter()
            .map(|(parts, table)| (parts.context.as_str(), *table))
            .collect();
        self.stage_worker_extras(&tables, &mut wrangler_doc, staged_assets);
        // #3477: the Durable Object classes of this Worker's containers.
        if let Some(program) = emitter::emit_containers_ts(bindings) {
            compiled.push(StagedFile {
                output_path: layout.worker_file(first, "containers.ts"),
                document: Document::Ts(program),
                source_map: None,
                debug_metadata: None,
            });
        }
        compiled.push(StagedFile {
            output_path: layout.worker_file(first, "wrangler.toml"),
            document: Document::Toml(wrangler_doc),
            source_map: None,
            debug_metadata: None,
        });
        if layout.is_member(first) {
            if let Some(text) = agents_manifest(members) {
                compiled.push(StagedFile {
                    output_path: layout.worker_file(first, crate::AGENTS_MANIFEST),
                    document: Document::Json(text),
                    source_map: None,
                    debug_metadata: None,
                });
            }
            self.stage_group_manifests(first, members, compiled);
            // #3562 (G3-1): the group's one events fan-out, for every member that
            // publishes, in declaration order.
            let publishers: Vec<&str> = members
                .iter()
                .filter(|(parts, _)| parts.publishes)
                .map(|(parts, _)| parts.context.as_str())
                .collect();
            if !publishers.is_empty() {
                compiled.push(StagedFile {
                    output_path: layout.worker_file(first, "events_fanout.ts"),
                    document: Document::Ts(emitter::emit_group_events_fanout_do(
                        &layout.dir(first),
                        &publishers,
                        &self.compose_plan.events,
                    )),
                    source_map: None,
                    debug_metadata: None,
                });
            }
            if let Some(text) = group_inspect_manifest(members) {
                compiled.push(StagedFile {
                    output_path: layout.worker_file(first, crate::INSPECT_MEMBERS_FILE),
                    document: Document::Json(text),
                    source_map: None,
                    debug_metadata: None,
                });
            }
            let entry = self.group_entry(first, members, bindings);
            compiled.push(StagedFile {
                output_path: layout.worker_file(first, "index.ts"),
                document: Document::Ts(entry),
                source_map: None,
                debug_metadata: None,
            });
        }
    }

    /// A group's one `bynk-contracts.json` and `bynk-secrets.json`, which `deploy`
    /// reads beside the Worker: the members' merged (#3562).
    fn stage_group_manifests(
        &self,
        first: &str,
        members: &[(&MemberParts, &UnitTable)],
        compiled: &mut Vec<StagedFile>,
    ) {
        let names: Vec<String> = members.iter().map(|(p, _)| p.context.clone()).collect();
        let contracts = emitter::contracts::ContractParts::merged(
            members.iter().map(|(p, _)| p.contracts.clone()),
            &names,
        );
        let secrets =
            emitter::secrets::SecretsParts::merged(members.iter().map(|(p, _)| p.secrets.clone()));
        for (file, text) in [
            (emitter::contracts::CONTRACTS_MANIFEST, contracts.render()),
            (emitter::secrets::SECRETS_MANIFEST, secrets.render()),
        ] {
            if let Some(text) = text {
                compiled.push(StagedFile {
                    output_path: self.layout.worker_file(first, file),
                    document: Document::Json(text),
                    source_map: None,
                    debug_metadata: None,
                });
            }
        }
    }

    /// A group's `index.ts` (see `emitter::workers_group`).
    fn group_entry(
        &self,
        first: &str,
        members: &[(&MemberParts, &UnitTable)],
        bindings: &[emitter::bindings::WorkerBinding],
    ) -> bynk_ts::TsProgram {
        let layout = self.layout;
        let facts: Vec<emitter::GroupMember> = members
            .iter()
            .map(|(parts, _)| emitter::GroupMember {
                context: parts.context.clone(),
                specifier: layout.own_module_specifier(&parts.context, "index.ts"),
                crons: parts.crons.clone(),
                queues: parts.queues.clone(),
                addresses: parts.addresses.clone(),
                classes: parts.classes.clone(),
            })
            .collect();
        emitter::emit_group_entry(&emitter::GroupEntry {
            key: &layout.dir(first),
            members: &facts,
            container_classes: &emitter::container_class_names(bindings),
            has_fanout: members.iter().any(|(parts, _)| parts.publishes),
            has_events: members.iter().any(|(parts, _)| parts.has_events),
            has_migrate: members.iter().any(|(parts, _)| parts.has_migrate),
            has_inspect: members.iter().any(|(parts, _)| parts.has_inspect),
        })
    }

    /// The files `bynk deploy` reads beside a Worker: the contracts manifest
    /// (the hashes its entry enforces and what it expects of each dependency),
    /// the typed client a foreign caller imports, and the secrets manifest.
    fn emit_contract_manifests(
        &self,
        ctx_name: &str,
        table: &UnitTable,
        called: &BTreeMap<String, BTreeSet<String>>,
        own_types: &HashMap<String, Arc<bynk_check::project_model::TypeDecl>>,
        accepted: &bynk_check::contract_compat::ContextAccepted,
        compiled: &mut Vec<StagedFile>,
    ) -> (
        emitter::contracts::ContractParts,
        emitter::secrets::SecretsParts,
    ) {
        let WorkersEmit {
            layout,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
            unit_tables,
            unit_uses,
            ..
        } = *self;
        // v0.177 (#643): the contract hashes this context's entry
        // enforces, written where `deploy` can read them — so a skew is
        // refused at the push rather than discovered by live traffic.
        // #2750: the SAME table the entry's hashes (`own_contracts`, in
        // `emit_member`) and every caller's `expects` are computed from —
        // `own_types`, bound once from `contract_types_for`. A second
        // binding from `combined_types_for` here left a consumed
        // context's exported type unresolved in `provides`.
        // What this context expects of each dependency — the same hash
        // it stamps at each call site, computed the same way: in the
        // *dependency's* namespace, from the dependency's own table.
        //
        // Only the services this context actually **calls**. Recording
        // everything the dependency provides would refuse a deploy over a
        // service this caller never touches — a skew its runtime check
        // could never fire on. See `called_cross_context_services`.
        let mut expects: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<String, String>,
        > = std::collections::BTreeMap::new();
        let mut expects_shapes: std::collections::BTreeMap<
            String,
            std::collections::BTreeMap<String, String>,
        > = std::collections::BTreeMap::new();
        for (dep, services) in called {
            let Some(dep_table) = unit_tables.get(dep) else {
                continue;
            };
            let dep_types =
                bynk_check::symbols::contract_types_for(dep, unit_tables, unit_uses, unit_consumes)
                    .0;
            let all = bynk_check::contract::own_contract_hashes(dep_table, &dep_types);
            let hashes: std::collections::BTreeMap<String, String> = all
                .into_iter()
                .filter(|(svc, _)| services.contains(svc))
                .collect();
            if !hashes.is_empty() {
                // #2485: the structure behind each hash, from the same
                // dependency table.
                let dep_shapes: std::collections::BTreeMap<String, String> =
                    bynk_check::contract_compat::own_contract_shapes(dep_table, &dep_types)
                        .into_iter()
                        .filter(|(svc, _)| hashes.contains_key(svc))
                        .collect();
                expects_shapes.insert(dep.clone(), dep_shapes);
                expects.insert(dep.clone(), hashes);
            }
        }
        // I4 (#1970): what each declared foreign caller's generated
        // client embeds, recorded beside `provides` so `deploy` can
        // hold it against the caller's pin.
        let foreign = emitter::client::foreign_contracts(table, own_types);
        let contracts = emitter::contracts::ContractParts {
            provides: bynk_check::contract::own_contract_hashes(table, own_types),
            expects,
            foreign,
            provides_shapes: bynk_check::contract_compat::own_contract_shapes(table, own_types),
            expects_shapes,
            accepts: accepted.hashes.clone(),
        };
        // A group's members share one Worker and so one manifest, merged by
        // `finish_worker` (#3562).
        if !layout.is_member(ctx_name)
            && let Some(manifest) = contracts.render()
        {
            compiled.push(StagedFile {
                output_path: layout.member_file(ctx_name, emitter::contracts::CONTRACTS_MANIFEST),
                document: Document::Json(manifest),
                source_map: None,
                debug_metadata: None,
            });
        }

        // I4 (#1970): the typed TypeScript client a foreign caller
        // imports, when the context declares one.
        if let Some(client) = emitter::client::emit_client(ctx_name, table, own_types) {
            compiled.push(StagedFile {
                output_path: layout.member_file(ctx_name, emitter::client::CLIENT_FILE),
                document: Document::Ts(client),
                source_map: None,
                debug_metadata: None,
            });
        }

        // v0.172 (ADR 0195 D5): the secret names this Worker's handlers
        // will read from `env`, for `deploy` to check before it pushes.
        // Emitted from the same seams the entry lowers, so the two
        // cannot describe different Workers.
        //
        // v0.173 (ADR 0196): plus the literal `bynk.Secrets` names it
        // reads, and whether that list is everything. The walk is here
        // rather than in the checker because it needs `unit_flattened`
        // to answer *whose* `Secrets` this is (D4) and the warning sink
        // to say when it cannot know a name — and `bynk-check` has
        // neither. Absent when there is nothing at all to say (D5).
        // The warnings half is dropped here: `run_checks` already raised
        // it, on the analyse path the editor shares.
        let reads = emitter::secrets::secret_reads(
            ctx_name,
            unit_tables,
            unit_consumes,
            unit_consumes_aliases,
            unit_flattened,
        );
        let secrets = emitter::secrets::SecretsParts::of(table, &reads);
        if !layout.is_member(ctx_name)
            && let Some(manifest) = secrets.render()
        {
            compiled.push(StagedFile {
                output_path: layout.member_file(ctx_name, emitter::secrets::SECRETS_MANIFEST),
                document: Document::Json(manifest),
                source_map: None,
                debug_metadata: None,
            });
        }
        (contracts, secrets)
    }

    /// What a Worker's `wrangler.toml` gains beyond its bindings: the static
    /// files it serves under its assets directory (with `_headers` and the
    /// `[assets]` block), the Workers Cache for an `EdgeCache` instance, and the
    /// `CompiledWasm` rule for a WebAssembly module its adapter closure reaches.
    /// A group's members share one Worker, so each is asked and each block is
    /// added once.
    fn stage_worker_extras(
        &self,
        members: &[(&str, &UnitTable)],
        wrangler_doc: &mut crate::emitter::toml_doc::TomlDocument,
        staged_assets: &mut BTreeMap<PathBuf, Vec<u8>>,
    ) {
        let WorkersEmit {
            adapter_bindings,
            asset_files,
            asset_served,
            kinds,
            layout,
            unit_consumes,
            unit_tables,
            unit_uses,
            ..
        } = *self;
        // #1883, slice 1b: the static files this Worker serves — its
        // own and those of every commons its `uses` closure reaches —
        // under its assets directory, with `_headers` and the
        // `[assets]` block. A Worker with none gets neither.
        let mut deployed = BTreeMap::new();
        for (ctx_name, _) in members {
            deployed.extend(bynk_check::assets::deployed_assets(
                ctx_name,
                unit_uses,
                unit_consumes,
                unit_tables,
                asset_served,
            ));
        }
        if !deployed.is_empty() {
            let dir = worker_assets_dir(&layout.dir(members[0].0));
            for (served, source) in &deployed {
                let bytes = asset_files
                    .and_then(|files| files.get(source))
                    .cloned()
                    .unwrap_or_else(|| {
                        panic!("bynk internal error: deployed asset `{source}` was never read")
                    });
                staged_assets.insert(dir.join(served.trim_start_matches('/')), bytes);
            }
            staged_assets.insert(
                dir.join("_headers"),
                emitter::wrangler::asset_headers().into_bytes(),
            );
            wrangler_doc.push_block(emitter::wrangler::assets_block());
        }
        // L7 (#2414): an `EdgeCache` instance turns Workers Cache on.
        if members
            .iter()
            .any(|(_, table)| table.derived.values().any(|d| d.is_edge_cache()))
        {
            wrangler_doc.push_block(emitter::wrangler::edge_cache_block());
        }
        // #2321: a Worker whose adapter closure reaches an included
        // WebAssembly module gets the explicit `CompiledWasm` rule.
        if members.iter().any(|(ctx_name, _)| {
            worker_reaches_wasm_module(ctx_name, unit_consumes, kinds, adapter_bindings)
        }) {
            wrangler_doc.push_block(emitter::wrangler::wasm_rule_block());
        }
    }
}

/// What one context contributes to its Worker's `wrangler.toml` and, in a
/// group, to the group's entry ([`WorkersEmit::emit_member`]).
pub(crate) struct MemberParts {
    context: String,
    crons: Vec<String>,
    queues: Vec<String>,
    /// #3877: its consumers' `delivery { }` settings.
    queue_deliveries: Vec<bynk_check::symbols::QueueDelivery>,
    addresses: Vec<String>,
    /// Whether it subscribes to events (answers `/_bynk/event/`).
    has_events: bool,
    /// Whether it hosts a migratable agent (answers `/_bynk/migrate/`).
    has_migrate: bool,
    /// Whether it hosts an agent a `bynk dev` build can inspect.
    has_inspect: bool,
    /// The agent classes a `bynk dev` build can inspect.
    inspect_classes: Vec<String>,
    /// The Durable Object classes its entry exports.
    classes: Vec<String>,
    /// Its contract and secrets manifests, merged into the Worker's own when it is
    /// in a group (#3562).
    contracts: emitter::contracts::ContractParts,
    secrets: emitter::secrets::SecretsParts,
    /// Whether a group member publishes, so the group's one fan-out routes its
    /// events (#3562).
    publishes: bool,
}

/// #3562 (G6): which member owns which agent class in a `bynk dev` group, for
/// `bynk dev inspect`, which signs each request for the member that answers it: the
/// listing for the first member that hosts an agent, a read for the class's owner.
/// `None` when no member hosts one (and in a deploy build, which has no inspect).
///
/// Written as `serde_json::to_string_pretty` would (sorted keys, two-space indent), by
/// hand: the shape is two fields, and this crate takes no JSON dependency.
fn group_inspect_manifest(members: &[(&MemberParts, &UnitTable)]) -> Option<String> {
    // Every member is named, those hosting no agent with no classes, so that
    // `bynk dev inspect --context <member>` finds the group for any of them.
    members.iter().any(|(p, _)| p.has_inspect).then(|| {
        let all: Vec<(&str, &[String])> = members
            .iter()
            .map(|(p, _)| (p.context.as_str(), p.inspect_classes.as_slice()))
            .collect();
        render_inspect_members(&all)
    })
}

/// The listing of [`group_inspect_manifest`], from each member's context and classes.
fn render_inspect_members(members: &[(&str, &[String])]) -> String {
    let all: Vec<String> = members
        .iter()
        .map(|(context, classes)| {
            format!(
                "    {{\n      \"classes\": {},\n      \"context\": {}\n    }}",
                json_string_array(classes, 6),
                bynk_project::json_string(context),
            )
        })
        .collect();
    format!("{{\n  \"members\": [\n{}\n  ]\n}}\n", all.join(",\n"))
}

/// #3562 (G5): which context of a Worker owns which agent class, for `bynk deploy`: a
/// class's state belongs to its Worker's script, so a context that hosts agents moving to
/// another script must not be redeployed over empty state unannounced. Only a group has one
/// (a Worker that is one context owns its own classes). `None` when no member hosts an agent.
///
/// Written as `serde_json::to_string_pretty` would (sorted keys, two-space indent).
fn agents_manifest(members: &[(&MemberParts, &UnitTable)]) -> Option<String> {
    let agents: BTreeMap<&str, Vec<String>> = members
        .iter()
        .filter_map(|(parts, table)| {
            let mut classes: Vec<String> = table.agents.keys().cloned().collect();
            classes.sort();
            (!classes.is_empty()).then_some((parts.context.as_str(), classes))
        })
        .collect();
    (!agents.is_empty()).then(|| render_agents(&agents))
}

/// The manifest of [`agents_manifest`], from each hosting context's classes.
fn render_agents(agents: &BTreeMap<&str, Vec<String>>) -> String {
    let entries: Vec<String> = agents
        .iter()
        .map(|(context, classes)| {
            format!(
                "    {}: {}",
                bynk_project::json_string(context),
                json_string_array(classes, 4)
            )
        })
        .collect();
    format!("{{\n  \"agents\": {{\n{}\n  }}\n}}\n", entries.join(",\n"))
}

/// A JSON array of strings, one item per line at `indent` + 2 spaces, as
/// `serde_json::to_string_pretty` lays it out; `[]` when empty.
fn json_string_array(items: &[String], indent: usize) -> String {
    if items.is_empty() {
        return "[]".to_string();
    }
    let pad = " ".repeat(indent);
    let lines: Vec<String> = items
        .iter()
        .map(|item| format!("{pad}  {}", bynk_project::json_string(item)))
        .collect();
    format!("[\n{}\n{pad}]", lines.join(",\n"))
}

/// Whether any service of `table` has an `on event` handler.
fn own_handles_events(table: &UnitTable) -> bool {
    table.services.values().any(|s| {
        s.handlers.iter().any(|h| {
            matches!(
                bynk_model::lower_handler_kind_ir(&h.kind),
                bynk_model::IrHandlerKind::Event
            )
        })
    })
}

/// The plan's unit for a context the driver is emitting. Every grouped context
/// has a checked `UnitTable`, so the plan covers it.
fn planned_unit<'a>(plan: &'a emitter::ComposePlan, context: &str) -> &'a emitter::ComposeUnit {
    plan.units
        .get(context)
        .expect("the compose plan has a unit for every context with a checked table")
}

/// The files a Worker's bindings stage beside its `wrangler.toml`: P4b (#2608)
/// each `D1` database's migrations, under the `migrations_dir` the config names,
/// and Hyperdrive H5 (#3322) which Hyperdrive bindings are MySQL and their TLS
/// modes, for `bynk deploy` (a Worker with no `MySql` instance gets no file).
pub fn stage_binding_files(
    bindings: &[emitter::bindings::WorkerBinding],
    layout: &WorkerLayout,
    context: &str,
    d1_migrations: &bynk_check::d1_migrations::D1Migrations,
    compiled: &mut Vec<StagedFile>,
) {
    // A D1 database is named from the Worker, as the migrations' check side
    // (`d1_migrations`, `d1_sql`) and `wrangler.toml`'s `d1_block` name it, so
    // all of them agree.
    let dashes = layout.dir(context);
    if let Some(manifest) = emitter::wrangler::emit_hyperdrive_manifest(bindings) {
        compiled.push(StagedFile {
            output_path: layout.worker_file(context, emitter::wrangler::HYPERDRIVE_MANIFEST),
            document: Document::Json(manifest),
            source_map: None,
            debug_metadata: None,
        });
    }
    // #3479: each `Vectorize` index's configuration, for `bynk deploy` to
    // create the index with and check it against.
    if let Some(manifest) = emitter::wrangler::emit_vectorize_manifest(bindings) {
        compiled.push(StagedFile {
            output_path: layout.worker_file(context, emitter::wrangler::VECTORIZE_MANIFEST),
            document: Document::Json(manifest),
            source_map: None,
            debug_metadata: None,
        });
    }
    // #3477: each container's capability, build directory and instance type,
    // for `bynk deploy` to plan from and to adopt an image against.
    if let Some(manifest) = emitter::wrangler::emit_containers_manifest(bindings) {
        compiled.push(StagedFile {
            output_path: layout.worker_file(context, emitter::wrangler::CONTAINERS_MANIFEST),
            document: Document::Json(manifest),
            source_map: None,
            debug_metadata: None,
        });
    }
    for binding in bindings {
        if !matches!(
            binding.source,
            emitter::bindings::BindingSource::D1Database { .. }
        ) {
            continue;
        }
        let database = emitter::wrangler::d1_database_name(&dashes, &binding.name);
        for file in bynk_check::d1_migrations::migrations_of(d1_migrations, &database, binding) {
            compiled.push(StagedFile {
                output_path: layout.worker_file(
                    context,
                    &format!(
                        "{}/{database}/{}",
                        emitter::wrangler::D1_MIGRATIONS_DIR,
                        file.name
                    ),
                ),
                document: Document::Sql(file.sql.clone()),
                source_map: None,
                debug_metadata: None,
            });
        }
    }
}

/// State-migrations S4 (#2259 Decision D): the context's versioned agent classes,
/// which its Worker's signed `/_bynk/migrate/` route can wake, with their lock
/// versions, sorted.
fn migratable_agents(
    ctx_name: &str,
    table: &UnitTable,
    agent_versions: &std::collections::HashMap<String, bynk_check::schema_registry::AgentVersions>,
) -> Vec<(String, i64)> {
    let mut migratable: Vec<(String, i64)> = agent_versions
        .iter()
        .filter_map(|(class, v)| {
            let (unit, agent) = class.rsplit_once('.')?;
            (unit == ctx_name && table.agents.contains_key(agent))
                .then(|| (agent.to_string(), v.version))
        })
        .collect();
    migratable.sort();
    migratable
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Names that need escaping: a quote, a backslash, the three short control
    /// escapes both writers use (`\t`, `\n`, `\r`), control characters with only a
    /// `\u00xx` form, and non-ASCII (which `serde_json` writes as it is). U+0008
    /// and U+000C are left out: `serde_json` writes them `\b` and `\f`, while
    /// `json_string` writes `\u0008` and `\u000c`, the same value spelt
    /// differently, so the byte-for-byte comparisons below would not hold.
    const AWKWARD: &[&str] = &[
        "q\"uote",
        "back\\slash",
        "tab\tnl\ncr\r",
        "bell\u{7}\u{1f}",
        "caf\u{e9} \u{4e2d}\u{1f600}",
    ];

    fn classes(names: &[&str]) -> Vec<String> {
        names.iter().map(|c| c.to_string()).collect()
    }

    /// What `serde_json::to_string_pretty` printed for the group listing before
    /// the layout was written by hand: sorted keys, two-space indent, `[]` for
    /// no classes, and every awkward name escaped the same way (the writer
    /// reuses `bynk_project::json_string`).
    #[test]
    fn the_group_inspect_listing_is_laid_out_as_pretty_json() {
        let (orders, pay, odd) = (
            classes(&["Counter", "Ledger"]),
            classes(&[]),
            classes(AWKWARD),
        );
        let odd_context = AWKWARD.join("/");
        let ours = render_inspect_members(&[
            ("shop.orders", &orders),
            ("shop.pay", &pay),
            (&odd_context, &odd),
        ]);
        let theirs = serde_json::to_string_pretty(&json!({ "members": [
            { "context": "shop.orders", "classes": orders },
            { "context": "shop.pay", "classes": pay },
            { "context": odd_context, "classes": odd },
        ] }))
        .unwrap()
            + "\n";
        assert_eq!(ours, theirs);
        assert!(ours.contains("\"classes\": [],"), "{ours}");
    }

    /// The agents manifest is what `serde_json` printed: contexts and classes in
    /// order, and every awkward name escaped.
    #[test]
    fn the_agents_manifest_is_laid_out_as_pretty_json() {
        let odd_context = AWKWARD.join("/");
        let agents = BTreeMap::from([
            ("shop.pay", classes(&["Counter", "Ledger"])),
            (odd_context.as_str(), classes(AWKWARD)),
            ("\u{e9}clair", classes(&["A"])),
        ]);
        let theirs = serde_json::to_string_pretty(&json!({ "agents": {
            "shop.pay": ["Counter", "Ledger"],
            odd_context.as_str(): AWKWARD,
            "\u{e9}clair": ["A"],
        } }))
        .unwrap()
            + "\n";
        assert_eq!(render_agents(&agents), theirs);
    }
}
