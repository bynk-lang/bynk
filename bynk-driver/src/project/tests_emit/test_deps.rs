//! Test dependency emission: `deps`, namespace destructuring, assets and fixtures.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
// #1759: every namespace a test module imports, from one definition.
use crate::dev_run::TEST_SESSION_KEY;
use bynk_check::test_suites::{self, ResolvedStub};
use bynk_emit::emitter::emit::test_scaffold_ns as test_ns;
use bynk_ts::{TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt, TsType};

/// The arguments a derived capability's test double is built with: P3 (#2342) a
/// rate limiter's limit and window, #3479 a `Vectorize` index's dimensions,
/// metric and `@indexed` entries, so the double enforces its declaration. None
/// for any other kind (and for the bare capability, whose double takes
/// defaults).
fn double_args(derived: Option<&bynk_check::symbols::DerivedInstance>) -> Vec<TsExpr> {
    if let Some((limit, period)) = derived.and_then(|d| d.rate_limit()) {
        return vec![num_lit(limit.to_string()), num_lit(period.to_string())];
    }
    match derived.filter(|d| d.is_vectorize()) {
        Some(d) => vec![
            num_lit(d.vectorize_dimensions().unwrap_or_default().to_string()),
            str_lit(d.vectorize_metric().unwrap_or_default()),
            TsExpr::Array {
                items: d.vectorize_indexed().iter().map(str_lit).collect(),
                multiline: false,
            },
        ],
        None => Vec::new(),
    }
}

