//! Test scope setup and the per-case function.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use crate::emitter::RuntimeUse;
// #1759: every namespace a test module imports, from one definition.
use crate::emitter::emit::test_scaffold_ns as test_ns;
use bynk_check::checker::Types;
use bynk_check::test_suites::{self, ResolvedStub, StubScope};
use bynk_syntax::ast::{Case, FnName};
use bynk_ts::{TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsStmt, TsType, TsTypeMember};
use std::sync::Arc;

/// #3892: `(name, [service, …])` for each consumed context that declares
/// services, named by its alias or its last path segment, the key its surface
/// double sits under in the test `deps`.
fn consumed_service_seams(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
) -> Vec<(String, TsExpr)> {
    let aliases = unit_consumes_aliases.get(target_name);
    let mut out: Vec<(String, TsExpr)> = Vec::new();
    for q in unit_consumes.get(target_name).into_iter().flatten() {
        let Some(table) = unit_tables.get(q) else {
            continue;
        };
        if table.kind == Some(UnitKind::Adapter) || table.services.is_empty() {
            continue;
        }
        let key = aliases
            .and_then(|a| a.iter().find(|(_, v)| *v == q).map(|(k, _)| k.clone()))
            .unwrap_or_else(|| q.rsplit('.').next().unwrap_or(q.as_str()).to_string());
        let mut services: Vec<&String> = table.services.keys().collect();
        services.sort();
        out.push((
            key,
            TsExpr::array(services.into_iter().map(str_lit).collect()),
        ));
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Emit the shared per-runner scope setup — agent reset, the `deps` factory, and
/// the destructurings (see [`emit_ns_destructure`]) that bring the target's,
/// `uses`', and consumed contexts' names into scope. Shared by `case` and
/// `property` runners so a property body resolves names exactly as a case does.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_test_scope_setup(
    out: &mut Vec<TsStmt>,
    // #1975: the deps factory the case calls (`__makeTestDeps`, or the
    // `integration` tier's `__makeIntegrationDeps`).
    deps_factory: &str,
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    // v0.117: when the body observes (`Cap.op called …` / `trace(Cap.op)`), wrap
    // `deps` with the recording proxy and declare the per-case trace `__obs`. Off
    // for bodies that don't observe, so their emitted output is unchanged.
    record_calls: bool,
    // #291: the running case's name, passed to `__makeTestDeps` when some
    // `stub` clause is scoped to the case (so it applies to it and no other).
    case_name: Option<&str>,
    // #1860: the running suite's ordinal, passed when some `stub` clause is
    // scoped to a suite.
    suite: Option<usize>,
    hidden: &BTreeMap<String, String>,
    // #3536: the declaring suite's own `type`s. One named like a first-party
    // type the target's braces leave out replaces it in the case's scope.
    suite_types: &[bynk_syntax::ast::TypeDecl],
) -> HashSet<String> {
    // #2024: every name this setup binds in the runner's scope, returned so the
    // body's lowering renames a case `let` that would redeclare one.
    let mut bound: HashSet<String> = ["deps", "__obs"].map(String::from).into();
    let deps_args: Vec<TsExpr> = match suite {
        Some(n) => vec![
            case_name.map_or_else(absent, str_lit),
            TsExpr::Lit(TsLit::Num(n.to_string())),
        ],
        None => case_name.map(str_lit).into_iter().collect(),
    };
    let target_ns = test_ns(target_name);
    // v0.9.2: reset the target context's agent registries so each test sees a
    // fresh per-key state (finding #10's "fresh per test" half).
    let target_has_agents = unit_tables
        .get(target_name)
        .is_some_and(|t| !t.agents.is_empty());
    if target_has_agents {
        let stmt = TsStmt::expr_stmt(
            method_call(ident(target_ns.clone()), "__resetAgents", Vec::new()),
            None,
        );
        out.push(stmt);
    }
    // v0.117: the per-case recorded-call trace, and — for a context target with
    // capabilities — a `deps` wrapped so each capability operation records its
    // calls into `__obs`. Observations and `trace(Cap.op)` in the body read it.
    let obs_spec: Option<TsExpr> = if record_calls && target_kind == UnitKind::Context {
        unit_tables.get(target_name).and_then(|table| {
            // #291: the seams are the target's own capabilities and the platform
            // capabilities it flattens in (`consumes bynk { Logger }`); its own
            // wins on a name clash.
            let mut seams: std::collections::BTreeMap<&String, &bynk_syntax::ast::CapabilityDecl> =
                platform_seams(target_name, unit_tables)
                    .into_iter()
                    .map(|(cap, decl, _)| (cap, decl))
                    .collect();
            seams.extend(table.capabilities.iter());
            // #3892: a consumed context's services are seams too, by the name
            // the target reaches the context under.
            let service_seams = consumed_service_seams(
                target_name,
                unit_tables,
                unit_consumes,
                unit_consumes_aliases,
            );
            if seams.is_empty() && service_seams.is_empty() {
                return None;
            }
            let mut entries: Vec<(String, TsExpr)> = seams
                .iter()
                .map(|(c, decl)| {
                    let mut ops: Vec<&String> = decl.ops.iter().map(|o| &o.name.name).collect();
                    ops.sort();
                    (
                        (*c).clone(),
                        TsExpr::array(ops.into_iter().map(str_lit).collect()),
                    )
                })
                .collect();
            entries.extend(service_seams);
            Some(TsExpr::object(entries))
        })
    } else {
        None
    };
    if matches!(target_kind, UnitKind::Context | UnitKind::Adapter) {
        if let Some(spec) = obs_spec {
            // P7.2: matches `emitter/lower.rs`'s own `{ args: unknown[] }` reads
            // of this exact shape (`Trace`/`Called`-with-predicate lowering).
            let obs_ty = TsType::named_with_args(
                "globalThis.Record",
                vec![
                    TsType::named("string"),
                    TsType::array(TsType::Object(vec![
                        TsTypeMember::Prop {
                            name: "args".to_string(),
                            ty: TsType::array(TsType::named("unknown")),
                            optional: false,
                            readonly: false,
                        },
                        TsTypeMember::Prop {
                            name: "order".to_string(),
                            ty: TsType::named("number"),
                            optional: false,
                            readonly: false,
                        },
                    ])),
                ],
            );
            let obs_stmt = TsStmt::const_stmt(
                TsBindingName::Ident("__obs".to_string()),
                None,
                TsExpr::object(vec![
                    (
                        "log".to_string(),
                        TsExpr::As {
                            expr: Box::new(TsExpr::object(vec![])),
                            ty: obs_ty,
                        },
                    ),
                    ("n".to_string(), num_lit("0")),
                ]),
                None,
            );
            out.push(obs_stmt);
            let deps_stmt = TsStmt::const_stmt(
                TsBindingName::Ident("deps".to_string()),
                None,
                call(
                    ident("__bynkRecordDeps"),
                    vec![
                        call(ident(deps_factory), deps_args.clone()),
                        spec,
                        ident("__obs"),
                    ],
                ),
                None,
            );
            out.push(deps_stmt);
        } else {
            let deps_stmt = TsStmt::const_stmt(
                TsBindingName::Ident("deps".to_string()),
                None,
                call(ident(deps_factory), deps_args.clone()),
                None,
            );
            out.push(deps_stmt);
        }
    } else {
        let deps_stmt = TsStmt::const_stmt(
            TsBindingName::Ident("deps".to_string()),
            None,
            TsExpr::object(vec![]),
            None,
        );
        out.push(deps_stmt);
    }
    // Bring the target's top-level names into local scope so the lowered
    // body can reference them unqualified. The target's types and fns are
    // exported from its namespace by the production emitter.
    if let Some(table) = unit_tables.get(target_name) {
        let names = target_scope_names(table);
        let type_names = aliased_types(table, |_| true);
        bound.extend(names.iter().cloned());
        out.extend(emit_ns_destructure(&target_ns, &names, &type_names));
    }
    // Bring in `uses` commons names too — the target's body can use them.
    let (_, target_values) = target_names(target_name, unit_tables);
    let mut seen_assets: HashSet<&String> = HashSet::new();
    for (ns, names, type_names) in
        used_commons_destructures(target_name, unit_tables, unit_uses, &mut seen_assets)
    {
        bound.extend(names.iter().cloned());
        out.extend(emit_ns_destructure(&ns, &names, &type_names));
    }
    // #1814: the types the target's view reaches but does not import, from
    // their owning commons (imported by `emit_test_module`).
    out.extend(hidden_destructures(&hidden_by_owner(hidden), unit_tables));
    bound.extend(hidden.keys().cloned());
    // Bring consumed-context exported names into scope, plus a `Payment`
    // alias for the consumed surface (so `Payment.authorise.call(...)` works).
    if let Some(consumed) = unit_consumes.get(target_name) {
        let aliases = unit_consumes_aliases
            .get(target_name)
            .cloned()
            .unwrap_or_default();
        let mut alias_for: HashMap<String, String> = HashMap::new();
        for (alias, q) in &aliases {
            alias_for.insert(q.clone(), alias.clone());
        }
        for q in consumed {
            let ns = test_ns(q);
            let is_adapter = matches!(
                unit_tables.get(q).and_then(|t| t.kind),
                Some(UnitKind::Adapter)
            );
            if let Some(table) = unit_tables.get(q) {
                // #2702: not a consumed context's `private` types.
                // #3536: a suite type of the name of a first-party type the
                // target's braces leave out is the name's meaning here.
                let types = aliased_types(table, |n| {
                    is_public_type(table, n)
                        && !suite_replaces_left_out(unit_tables, target_name, suite_types, n)
                });
                let names: Vec<String> = types.iter().map(|(n, _)| n.clone()).collect();
                // #2887: the types this target renames from `q` bind as their aliases.
                let renames: HashMap<String, String> = unit_tables
                    .get(target_name)
                    .into_iter()
                    .flat_map(|t| &t.consumed_type_aliases)
                    .filter(|(_, a)| a.target == *q)
                    .map(|(alias, a)| (a.original.clone(), alias.clone()))
                    .collect();
                bound.extend(names.iter().map(|n| renames.get(n).unwrap_or(n).clone()));
                let declared: HashSet<String> = renames
                    .keys()
                    .filter(|o| names.contains(*o) && !bound.contains(*o))
                    .cloned()
                    .collect();
                bound.extend(declared.iter().cloned());
                out.extend(emit_ns_destructure_renamed(
                    &ns, &names, &types, &renames, &declared,
                ));
                // #1883 (review of #1923): a consumed context's content-hashed
                // assets, which the target's Worker ships too (its fixed paths
                // stay with its own Worker, and are not nameable).
                let mut hashed: Vec<String> = table
                    .assets
                    .iter()
                    .filter(|(n, d)| d.at.is_none() && !target_values.contains(n))
                    .filter(|(n, _)| seen_assets.insert(*n))
                    .map(|(n, _)| n.clone())
                    .collect();
                hashed.sort();
                bound.extend(hashed.iter().cloned());
                if !hashed.is_empty() {
                    out.extend(emit_ns_destructure(&ns, &hashed, &[]));
                }
            }
            // An `adapter` target has no `__makeSurface`/`deps.surface` entry —
            // its capabilities are already flattened onto `deps` directly
            // (Locale capability track, slice 1, #844).
            if is_adapter {
                continue;
            }
            let key = alias_for
                .get(q)
                .cloned()
                .unwrap_or_else(|| q.rsplit('.').next().unwrap_or(q.as_str()).to_string());
            // P7.2: deferred, not narrowed. `deps.surface`'s own per-capability
            // mock shapes are constructed elsewhere in this file; typing `{key}`
            // correctly here needs cross-referencing that mock-construction code
            // rather than guessing a structural type, which risks a `tsc --strict`
            // mismatch against whatever it actually builds. The *node
            // representation* has no such gap, though — `TsExpr::OptionalMember`
            // already exists — so review of #1404 caught this site's own
            // `format!` construction as a real miss, not a documented carve-out.
            let alias_stmt = TsStmt::const_stmt(
                TsBindingName::Ident(key.clone()),
                None,
                TsExpr::OptionalMember {
                    object: Box::new(member(
                        TsExpr::As {
                            expr: Box::new(ident("deps")),
                            ty: TsType::named("any"),
                        },
                        "surface",
                    )),
                    property: key.clone(),
                },
                None,
            );
            out.push(alias_stmt);
            bound.insert(key);
        }
    }
    bound
}

/// The `async function {name}() { … }` wrapper of a runner built as statements (1B.5b): no return
/// type, generics or parameters, never exported. The statements' own spans give the case's source
/// map; the module sets the function's `nested_map_source_id` to the case's source.
pub(super) fn async_runner_fn(name: &str, body: Vec<TsStmt>) -> TsStmt {
    TsStmt::decl(
        TsDecl::Function {
            name: name.to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body,
            is_async: true,
            inline: false,
        },
        None,
    )
}

/// The names the target's own module exports that a case's scope binds
/// unqualified: its types, fns, assets, embeds, consts, services and agents,
/// and each agent's construction factory, sorted.
fn target_scope_names(table: &UnitTable) -> Vec<String> {
    // #1883, slice 3: the target's assets are exported constants too.
    let mut names: Vec<String> = table
        .types
        .keys()
        .chain(table.fns.keys())
        .chain(table.assets.keys())
        .chain(table.embeds.keys())
        .chain(table.consts.keys())
        .cloned()
        .collect();
    // For contexts, also bring services and providers into scope.
    let extras: Vec<String> = table
        .services
        .keys()
        .chain(table.agents.keys())
        .cloned()
        .collect();
    names.extend(extras);
    // v0.9.2: bring each agent's construction factory into scope so a test
    // body's `AgentName(key)` lowers to `__makeAgentName(key)`.
    for agent in table.agents.keys() {
        names.push(crate::emitter::agent_factory_name(agent));
    }
    names.sort();
    names.dedup();
    names
}

/// #3536: whether a suite `type` named `name` replaces the first-party type of
/// that name the target's braces leave out.
pub(super) fn suite_replaces_left_out(
    unit_tables: &HashMap<String, UnitTable>,
    target_name: &str,
    suite_types: &[bynk_syntax::ast::TypeDecl],
    name: &String,
) -> bool {
    unit_tables.get(target_name).is_some_and(|t| {
        t.unselected_consumed_types.contains(name)
            && suite_types.iter().any(|s| s.name.name == *name)
    })
}

/// #3111: the local codecs the runner's lowered body reached for beyond the
/// always-present ones — a suite type under `Result` or another generic at a
/// `Json` boundary — as function declarations to splice ahead of the body.
/// `mark` is `RuntimeUse::json_codec_roots_len` taken before the runner lowered.
pub(super) fn suite_late_codec_stmts(
    suite_fns: test_suites::SuiteItems<'_>,
    typed: &checker::TypedCommons,
    runtime_use: &RuntimeUse,
    mark: usize,
) -> Vec<TsStmt> {
    if suite_fns.types.is_empty() {
        return Vec::new();
    }
    emitter::emit_suite_late_codecs(
        suite_fns.types,
        typed,
        &runtime_use.json_codec_qual(),
        runtime_use,
        &runtime_use.json_codec_roots_since(mark),
    )
}

/// #2834: the suite's helper `fn`s as local function declarations, for a runner
/// to splice in ahead of its body. Each runner declares its own suite's helpers,
/// after the scope setup, so a helper sees the same unqualified target names a
/// case does and stays private to its suite. `resolved` is the suite's view
/// *without* the helpers (they are added here, as the check pass adds them); the
/// helpers' types are merged into `typed`, and their names into `bound` so a
/// case `let` that reuses one is renamed. A helper's body carries no source map.
pub(super) fn suite_fn_decls(
    suite_fns: test_suites::SuiteItems<'_>,
    mut resolved: ResolvedCommons,
    target: Option<&UnitTable>,
    typed: &mut checker::TypedCommons,
    runtime_use: &RuntimeUse,
    bound: &mut HashSet<String>,
    tys: &Arc<Types>,
) -> Vec<TsStmt> {
    if suite_fns.is_empty() {
        return Vec::new();
    }
    let mut errs: Vec<CompileError> = Vec::new();
    // The fixtures are constants in the view the helpers are checked in.
    test_suites::add_suite_fns(
        &mut resolved,
        test_suites::SuiteItems {
            fns: &[],
            fixtures: suite_fns.fixtures,
            types: suite_fns.types,
            consts: suite_fns.consts,
        },
    );
    let (expr_types, callees) = test_suites::check_suite_fns(
        suite_fns.fns,
        &mut resolved,
        target,
        &mut errs,
        &mut RefSink::new(),
        &mut LocalsSink::new(),
        tys,
    );
    debug_assert!(
        errs.is_empty(),
        "suite helpers re-typed for emission reported errors the check pass did not: {:?}",
        errs.iter().map(|e| e.category).collect::<Vec<_>>()
    );
    typed.expr_types.extend(expr_types);
    typed.callees.extend(callees);
    test_suites::add_suite_fns_to_typed(typed, suite_fns.fns);
    // #3066: the suite's own types and constants, re-typed for their lowering
    // and declared ahead of everything that can name them.
    let (decl_types, decl_callees) = checker::check_suite_decls(
        suite_fns.types,
        suite_fns.consts,
        &resolved,
        tys,
        &mut errs,
        &mut RefSink::new(),
        &mut LocalsSink::new(),
    );
    typed.expr_types.extend(decl_types);
    typed.callees.extend(decl_callees);
    for t in suite_fns.types {
        typed.types.insert(t.name.name.clone(), Arc::new(t.clone()));
        bound.insert(t.name.name.clone());
    }
    for c in suite_fns.consts {
        typed.consts.insert(c.name.name.clone(), c.clone());
        bound.insert(c.name.name.clone());
    }
    let mut stmts: Vec<TsStmt> = Vec::new();
    if !suite_fns.types.is_empty() || !suite_fns.consts.is_empty() {
        stmts.extend(emitter::emit_suite_decls(
            suite_fns.types,
            suite_fns.consts,
            typed,
            &runtime_use.json_codec_qual(),
            runtime_use,
        ));
    }
    // #2841: each fixture is a local name for its file's module-level constant.
    for f in suite_fns.fixtures {
        bound.insert(f.name.name.clone());
        stmts.push(crate::ts_build::const_typed(
            f.name.name.clone(),
            TsType::named("string"),
            crate::ts_build::ident(fixture_ident(&f.path)),
        ));
    }
    for f in suite_fns.fns {
        let FnName::Free(name) = &f.name else {
            continue;
        };
        bound.insert(name.name.clone());
        stmts.extend(emitter::emit_suite_fn(f, typed, runtime_use));
    }
    stmts
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_test_case_function(
    runner_name: &str,
    // #2834/#2841: the declaring suite's helper `fn`s and fixtures.
    suite_fns: test_suites::SuiteItems<'_>,
    case: &Case,
    // #2134: the case's `env` entries, when the module holds `__bynkEnv`
    // (some case in it has entries): set first, and cleared on the way out
    // so a later `property` runs with none.
    env: Option<BTreeMap<String, bynk_check::test_env::EnvValue>>,
    deps_factory: &str,
    target_name: &str,
    target_kind: UnitKind,
    stubs: &HashMap<String, ResolvedStub>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    source: &str,
    rel_path: &str,
    runtime_use: &RuntimeUse,
    // #1860: the run's suite ordinal, when some `stub` is suite-scoped.
    suite: Option<usize>,
    hidden: &BTreeMap<String, String>,
    // #2842: the snapshot file the case takes a snapshot into, when it takes one.
    snapshot_file: Option<&str>,
    tys: &Arc<Types>,
) -> TsStmt {
    // #291: a case some `stub` clause is scoped to passes its name to
    // `__makeTestDeps`, so the clause applies to it.
    let scoped = stubs.values().any(|rp| {
        rp.clause_scopes
            .iter()
            .any(|s| matches!(s, StubScope::Case(c) if *c == case.name))
    });
    let mut out: Vec<TsStmt> = Vec::new();
    if let Some(file) = snapshot_file {
        out.push(TsStmt::expr_stmt(
            call(
                ident("__bynkSnapshotCase"),
                vec![
                    crate::ts_build::raw_lit(bynk_project::json_string(file)),
                    crate::ts_build::raw_lit(bynk_project::json_string(&case.name)),
                ],
            ),
            None,
        ));
    }
    if let Some(env) = &env {
        let entries = env_entries(env);
        out.push(TsStmt::assign(
            ident("__bynkEnv"),
            TsExpr::object(entries),
            None,
        ));
    }
    let mut harness_bound = emit_test_scope_setup(
        &mut out,
        deps_factory,
        target_name,
        target_kind,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        case_observes(&case.body),
        scoped.then_some(case.name.as_str()),
        suite,
        hidden,
        suite_fns.types,
    );
    let mut typed = synthetic_typed_commons_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        hidden,
        tys,
        suite_fns.types,
    );
    // v0.117: re-type-check the case body (with the call-record types registered)
    // so the lowering has full expr types — collection kernels, notably a
    // `trace(Cap.op)` result's `List[…]` methods, dispatch on the checked type.
    if let Some((mut resolved, _)) = test_suites::build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) {
        test_suites::register_call_record_types(&mut resolved, target_name, unit_tables);
        test_suites::add_suite_fns(&mut resolved, suite_fns);
        let mut throwaway_errors: Vec<CompileError> = Vec::new();
        let mut throwaway_refs = RefSink::new();
        // P6.21 review: `callees` is real output here now, not discarded —
        // see `typecheck_case_body`'s own doc comment for why a test-case
        // body's `Callee` classification was previously lost entirely.
        (typed.expr_types, typed.callees) = test_suites::typecheck_case_body(
            target_name,
            &case.body,
            case.span,
            unit_tables,
            &resolved,
            &mut throwaway_errors,
            &mut throwaway_refs,
            HashMap::new(),
            tys,
        );
        // #1875 review: as at the system tier, an error here is drift.
        debug_assert!(
            throwaway_errors.is_empty(),
            "a unit case re-typed for emission reported errors the check pass did not: {:?}",
            throwaway_errors
                .iter()
                .map(|e| e.category)
                .collect::<Vec<_>>()
        );
    }
    let roots_mark = runtime_use.json_codec_roots_len();
    // #2834: the suite's helpers, declared ahead of the body.
    if !suite_fns.is_empty()
        && let Some((helper_view, _)) = test_suites::build_privileged_resolved(
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
        )
    {
        let mut helper_view = helper_view;
        test_suites::register_call_record_types(&mut helper_view, target_name, unit_tables);
        out.extend(suite_fn_decls(
            suite_fns,
            helper_view,
            unit_tables.get(target_name),
            &mut typed,
            runtime_use,
            &mut harness_bound,
            tys,
        ));
    }
    let cross = bynk_check::resolver::CrossContextInfo::default();
    let test_services: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.services.keys().cloned().collect())
        .unwrap_or_default();
    let test_agents: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.agents.keys().cloned().collect())
        .unwrap_or_default();
    // #3585: a `bynk eval` snippet's case reports its tail's value.
    let (body, tail_span) = case_body::lower_test_case_stmts(
        &case.body,
        &mut typed,
        &cross,
        test_services,
        target_service_handler_kinds(unit_tables.get(target_name)),
        target_sum_handlers(unit_tables.get(target_name)),
        test_agents,
        target_hands_off_to_exec(target_name, target_kind, unit_tables),
        harness_bound,
        source,
        rel_path,
        runtime_use,
        case.name == crate::testgen::EVAL_CASE,
    );
    out.extend(suite_late_codec_stmts(
        suite_fns,
        &typed,
        runtime_use,
        roots_mark,
    ));
    out.extend(body);
    let mut return_pass_true = TsStmt::return_stmt(
        Some(TsExpr::object(vec![(
            "pass".to_string(),
            TsExpr::Lit(TsLit::Bool(true)),
        )])),
        None,
    );
    // The tail's checkpoint falls on the `return` when the tail added no statement of its own.
    return_pass_true.span = tail_span;
    out.push(return_pass_true);
    let expectation_error_branch = if_(
        TsExpr::Binary {
            op: TsBinaryOp::InstanceOf,
            left: Box::new(ident("e")),
            right: Box::new(ident("ExpectationError")),
        },
        block(vec![TsStmt::return_stmt(
            Some(TsExpr::object(vec![
                ("pass".to_string(), TsExpr::Lit(TsLit::Bool(false))),
                (
                    "error".to_string(),
                    TsExpr::object(vec![
                        ("message".to_string(), member(ident("e"), "message")),
                        ("location".to_string(), member(ident("e"), "location")),
                        // #3980: the value a failed `is` tested; absent otherwise.
                        ("actual".to_string(), member(ident("e"), "actual")),
                    ]),
                ),
            ])),
            None,
        )]),
    );
    let fallback_return = TsStmt::return_stmt(
        Some(TsExpr::object(vec![
            ("pass".to_string(), TsExpr::Lit(TsLit::Bool(false))),
            (
                "error".to_string(),
                TsExpr::object(vec![
                    (
                        "message".to_string(),
                        call(ident("String"), vec![ident("e")]),
                    ),
                    ("location".to_string(), str_lit("unknown")),
                    // #2272: the declaration a violated invariant or transition names.
                    (
                        "violation".to_string(),
                        call(ident("__bynkViolationOf"), vec![ident("e")]),
                    ),
                ]),
            ),
        ])),
        None,
    );
    let guarded = block(out);
    let handler = block(vec![expectation_error_branch, fallback_return]);
    let try_stmt = if env.is_some() {
        // `finally { __bynkEnv = {}; }` clears the case's `env` on the way out.
        TsStmt::try_catch_finally(
            guarded,
            Some("e"),
            handler,
            block(vec![TsStmt::assign(
                ident("__bynkEnv"),
                TsExpr::object(Vec::new()),
                None,
            )]),
            None,
        )
    } else {
        TsStmt::try_catch(guarded, Some("e"), handler, None)
    };
    async_runner_fn(runner_name, vec![try_stmt])
}

