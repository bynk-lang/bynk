//! Test scope setup and the per-case function.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use bynk_emit::emitter::RuntimeUse;
// #1759: every namespace a test module imports, from one definition.
use bynk_check::checker::Types;
use bynk_check::test_suites::{self, ResolvedStub, StubScope};
use bynk_emit::emitter::emit::test_scaffold_ns as test_ns;
use bynk_syntax::ast::{Case, FnName};
use bynk_ts::{
    SourceMapBuilder, TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsStmt, TsType,
    TsTypeMember,
};
use std::sync::Arc;

/// Emit the shared per-runner scope setup — agent reset, the `deps` factory, and
/// the destructurings (see [`emit_ns_destructure`]) that bring the target's,
/// `uses`', and consumed contexts' names into scope. Shared by `case` and
/// `property` runners so a property body resolves names exactly as a case does.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_test_scope_setup(
    out: &mut String,
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
) -> HashSet<String> {
    // #2024: every name this setup binds in the runner's scope, returned so the
    // body's lowering renames a case `let` that would redeclare one.
    let mut bound: HashSet<String> = ["deps", "__obs"].map(String::from).into();
    let deps_args: Vec<TsExpr> = match suite {
        Some(n) => vec![
            case_name.map_or_else(|| ident("undefined"), str_lit),
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
        out.push_str(&bynk_ts::print_stmt(&stmt, 2));
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
            if seams.is_empty() {
                return None;
            }
            let entries: Vec<(String, TsExpr)> = seams
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
            out.push_str(&bynk_ts::print_stmt(&obs_stmt, 2));
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
            out.push_str(&bynk_ts::print_stmt(&deps_stmt, 2));
        } else {
            let deps_stmt = TsStmt::const_stmt(
                TsBindingName::Ident("deps".to_string()),
                None,
                call(ident(deps_factory), deps_args.clone()),
                None,
            );
            out.push_str(&bynk_ts::print_stmt(&deps_stmt, 2));
        }
    } else {
        let deps_stmt = TsStmt::const_stmt(
            TsBindingName::Ident("deps".to_string()),
            None,
            TsExpr::object(vec![]),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&deps_stmt, 2));
    }
    // Bring the target's top-level names into local scope so the lowered
    // body can reference them unqualified. The target's types and fns are
    // exported from its namespace by the production emitter.
    if let Some(table) = unit_tables.get(target_name) {
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
            names.push(bynk_emit::emitter::agent_factory_name(agent));
        }
        names.sort();
        names.dedup();
        let type_names = aliased_types(table, |_| true);
        bound.extend(names.iter().cloned());
        bynk_emit::emitter::extend_printed_at(
            out,
            emit_ns_destructure(&target_ns, &names, &type_names),
            2,
        );
    }
    // Bring in `uses` commons names too — the target's body can use them.
    let (_, target_values) = target_names(target_name, unit_tables);
    let mut seen_assets: HashSet<&String> = HashSet::new();
    for (ns, names, type_names) in
        used_commons_destructures(target_name, unit_tables, unit_uses, &mut seen_assets)
    {
        bound.extend(names.iter().cloned());
        bynk_emit::emitter::extend_printed_at(
            out,
            emit_ns_destructure(&ns, &names, &type_names),
            2,
        );
    }
    // #1814: the types the target's view reaches but does not import, from
    // their owning commons (imported by `emit_test_module`).
    extend_hidden_destructures(out, &hidden_by_owner(hidden), unit_tables);
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
                let types = aliased_types(table, |n| is_public_type(table, n));
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
                bynk_emit::emitter::extend_printed_at(
                    out,
                    emit_ns_destructure_renamed(&ns, &names, &types, &renames, &declared),
                    2,
                );
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
                    bynk_emit::emitter::extend_printed_at(
                        out,
                        emit_ns_destructure(&ns, &hashed, &[]),
                        2,
                    );
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
            out.push_str(&bynk_ts::print_stmt(&alias_stmt, 2));
            bound.insert(key);
        }
    }
    bound
}