/// #1479: returns the real [`TsStmt`] itself (was pre-printed `String`) — the
/// one real declaration this function ever built; its caller now prints it
/// directly via `bynk_ts::print_stmt`, the same shape it always used, just
/// one call further out.
///
/// #1863: a provider is built as the composition root builds it
/// ([`instantiate_provider_ts_expr`]), its own `given` capabilities wired in,
/// whether the context declares it or a context it consumes does. The units
/// that construction names are returned, for the test module to import: a
/// provider's `given` can reach a unit the target doesn't consume itself.
///
/// Unlike the composition root, a provider's `given` that is also one of the
/// deps' own entries is that entry, held in a local, not a second copy: in a
/// test a `stub` overlays the entry, and a copy would escape it (#1864
/// review).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_test_deps(
    target_name: &str,
    target_kind: UnitKind,
    stubs: &HashMap<String, ResolvedStub>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_flattened: &HashMap<String, HashMap<String, String>>,
    // #1975: the adapters whose binding this factory builds for real, by
    // binding module (an adapter suite, or the `integration` tier's factory);
    // `None` keeps every external provider the placeholder.
    bindings: Option<&HashMap<String, String>>,
    // The factory's name: `__makeTestDeps`, or `__makeIntegrationDeps`.
    name: &str,
    // #2050: whether the target's handlers emit, so their `deps` require
    // `__eventsDispatch` ([`target_uses_emit`]).
    uses_emit: bool,
) -> TestDeps {
    let mut units: BTreeSet<String> = BTreeSet::new();
    let mut doubles: Vec<(String, String, String)> = Vec::new();
    // #1975: a real binding's own `given` that the target does not reach
    // takes the tier's test double, recorded so the module defines it.
    let given_doubles: std::cell::RefCell<Vec<(String, String, String)>> =
        std::cell::RefCell::new(Vec::new());
    let given_double = |owner: &str, cap: &str| -> Option<TsExpr> {
        bindings?;
        platform_double(owner, cap, &test_ns(owner), cap, false, None)?;
        given_doubles
            .borrow_mut()
            .push((owner.to_string(), cap.to_string(), cap.to_string()));
        Some(call(ident(format!("__bynkTest_{cap}")), Vec::new()))
    };
    // Each top-level entry: its deps key, the `(unit, capability)` it is, and
    // how its value is built.
    let mut tops: Vec<(String, (String, String), TopDep)> = Vec::new();
    let mut surface_entries: Vec<(String, TsExpr)> = Vec::new();
    // #1975: an adapter target's deps are its own capabilities, built from
    // its binding: the binding is the unit under test.
    if matches!(target_kind, UnitKind::Context | UnitKind::Adapter)
        && let Some(table) = unit_tables.get(target_name)
    {
        // Sorted so `__makeTestDeps` field order is deterministic across the
        // capability map's hash iteration order.
        let mut caps: Vec<&String> = table.capabilities.keys().collect();
        caps.sort();
        for cap in caps {
            // v0.118: a capability with a `stub` override plugs its
            // `__Stub_<Cap>` stub; otherwise the declared provider (its real
            // implementation) is used, as an un-overridden seam.
            let how = if table.providers.contains_key(cap) {
                TopDep::Provider
            } else if let Some(inst) = table.derived.get(cap) {
                // S4a (#1966): a derived capability's instance has its base's
                // double, its own fresh one.
                platform_or_placeholder(&inst.base_unit, &inst.base_cap)
            } else {
                TopDep::Placeholder
            };
            tops.push((cap.clone(), (target_name.to_string(), cap.clone()), how));
        }
        // v0.17 (Locale capability track, slice 1, #844): a capability
        // flattened in via `consumes U { Cap }` (e.g. an adapter's `Locale`)
        // is never in `table.capabilities` above — that holds only
        // capabilities this unit declares itself. Its real implementation is
        // wired by production `compose()` from a platform binding the test
        // module never imports, so an un-stubbed one is the placeholder,
        // exactly like a locally-declared capability with no provider — or,
        // #291, a platform capability's deterministic test double.
        let flattened = unit_flattened.get(target_name).cloned().unwrap_or_default();
        let mut flat: Vec<(&String, &String)> = flattened.iter().collect();
        flat.sort_by_key(|(cap, _)| cap.as_str());
        for (cap, owner) in flat {
            // #1975: an adapter's capability whose binding this factory
            // builds is its real provider.
            let how = if bindings.is_some_and(|b| b.contains_key(owner)) {
                TopDep::Provider
            } else {
                platform_or_placeholder(owner, cap)
            };
            tops.push((cap.clone(), (owner.clone(), cap.clone()), how));
        }
        // #1863: a capability another context provides, named with that
        // context's prefix (`given Fees.Fees`), as the composition root wires
        // it (`handler_cross_caps`, which also lists the flattened ones
        // above): that context's provider, keyed by the capability's name,
        // or a first-party platform capability's test double.
        let consumed_all: Vec<String> = unit_consumes.get(target_name).cloned().unwrap_or_default();
        let aliases = unit_consumes_aliases
            .get(target_name)
            .cloned()
            .unwrap_or_default();
        for (cap, owner) in handler_cross_caps(table, &consumed_all, &aliases, &flattened) {
            if tops.iter().any(|(k, _, _)| *k == cap) {
                continue;
            }
            let how = match platform_or_placeholder(&owner, &cap) {
                TopDep::Placeholder => TopDep::Provider,
                double => double,
            };
            tops.push((cap.clone(), (owner, cap), how));
        }
        // Cross-context surface: consumed contexts run with their real surface
        // (v0.118 `stub` is capability-only — a consumed-context capability
        // flattened via `consumes U { Cap }` is folded in via `unit_flattened`
        // above). An `adapter` target (e.g. `consumes bynk { Locale }`) has no
        // `__makeSurface` at all — so it must not get a surface entry either
        // (Locale capability track, slice 1, #844), nor does a context with
        // no services, consumed only for a capability (#1863).
        let mut alias_for_target: HashMap<String, String> = HashMap::new();
        for (alias, q) in &aliases {
            alias_for_target.insert(q.clone(), alias.clone());
        }
        for q in consumed_all.iter().filter(|q| {
            unit_tables
                .get(*q)
                .is_some_and(|t| t.kind != Some(UnitKind::Adapter) && !t.services.is_empty())
        }) {
            let key = alias_for_target
                .get(q)
                .cloned()
                .unwrap_or_else(|| q.rsplit('.').next().unwrap_or(q.as_str()).to_string());
            let other_ns = test_ns(q);
            surface_entries.push((
                key,
                undefined_as_unknown_as(format!(
                    "globalThis.ReturnType<typeof {other_ns}.__makeSurface>"
                )),
            ));
        }
    }

    // #1864 review: build every top-level value with each top-level
    // capability named by a local, so a provider whose `given` reaches one
    // takes that local, stub and all, rather than a fresh copy. The locals a
    // construction used become `const`s ahead of the `return`, each after
    // the ones its own construction used; every other value stays inline.
    let prebuilt: HashMap<(String, String), String> = tops
        .iter()
        .map(|(key, id, _)| (id.clone(), format!("__dep_{key}")))
        .collect();
    let mut built: Vec<BuiltDep> = Vec::new();
    for (key, (unit, cap), how) in &tops {
        let used = std::cell::RefCell::new(BTreeSet::new());
        let ty = format!("{}.{cap}", test_ns(unit));
        let base = match how {
            TopDep::Provider => Some(instantiate_provider_ts_expr(
                unit,
                cap,
                unit_tables,
                unit_consumes,
                unit_consumes_aliases,
                unit_flattened,
                ProviderNamespaces::Test {
                    prebuilt: &prebuilt,
                    used: &used,
                    bindings,
                    double: &given_double,
                },
                None,
                None,
                &mut units,
            )),
            TopDep::Double => {
                // A derived capability's double is its base's.
                let derived = unit_tables.get(unit).and_then(|t| t.derived.get(cap));
                let (owner, base) = derived.map_or((unit, cap), |d| (&d.base_unit, &d.base_cap));
                doubles.push((owner.clone(), base.clone(), cap.clone()));
                let args = double_args(derived);
                Some(call(ident(format!("__bynkTest_{cap}")), args))
            }
            TopDep::Placeholder => None,
        };
        let value = match stubs.get(key) {
            Some(rp) => stub_overlay(rp, &ty, base),
            None => base.unwrap_or_else(|| undefined_as_unknown_as(ty)),
        };
        let id = (unit.clone(), cap.clone());
        built.push(BuiltDep {
            key: key.clone(),
            local: prebuilt[&id].clone(),
            id,
            value,
            used: used.into_inner(),
        });
    }
    let shared: BTreeSet<(String, String)> =
        built.iter().flat_map(|b| b.used.iter().cloned()).collect();
    let mut body: Vec<TsStmt> = Vec::new();
    let mut declared: BTreeSet<(String, String)> = BTreeSet::new();
    // Dependencies first: no provider chain is cyclic (the checker rejects
    // one), so this terminates.
    fn declare(
        id: &(String, String),
        built: &[BuiltDep],
        declared: &mut BTreeSet<(String, String)>,
        body: &mut Vec<TsStmt>,
    ) {
        if declared.contains(id) {
            return;
        }
        let Some(dep) = built.iter().find(|b| b.id == *id) else {
            return;
        };
        for used in &dep.used {
            declare(used, built, declared, body);
        }
        declared.insert(id.clone());
        body.push(TsStmt::const_stmt(
            TsBindingName::Ident(dep.local.clone()),
            None,
            dep.value.clone(),
            None,
        ));
    }
    for dep in &built {
        if shared.contains(&dep.id) {
            declare(&dep.id, &built, &mut declared, &mut body);
        }
    }
    let mut entries: Vec<(String, TsExpr)> = built
        .into_iter()
        .map(|dep| {
            let value = if shared.contains(&dep.id) {
                ident(dep.local)
            } else {
                dep.value
            };
            (dep.key, value)
        })
        .collect();
    if !surface_entries.is_empty() {
        entries.push(("surface".to_string(), TsExpr::object(surface_entries)));
    }
    // #1994/#2683: the execution context a `~>`-sending handler hands its promise to,
    // plus, on a virtual-time module, the harness's timers (`within` reads them).
    let virtual_time = module_is_virtual_time(target_name, target_kind, stubs, unit_tables);
    if target_uses_send(target_name, target_kind, unit_tables) {
        let exec = call(ident("__bynkExec"), Vec::new());
        entries.push((
            "__exec".to_string(),
            if virtual_time {
                call(
                    ident("globalThis.Object.assign"),
                    vec![
                        exec,
                        TsExpr::object(vec![("timers".to_string(), ident("__bynkTimers"))]),
                    ],
                )
            } else {
                exec
            },
        ));
    }
    entries.extend(session_keys_entry(target_name, target_kind, unit_tables));
    let mut entries: Vec<TsObjectEntry> = entries
        .into_iter()
        .map(|(k, v)| TsObjectEntry::Prop(k, v))
        .collect();
    // #2050: the release-at-commit flush of an emitting handler — a service's,
    // or an agent's, which runs on its caller's `deps` — calls
    // `deps.__eventsDispatch`. In a test it hands each event to the `Events`
    // double, which records the call and delivers nothing, so `expect
    // Events.emit called …` observes what the handler released. A method
    // reading `this.Events` rather than a captured value: the recording
    // wrapper lands on the deps object after this factory runs.
    if uses_emit {
        entries.push(TsObjectEntry::Method {
            name: "__eventsDispatch".to_string(),
            is_async: true,
            generics: Vec::new(),
            params: vec![TsParam {
                name: "events".to_string(),
                ty: Some(TsType::named_with_args(
                    "globalThis.Array",
                    vec![TsType::named(bynk_emit::emitter::EVENTS_WIRE_EVENT_TS_TYPE)],
                )),
                optional: false,
            }],
            return_type: None,
            doc: None,
            inline: true,
            body: vec![TsStmt::raw(
                "for (const ev of events) await this.Events.emit(ev.payload);",
                None,
            )],
        });
    }
    // The same `"{  }"` double-space quirk `workers.rs`/`project.rs`/
    // `emit.rs`/`gen_ts_for_ty` (this file, Arc C slice 31) already carry as
    // opaque text: the pre-conversion `format!("  return {{ {} }};\n",
    // entries.join(", "))` template always has a space on each side of its
    // `{}` slot, so zero entries (a non-`Context` target, or a `Context`
    // with no capabilities/flattened caps/consumed surface) literally
    // produced a double space, not the tight `"{}"` `TsExpr::object`'s own
    // empty-entries shortcut renders — a real, reachable shape (an
    // integration target's own non-`Context` participants all hit it).
    let case_scoped = stubs.values().any(stub_is_scoped);
    let suite_scoped = stubs.values().any(stub_is_suite_scoped);
    let return_value = if entries.is_empty() {
        ident("{  }")
    } else {
        TsExpr::object_entries(entries)
    };
    if virtual_time {
        // #2683: each case builds its `deps` afresh, so this is where the
        // virtual clock, its timers and the in-flight count start over, and
        // where each capability call comes to be counted.
        body.push(TsStmt::const_stmt(
            TsBindingName::Ident("__deps".to_string()),
            None,
            return_value,
            None,
        ));
        body.push(TsStmt::expr_stmt(
            call(ident("__bynkTrackDeps"), vec![ident("__deps")]),
            None,
        ));
        body.push(TsStmt::return_stmt(Some(ident("__deps")), None));
    } else {
        body.push(TsStmt::return_stmt(Some(return_value), None));
    }
    doubles.extend(given_doubles.into_inner());
    let stmt = TsStmt::decl(
        TsDecl::Function {
            name: name.to_string(),
            generics: Vec::new(),
            // #291: the running case's name, when some `stub` is case-scoped,
            // and #1860 the running suite's ordinal, when one is suite-scoped.
            params: [
                ("__case", "string", case_scoped),
                ("__suite", "number", suite_scoped),
            ]
            .into_iter()
            .filter(|(_, _, on)| *on)
            .map(|(name, ty, _)| TsParam {
                name: name.to_string(),
                ty: Some(TsType::named(ty)),
                optional: true,
            })
            .collect(),
            return_type: None,
            body,
            is_async: false,
            inline: false,
        },
        None,
    );
    TestDeps {
        stmt,
        units,
        doubles,
    }
}