/// v0.114 (testing track slice 2): the runtime the generative `property` runner
/// relies on — a seeded PRNG, per-type shrinkers, the case loop, and shrinking.
/// Emitted once per test module that declares a `property`, alongside the
/// expectation helpers. `__bynkSeed` is the run's root seed (from
/// `BYNK_TEST_SEED`, else random), from which each property derives its seed via
/// `__bynkMix`, so `bynk test --seed <hex>` reproduces a run byte-for-byte.
/// v0.117: emit a TypeScript `type` alias for each observed capability
/// operation's call record (`type __Cap_op_Call = { param: T, … }`), so a
/// `trace(Cap.op)` list — whose elements are these records — type-checks under
/// `tsc` when a test projects a field (`c.msg`). Names mirror
/// [`checker::call_record_type_name`]. Ordered by capability then operation for
/// deterministic output.
/// #1479: returns real [`TsStmt`]s (was `String`) — its one caller now
/// appends via `crate::testgen::case_body::extend_printed_at(out, stmts, 0)`, the same
/// depth-0 top-level shape these type aliases always printed at.
pub(super) fn observation_call_record_types(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
) -> Vec<TsStmt> {
    let Some(table) = unit_tables.get(target_name) else {
        return Vec::new();
    };
    // Named/opaque parameter types are re-exported under the target's namespace,
    // so qualify them (`AuthId` → `commerce_payment.AuthId`); base types are
    // unaffected. Matches the mock-signature qualification.
    let scope_ns = test_ns(target_name);
    let mut type_ns: HashMap<String, String> = table
        .types
        .keys()
        .map(|n| (n.clone(), scope_ns.clone()))
        .collect();
    // #2886: a type reached through a `uses` commons is qualified by that
    // commons' namespace, as the stub signatures do.
    for u in unit_uses.get(target_name).into_iter().flatten() {
        if let Some(t) = unit_tables.get(u) {
            for n in public_types(t) {
                type_ns.entry(n.clone()).or_insert_with(|| test_ns(u));
            }
        }
    }
    // #291: a consumed adapter's capabilities are seams too, their parameter
    // types qualified by the adapter's namespace (`bynk.Request`).
    let mut caps: std::collections::BTreeMap<&String, &bynk_syntax::ast::CapabilityDecl> =
        table.capabilities.iter().collect();
    for (cap, decl, owner) in platform_seams(target_name, unit_tables) {
        let uns = test_ns(owner);
        for n in unit_tables[owner].types.keys() {
            type_ns.entry(n.clone()).or_insert_with(|| uns.clone());
        }
        caps.entry(cap).or_insert(decl);
    }
    // #3892: a consumed context's services are seams too, their parameter types
    // qualified by the context's namespace.
    let consumed: &[String] = unit_consumes
        .get(target_name)
        .map_or(&[][..], Vec::as_slice);
    let service_seams = bynk_check::test_suites::consumed_service_seam_decls(
        unit_tables,
        consumed,
        &unit_consumes_aliases
            .get(target_name)
            .cloned()
            .unwrap_or_default(),
    );
    for q in consumed {
        if let Some(t) = unit_tables.get(q)
            && t.kind != Some(UnitKind::Adapter)
            && !t.services.is_empty()
        {
            for n in public_types(t) {
                type_ns.entry(n.clone()).or_insert_with(|| test_ns(q));
            }
        }
    }
    for (cap, decl) in &service_seams {
        caps.entry(cap).or_insert(decl);
    }
    let mut stmts = Vec::new();
    for (cap, decl) in caps {
        for op in &decl.ops {
            let name = checker::call_record_type_name(cap, &op.name.name);
            let fields: Vec<TsTypeMember> = op
                .params
                .iter()
                .map(|p| TsTypeMember::Prop {
                    name: p.name.name.clone(),
                    ty: {
                        // A generic operation's own type parameter (`Events.emit[E]`)
                        // has no binding here, so a field mentioning one is `unknown`.
                        let ty = emitter::ts_types::ts_type_ref_qualified_multi_ts_type(
                            &p.type_ref,
                            &type_ns,
                        );
                        // The operation's own type parameters are reported even when one is spelled
                        // like a global type (`Error`, `Map`): it is the parameter here, not the global.
                        let used = bynk_ts::unbound_names_in(
                            &[const_typed("__t", ty.clone(), ident("__v"))],
                            |n| op.type_params.iter().any(|tp| tp.name.name == n),
                        );
                        let generic = op.type_params.iter().any(|tp| used.contains(&tp.name.name));
                        if generic {
                            TsType::named("unknown")
                        } else {
                            ty
                        }
                    },
                    optional: false,
                    readonly: false,
                })
                .collect();
            // A zero-param capability op's call record is the empty object type, printed `{}`
            // (1B.5e; the ruled 1A.40 class `{  }` to `{}`).
            let ty = TsType::Object(fields);
            stmts.push(TsStmt::decl(
                TsDecl::TypeAlias {
                    name,
                    type_params: Vec::new(),
                    ty,
                },
                None,
            ));
        }
    }
    stmts
}