/// #1484: the `async function {name}() { ... }` wrapper every test-generation
/// runner shares (`emit_test_case_function`/`emit_test_property_function`/
/// `emit_test_history_property_function`/`emit_contract_attack_function`, and
/// `emit_integration_module`'s own inline case wrapper) — no return type
/// annotation, no generics, no params, never exported. One shared
/// construction (a real [`TsDecl::Function`]) instead of five separate
/// `format!("async function {name}() {{\n")` hand-templates.
///
/// `body_text` is the function's own already-fully-formed inner content —
/// everything between the wrapper's own opening and closing braces (a
/// `try`/`catch`, or a flat sequence of statements ending in a `return`),
/// carried as one opaque [`TsStmt::raw`] and not decomposed further: it mixes
/// real node prints (`bynk_ts::print_stmt` calls already scattered through
/// each caller) with genuinely-opaque lowered bodies (`lower.rs`'s permanent
/// exclusion, ADR 0391) at depths this track's own established convention
/// for these files uses, so decomposing the wrapper's *interior* into a
/// fully real tree per site would risk silently changing that convention's
/// baked-in indentation — the `TsDecl::Function`/`Raw`-body combination
/// `emit_free_fn` (#1480) already established for exactly this reason.
/// `nested_map`, when `Some`, is the one real per-case source map this
/// track still merges (`emit_test_case_function`'s own `case_smb`/
/// `emit_integration_module`'s own inline case) — every other caller's own
/// body map is already a deliberate, pre-existing scope cut (property/
/// history-property/contract-attack bodies are collaborator-free predicate
/// scaffolding), unchanged by this conversion.
pub(super) fn async_test_runner_fn(
    name: &str,
    body_text: String,
    nested_map: Option<SourceMapBuilder>,
) -> TsStmt {
    let mut raw_body = TsStmt::raw(body_text, None);
    raw_body.nested_map = nested_map;
    TsStmt::decl(
        TsDecl::Function {
            name: name.to_string(),
            generics: Vec::new(),
            params: Vec::new(),
            return_type: None,
            body: vec![raw_body],
            is_async: true,
            inline: false,
        },
        None,
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
) -> String {
    if suite_fns.is_empty() {
        return String::new();
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
    let mut out = String::new();
    if !suite_fns.types.is_empty() || !suite_fns.consts.is_empty() {
        let decls = emitter::emit_suite_decls(
            suite_fns.types,
            suite_fns.consts,
            typed,
            &runtime_use.json_codec_qual(),
            runtime_use,
        );
        out.push_str(&reindent_by(&decls, "    "));
    }
    // #2841: each fixture is a local name for its file's module-level constant.
    for f in suite_fns.fixtures {
        bound.insert(f.name.name.clone());
        out.push_str(&format!(
            "    const {}: string = {};\n",
            f.name.name,
            fixture_ident(&f.path)
        ));
    }
    for f in suite_fns.fns {
        let FnName::Free(name) = &f.name else {
            continue;
        };
        bound.insert(name.name.clone());
        let stmts = emitter::emit_suite_fn(f, typed, runtime_use);
        let printed: String = stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect();
        out.push_str(&reindent_by(&printed, "    "));
    }
    out
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
    let mut out = String::new();
    out.push_str("  try {\n");
    if let Some(file) = snapshot_file {
        out.push_str(&format!(
            "    __bynkSnapshotCase({}, {});\n",
            bynk_project::json_string(file),
            bynk_project::json_string(&case.name)
        ));
    }
    if let Some(env) = &env {
        let entries = env_entries(env);
        out.push_str(&bynk_ts::print_stmt(
            &TsStmt::assign(ident("__bynkEnv"), TsExpr::object(entries), None),
            2,
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
        block_uses_observation(&case.body),
        scoped.then_some(case.name.as_str()),
        suite,
        hidden,
    );
    let mut typed = synthetic_typed_commons_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        hidden,
        tys,
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
        out.push_str(&suite_fn_decls(
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
    let (body_src, body_smb) = emitter::lower_test_case_body(
        &case.body,
        &mut typed,
        &cross,
        test_services,
        target_service_handler_kinds(unit_tables.get(target_name)),
        target_sum_handlers(unit_tables.get(target_name)),
        test_agents,
        target_uses_send(target_name, target_kind, unit_tables),
        harness_bound,
        source,
        rel_path,
        runtime_use,
    );
    // v0.70: splice the case body (line-by-line, indented) and merge its source-map
    // sub-builder into the case builder, line-anchored at the splice. The caller
    // (emit_test_module) merges this case builder into the module map under the
    // case's source file.
    let body_base = out.len();
    for line in body_src.lines() {
        out.push_str("    ");
        out.push_str(line);
        out.push('\n');
    }
    let mut case_smb = SourceMapBuilder::new();
    case_smb.merge(&body_smb, &body_src, &out, body_base, 0);
    let return_pass_true = TsStmt::return_stmt(
        Some(TsExpr::object(vec![(
            "pass".to_string(),
            TsExpr::Lit(TsLit::Bool(true)),
        )])),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&return_pass_true, 2));
    out.push_str("  } catch (e) {\n");
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
                    ]),
                ),
            ])),
            None,
        )]),
    );
    out.push_str(&bynk_ts::print_stmt(&expectation_error_branch, 2));
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
    out.push_str(&bynk_ts::print_stmt(&fallback_return, 2));
    if env.is_some() {
        out.push_str("  } finally {\n    __bynkEnv = {};\n  }\n");
    } else {
        out.push_str("  }\n");
    }
    async_test_runner_fn(runner_name, out, Some(case_smb))
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
/// appends via `bynk_emit::emitter::extend_printed_at(out, stmts, 0)`, the same
/// depth-0 top-level shape these type aliases always printed at.
pub(super) fn observation_call_record_types(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
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
                        let ty =
                            emitter::ts_type_ref_qualified_multi_ts_type(&p.type_ref, &type_ns);
                        let printed = bynk_ts::print_type(&ty);
                        let generic = op.type_params.iter().any(|tp| {
                            printed
                                .split(|c: char| !c.is_alphanumeric() && c != '_')
                                .any(|w| w == tp.name.name)
                        });
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
            // The same `"{  }"` double-space quirk as elsewhere in this file
            // (`gen_ts_for_ty`/`canon_ts_for_ty`, Arc C slice 31; `emit_test_deps`
            // above) — the pre-conversion `format!("type {name} = {{ {} }};\n",
            // fields.join("; "))` template always has a space on each side of its
            // `{}` slot, so a zero-param capability op (real and reachable)
            // literally produced a double space, not the tight `"{}"`
            // `TsType::Object`'s own empty-members shortcut renders.
            let ty = if fields.is_empty() {
                TsType::named("{  }")
            } else {
                TsType::Object(fields)
            };
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

/// v0.117: the observation runtime — wraps each observed capability operation on
/// the test `deps` so every call records its arguments and a monotonic order
/// index into the per-case trace `__obs`. Emitted once per module that observes.
/// Finding #17: source lives at `emitter/test_runtime/observation.ts`.
pub(super) fn observation_runtime_helpers() -> String {
    include_str!("../test_runtime/observation.ts").to_string()
}

pub(super) fn property_runtime_helpers() -> String {
    include_str!("../test_runtime/property.ts").to_string()
}

/// v0.119 (testing track slice 7, ADR 0155): the history-property runtime, emitted
/// alongside [`property_runtime_helpers`] only when a module declares a
/// `for all run: History[Agent]` property. Generates a bounded, seeded sequence of
/// handler calls, drives it through the real handlers (`spec.drive`, the agent
/// module's `__bynkDriveHistory_<Agent>`), evaluates the predicate over the
/// observed `run`, and on failure delta-debugs the *sequence* (re-driving after
/// each reduction so the counterexample stays reachable) then shrinks the surviving
/// arguments — reporting the seed, the shrunk sequence, and a reproduce line.
pub(super) fn history_runtime_helpers() -> String {
    include_str!("../test_runtime/history.ts").to_string()
}