/// What [`emit_test_deps`] built (#1863).
pub(super) struct TestDeps {
    /// The `__makeTestDeps` declaration.
    pub(super) stmt: TsStmt,
    /// The units its providers name, which the test module imports.
    pub(super) units: BTreeSet<String>,
    /// Each `(owner, capability, name)` whose `__bynkTest_<name>` test double
    /// it calls, which the test module defines (`platform_double`): `name` is
    /// the capability, or a derived capability over it (S4a, #1966).
    pub(super) doubles: Vec<(String, String, String)>,
}

/// One top-level test dep, built ([`emit_test_deps`]).
pub(super) struct BuiltDep {
    /// Its deps key: the capability's name.
    pub(super) key: String,
    /// The `(unit, capability)` it is.
    pub(super) id: (String, String),
    /// The local that holds it, when another dep is built over it.
    pub(super) local: String,
    /// Its value, `stub` overlay included.
    pub(super) value: TsExpr,
    /// The other deps' locals its construction names.
    pub(super) used: BTreeSet<(String, String)>,
}

/// How a top-level test dep's value is built, before any `stub` overlay.
pub(super) enum TopDep {
    /// Its provider, through [`instantiate_provider_ts_expr`].
    Provider,
    /// A platform capability's deterministic test double (#291).
    Double,
    /// The `undefined` placeholder: no provider the test module can build.
    Placeholder,
}

/// A capability of `owner` with a platform test double is [`TopDep::Double`];
/// any other, the placeholder, whatever provides it in production.
pub(super) fn platform_or_placeholder(owner: &str, cap: &str) -> TopDep {
    if platform_double(owner, cap, &test_ns(owner), cap, false, None).is_some() {
        TopDep::Double
    } else {
        TopDep::Placeholder
    }
}

/// #18 (testing-track infra): a real value destructure plus a per-type alias,
/// instead of one `const { … } = ns as any`. Every bynk-declared type pairs an
/// `export type`/`export interface` with a companion runtime value
/// (`export const X = {...}`, a refinement's `of`/`unsafe`, a sum's variant
/// constructors, or simply `{}` for a plain record) — but destructuring does
/// not carry the *type* side of that merge into the new local binding, so
/// `const { X } = ns` alone leaves `X` usable only as a value. The blanket
/// `as any` papered over that by disabling type-checking for every
/// destructured name, not just the ones needing the workaround — which is
/// exactly why this generator's own output couldn't be gated by
/// `tsc --strict`: nothing inside a generated test body was ever really
/// checked. `type_names` (a subset of `value_names`) gets the alias;
/// `value_names` is unconditionally destructured with no cast.
/// #1479: returns real [`TsStmt`]s (was `out: &mut String`) — every caller
/// now appends via `bynk_emit::emitter::extend_printed_at(out, stmts, 2)`, the
/// same depth-2 this scaffold body's own statements always printed at.
/// #1703: each type name carries its type parameters, so a generic type's
/// alias is generic too (`type Box<A> = ns.Box<A>;`). A bare alias of a generic
/// type is `TS2314` under `tsc`, which failed every suite whose target declared
/// or `uses` one, whether or not a case mentioned it. Build the list with
/// [`aliased_types`].
pub(super) fn emit_ns_destructure(
    ns: &str,
    value_names: &[String],
    type_names: &[(String, Vec<String>)],
) -> Vec<TsStmt> {
    emit_ns_destructure_renamed(
        ns,
        value_names,
        type_names,
        &HashMap::new(),
        &HashSet::new(),
    )
}

/// [`emit_ns_destructure`] where `renames` (original → alias, #2887) binds a
/// consumed type the unit renames with `consumes U { T as Alias }` under its
/// alias: `const { T: Alias } = ns;` and `type Alias = ns.T`.
/// `declared` holds the renamed names also bound under their declared name
/// (#2890): a suite is not a consuming context, so a case names the type by it.
pub(super) fn emit_ns_destructure_renamed(
    ns: &str,
    value_names: &[String],
    type_names: &[(String, Vec<String>)],
    renames: &HashMap<String, String>,
    declared: &HashSet<String>,
) -> Vec<TsStmt> {
    let mut stmts = Vec::new();
    if !value_names.is_empty() {
        let pattern = value_names
            .iter()
            .flat_map(|n| match renames.get(n) {
                Some(alias) if declared.contains(n) => vec![format!("{n}: {alias}"), n.clone()],
                Some(alias) => vec![format!("{n}: {alias}")],
                None => vec![n.clone()],
            })
            .collect();
        stmts.push(TsStmt::const_stmt(
            TsBindingName::ObjectPattern(pattern),
            None,
            ident(ns),
            None,
        ));
    }
    for (t, params) in type_names {
        let target = format!("{ns}.{t}");
        let ty = if params.is_empty() {
            TsType::named(target)
        } else {
            TsType::named_with_args(target, params.iter().map(TsType::named).collect())
        };
        let mut names = vec![renames.get(t).unwrap_or(t).clone()];
        if declared.contains(t) {
            names.push(t.clone());
        }
        for name in names {
            stmts.push(TsStmt::decl(
                TsDecl::TypeAlias {
                    name,
                    type_params: params.clone(),
                    ty: ty.clone(),
                },
                None,
            ));
        }
    }
    stmts
}

/// #2618: the names of `table`'s types not marked `private`.
pub(super) fn public_types(table: &UnitTable) -> impl Iterator<Item = &String> {
    table
        .types
        .iter()
        .filter(|(_, d)| d.private.is_none())
        .map(|(n, _)| n)
}

/// #2618: the names of `table`'s functions not marked `private`.
pub(super) fn public_fns(table: &UnitTable) -> impl Iterator<Item = &String> {
    table
        .fns
        .iter()
        .filter(|(_, d)| d.private.is_none())
        .map(|(n, _)| n)
}

/// #2618: whether `name` is a type of `table` not marked `private`.
pub(super) fn is_public_type(table: &UnitTable, name: &str) -> bool {
    table.types.get(name).is_some_and(|d| d.private.is_none())
}

/// #1703: the types of `table` that `keep` admits, each with its type
/// parameters, sorted by name: the `type_names` [`emit_ns_destructure`] aliases.
pub(super) fn aliased_types(
    table: &UnitTable,
    keep: impl Fn(&String) -> bool,
) -> Vec<(String, Vec<String>)> {
    let mut types: Vec<(String, Vec<String>)> = table
        .types
        .iter()
        .filter(|(name, _)| keep(name))
        .map(|(name, decl)| {
            let params = decl
                .type_params
                .iter()
                .map(|p| p.name.name.clone())
                .collect();
            (name.clone(), params)
        })
        .collect();
    types.sort();
    types
}

/// The target's own names, `(local, values)`, that hide a `uses` commons' name
/// in a test scope.
pub(super) fn target_names<'a>(
    target_name: &str,
    unit_tables: &'a HashMap<String, UnitTable>,
) -> (HashSet<&'a String>, HashSet<&'a String>) {
    // message-bundles slice 1 (#859): a name the target itself already
    // declares must be excluded here — the same local-shadows-`uses`
    // precedence `compose_unit_symbols` already applies to production
    // emission (project.rs's `combined_fns`/`combined_types`), which this
    // test-scaffold path builds independently and had never applied. Never
    // exercised before this slice: no prior commons declared a name also
    // present in something it `uses`. Without the filter, a target with its
    // own `render` (a messages block's synthetic one) *and* `uses
    // bynk.locale` (which also declares `render`) destructured both into one
    // scope — `Cannot redeclare block-scoped variable 'render'` under `tsc`.
    let target_local: HashSet<&String> = unit_tables
        .get(target_name)
        .map(|t| {
            t.types
                .keys()
                .chain(t.fns.keys())
                .chain(t.assets.keys())
                .chain(t.embeds.keys())
                .chain(t.consts.keys())
                .collect()
        })
        .unwrap_or_default();
    // #1925: a used commons' function or asset is hidden by any value the
    // target declares — its services, agents and bodied providers too — as
    // the checker's view hides it (`bynk_check::project_model::shadowed_imports`).
    // #1927: so is a used commons' type's *value* (its constructors), which a
    // case may not name (`check_shadowed_type_names_in_suite`). Its type
    // alias (`type X = ns.X`, below) is still bound: it lives in TypeScript's
    // type space, beside the target value's `const`, and the lowering names
    // the types of checked expressions (`(__x: X) => …`, #1778).
    let target_values: HashSet<&String> = unit_tables
        .get(target_name)
        .map(|t| {
            target_local
                .iter()
                .copied()
                .chain(t.services.keys())
                .chain(t.agents.keys())
                .chain(
                    t.providers
                        .values()
                        .filter(|p| !p.external)
                        .map(|p| &p.provider_name.name),
                )
                .collect()
        })
        .unwrap_or_default();
    (target_local, target_values)
}

/// One commons' destructure: its namespace, the value names it binds, and the
/// type aliases (each with its type parameters).
pub(super) type NsDestructure = (String, Vec<String>, Vec<(String, Vec<String>)>);

/// The `uses` commons' names a test scope binds, as `(namespace, value names,
/// type aliases)` per commons: what a case runner destructures, and (#2947) a
/// stub method that inlines suite helpers. One computation, so the two scopes
/// hide and admit the same names.
///
/// `seen_assets` carries the assets already bound across calls: #1883, slice 3,
/// a name brought in by an earlier used commons is not destructured again (two
/// used commons may not share an asset name, `bynk.uses.name_conflict`, but an
/// asset may share none with a fn either).
pub(super) fn used_commons_destructures<'a>(
    target_name: &str,
    unit_tables: &'a HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    seen_assets: &mut HashSet<&'a String>,
) -> Vec<NsDestructure> {
    let (target_local, target_values) = target_names(target_name, unit_tables);
    let mut out = Vec::new();
    if let Some(used) = unit_uses.get(target_name) {
        for u in used {
            let ns = test_ns(u);
            if let Some(table) = unit_tables.get(u) {
                // #2618: a used commons' `private` declarations stay in it, as
                // the checker's suite view leaves them out; destructuring one
                // would redeclare a public name another used commons binds.
                let assets = table
                    .assets
                    .iter()
                    .filter(|(_, d)| d.private.is_none())
                    .map(|(n, _)| n)
                    .filter(|n| seen_assets.insert(*n));
                let embeds = table
                    .embeds
                    .iter()
                    .filter(|(_, d)| d.private.is_none())
                    .map(|(n, _)| n);
                let consts = table
                    .consts
                    .iter()
                    .filter(|(_, d)| d.private.is_none())
                    .map(|(n, _)| n);
                let mut names: Vec<String> = public_types(table)
                    .chain(public_fns(table))
                    .chain(assets)
                    .chain(embeds)
                    .chain(consts)
                    .filter(|n| !target_values.contains(n))
                    .cloned()
                    .collect();
                names.sort();
                names.dedup();
                let type_names = aliased_types(table, |n| {
                    !target_local.contains(n) && is_public_type(table, n)
                });
                out.push((ns, names, type_names));
            }
        }
    }
    out
}

/// #1814: a suite view's hidden types grouped by owning commons, each group
/// sorted by name.
pub(super) fn hidden_by_owner(hidden: &BTreeMap<String, String>) -> BTreeMap<String, Vec<String>> {
    let mut by_owner: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, owner) in hidden {
        by_owner
            .entry(owner.clone())
            .or_default()
            .push(name.clone());
    }
    by_owner
}

/// #1814: destructure each hidden type from its owning commons' namespace,
/// as a value and a type alias, the way a `uses` commons' types are: a
/// lowered case spells a reached type (a kernel's `(__x: Repo) => …`), names
/// a reached sum's variants (`Status.Done`) and calls its methods
/// (`Repo.shout(…)`).
pub(super) fn extend_hidden_destructures(
    out: &mut String,
    hidden: &BTreeMap<String, Vec<String>>,
    unit_tables: &HashMap<String, UnitTable>,
) {
    for (owner, names) in hidden {
        let Some(table) = unit_tables.get(owner) else {
            continue;
        };
        let type_names = aliased_types(table, |n| names.contains(n));
        bynk_emit::emitter::extend_printed_at(
            out,
            emit_ns_destructure(&test_ns(owner), names, &type_names),
            2,
        );
    }
}

pub(super) fn sanitise_case_name(name: &str, index: &mut usize) -> String {
    let mut s = String::from("test_");
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            s.push(ch);
        } else {
            s.push('_');
        }
    }
    if s == "test_" {
        s.push_str(&index.to_string());
    }
    *index += 1;
    s
}

/// #1883, slice 3: the project's declared assets, for the test passes — each
/// declaration's served path ([`bynk_check::assets::declared_served_paths`])
/// and each declared file's bytes. Empty when no `assets/` folder was read.
pub(crate) struct TestAssetFiles<'a> {
    pub(crate) served: &'a BTreeMap<String, String>,
    pub(crate) bytes: &'a BTreeMap<String, Vec<u8>>,
    /// #2841: the text of each declared suite `fixture` file, by normalised path.
    pub(crate) fixtures: &'a BTreeMap<String, String>,
}

/// #2134/#2956: a case's `env` entries as the object `__bynkEnv` is set to: a
/// literal as itself, and a fixture as its module-level constant
/// ([`fixture_ident`]), so its text is in the module once.
pub(super) fn env_entries(
    env: &BTreeMap<String, bynk_check::test_env::EnvValue>,
) -> Vec<(String, TsExpr)> {
    env.iter()
        .map(|(k, v)| {
            let value = match v {
                bynk_check::test_env::EnvValue::Literal(s) => str_lit(s.clone()),
                bynk_check::test_env::EnvValue::Fixture(path) => ident(fixture_ident(path)),
            };
            (k.clone(), value)
        })
        .collect()
}

/// #2841: the TypeScript identifier of the module-level constant holding a
/// fixture file's text — a function of the normalised path alone, so suites that
/// name one file share one constant and a runner can name it without state.
pub(super) fn fixture_ident(path: &str) -> String {
    let key = crate::project::assets::normalise_asset_path(path);
    let mut out = String::from("__fixture_");
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b as char);
        } else {
            out.push_str(&format!("_{b:02x}"));
        }
    }
    out
}

/// #2841: one module-level `const <ident>: string = "<text>"` for each distinct
/// fixture file the suites in `indices` declare, in path order — the text is in
/// the test module once, however many suites and runners read it, and never
/// anywhere else (a deployable is built without suites). `None` when no suite
/// here declares a fixture, so a module without one is unchanged.
pub(super) fn fixture_consts(
    indices: &[usize],
    parsed: &[ParsedFile],
    files: &TestAssetFiles,
) -> Option<String> {
    let mut paths: BTreeSet<String> = BTreeSet::new();
    for &i in indices {
        if let bynk_syntax::ast::SourceUnit::Suite(s) = parsed[i].unit() {
            for f in &s.fixtures {
                paths.insert(crate::project::assets::normalise_asset_path(&f.path));
            }
        }
    }
    let stmts: Vec<TsStmt> = paths
        .iter()
        .filter_map(|p| {
            let text = files.fixtures.get(p)?;
            Some(TsStmt::const_stmt(
                TsBindingName::Ident(fixture_ident(p)),
                Some(TsType::named("string")),
                TsExpr::Lit(TsLit::Str(text.clone())),
                None,
            ))
        })
        .collect();
    if stmts.is_empty() {
        return None;
    }
    Some(stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect())
}

/// #1883, slice 3 ([DECISION F]): the TypeScript that lets a case fetch an
/// asset — the files `target`'s Worker ships
/// ([`bynk_check::assets::deployed_assets`]), embedded base64 by served path
/// with the platform's content type, and `__bynkAssetGet(url)`, which answers
/// as the platform's asset tier does: `HttpResult.Raw(bytes, contentType)`
/// for a shipped file, `NotFound` otherwise. Embedding keeps a suite free of
/// disk access at test time; it is harness data, not the compile-time
/// `String` embedding [DECISION E] excludes. `None` when the Worker ships no
/// files, so a module without assets is unchanged.
pub(super) fn asset_harness(
    target: &str,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_tables: &HashMap<String, UnitTable>,
    files: &TestAssetFiles,
) -> Option<String> {
    let deployed = bynk_check::assets::deployed_assets(
        target,
        unit_uses,
        unit_consumes,
        unit_tables,
        files.served,
    );
    if deployed.is_empty() {
        return None;
    }
    let mut out = String::from(
        "const __bynkAssetFiles: Record<string, { readonly body: string; readonly contentType: string }> = {\n",
    );
    for (served, source) in &deployed {
        // Review of #1922: every deployed file was read (`asset_served`
        // comes from the same bytes), so a miss is a broken invariant, not
        // something to degrade quietly into an undefined `__bynkAssetGet`.
        let bytes = files.bytes.get(source).unwrap_or_else(|| {
            panic!(
                "bynk internal error: deployed asset `{source}` has no bytes for the test harness"
            )
        });
        out.push_str(&format!(
            "  {}: {{ body: \"{}\", contentType: {} }},\n",
            bynk_project::json_string(served),
            base64(bytes),
            bynk_project::json_string(bynk_check::assets::content_type_for(served)),
        ));
    }
    out.push_str(
        "};\n\
         function __bynkAssetGet(url: string): HttpResult<never> {\n\
         \x20 const f = globalThis.Object.hasOwn(__bynkAssetFiles, url) ? __bynkAssetFiles[url] : undefined;\n\
         \x20 if (f === undefined) return HttpResult.NotFound;\n\
         \x20 const bin = atob(f.body);\n\
         \x20 const bytes = new Uint8Array(bin.length);\n\
         \x20 for (let i = 0; i < bin.length; i++) bytes[i] = bin.charCodeAt(i);\n\
         \x20 return HttpResult.Raw(bytes, f.contentType);\n\
         }\n",
    );
    Some(out)
}

/// #1883, slice 3: each asset a `system` case may name ([`SuiteView::assets`]),
/// grouped by the unit that declares it — a participant first (whose
/// `handlers.js` exports its own assets), then the commons that declare the
/// rest — sorted.
pub(super) fn system_asset_owners(
    view: &test_suites::SuiteView,
    participants: &[String],
    unit_tables: &HashMap<String, UnitTable>,
) -> BTreeMap<String, Vec<String>> {
    let mut owners: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (name, decl) in &view.assets {
        let owner = unit_tables
            .iter()
            .filter(|(_, t)| {
                t.assets
                    .get(name)
                    .is_some_and(|d| d.path == decl.path && d.at == decl.at)
            })
            .map(|(u, _)| u)
            .min_by_key(|u| (!participants.contains(u), (*u).clone()));
        if let Some(owner) = owner {
            owners.entry(owner.clone()).or_default().push(name.clone());
        }
    }
    for names in owners.values_mut() {
        names.sort();
    }
    owners
}

/// Standard base64 (RFC 4648, with padding) of `bytes`, for
/// [`asset_harness`]'s embedded files.
pub(super) fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18u32, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(ALPHABET[((n >> shift) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// `deps.__session`, for a handler's `Actor.issue(identity)`: every `SignedCookie` actor of the target
/// signs with the fixed `TEST_SESSION_KEY`, so an issued cookie is the same on every run. The `system`
/// tier signs with the key it sets on the deployed Worker instead.
fn session_keys_entry(
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
) -> Option<(String, TsExpr)> {
    if target_kind != UnitKind::Context {
        return None;
    }
    let mut actors: Vec<String> = unit_tables
        .get(target_name)?
        .actors
        .values()
        .filter_map(|a| bynk_check::actors::signed_cookie_seam_of(a, None))
        .map(|s| s.actor)
        .collect();
    if actors.is_empty() {
        return None;
    }
    actors.sort();
    let keys = actors
        .into_iter()
        .map(|a| {
            (
                a,
                TsExpr::object(vec![("secret".to_string(), str_lit(TEST_SESSION_KEY))]),
            )
        })
        .collect();
    Some(("__session".to_string(), TsExpr::object(keys)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitise_case_name_prefixes_and_advances_index() {
        let mut idx = 0;
        assert_eq!(
            sanitise_case_name("hello world", &mut idx),
            "test_hello_world"
        );
        assert_eq!(idx, 1); // index advances on every call
        assert_eq!(sanitise_case_name("a-b.c", &mut idx), "test_a_b_c");
        assert_eq!(idx, 2);
    }

    #[test]
    fn sanitise_case_name_appends_index_only_for_empty_name() {
        let mut idx = 5;
        assert_eq!(sanitise_case_name("", &mut idx), "test_5"); // empty -> index suffix
        assert_eq!(idx, 6);
        // A name of only non-alphanumeric chars is NOT "test_", so no suffix.
        let mut idx2 = 9;
        assert_eq!(sanitise_case_name(" ", &mut idx2), "test__");
        assert_eq!(idx2, 10);
    }

    // -- coerce_int_field (#1398, review of #1428) ---------------------------
}
