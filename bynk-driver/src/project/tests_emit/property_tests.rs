//! Property, history-property and contract-attack function emission.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use bynk_emit::emitter::RuntimeUse;
// #1759: every namespace a test module imports, from one definition.
use bynk_check::checker::Types;
use bynk_check::test_suites::{self};
use bynk_emit::emitter::emit::test_scaffold_ns as test_ns;
use bynk_syntax::ast::{
    BaseType, BinOp, Block, Expr, ExprKind, FnDecl, FnName, Ident, PropertyDecl, TypeRef,
};
use bynk_ts::{TsArrowBody, TsBindingName, TsExpr, TsLit, TsParam, TsStmt, TsType};
use std::sync::Arc;

/// v0.114: emit one async runner for a generative `property` — the binding
/// generators, the `where` filter and predicate body as closures over the
/// generated tuple, and the `__bynkRunProperty` call that draws cases, shrinks a
/// counterexample, and reports the seed + shrunk tuple + reproduce line.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_test_property_function(
    runner_name: &str,
    // #2834/#2841: the declaring suite's helper `fn`s and fixtures.
    suite_fns: test_suites::SuiteItems<'_>,
    prop: &PropertyDecl,
    prop_ordinal: usize,
    target_name: &str,
    target_kind: UnitKind,
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
    tys: &Arc<Types>,
) -> TsStmt {
    let mut out = String::new();
    let mut harness_bound = emit_test_scope_setup(
        &mut out,
        "__makeTestDeps",
        target_name,
        target_kind,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        false,
        None,
        suite,
        hidden,
    );

    // Generator descriptors, one per binding, over the target's privileged type
    // view (so refined/opaque constructors resolve).
    let resolved = test_suites::build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    )
    .map(|(mut r, _)| {
        test_suites::add_suite_fns(&mut r, suite_fns);
        r
    });
    // P7.2: deferred, not narrowed. `v`'s real type varies per binding, and
    // for `Int` (and refined-`Int`) bindings the *internal* property-test
    // representation is `bigint` (see `rng.int(...)`'s own `n`-suffixed
    // literals and `__bynkShrinkInt(v: bigint, ...)` above) — not `number`,
    // which is what a general `ts_type_ref`/`ts_ty` rendering of the same
    // declared type would produce. Narrowing correctly needs threading the
    // resolved type (or its internal representation) out of `BindingGen`
    // itself, not a same-line text change, and a wrong guess here risks a
    // real `tsc --strict` failure specifically on the shrink helpers' own
    // typed parameters.
    let gens: Vec<TsExpr> = prop
        .forall
        .bindings
        .iter()
        .map(|b| {
            let bg = resolved
                .as_ref()
                .and_then(|r| checker::resolve_type_ref(&b.type_ref, &r.types, tys).map(|t| (t, r)))
                .map(|(t, r)| binding_gen(t, &r.types, tys))
                .unwrap_or(BindingGen {
                    boundaries: Vec::new(),
                    gen_ts: ident("undefined"),
                    shrink: TsExpr::array(vec![]),
                });
            gen_descriptor_entry(Some(str_lit(b.name.name.clone())), &bg)
        })
        .collect();
    let gens_stmt = TsStmt::const_stmt(
        TsBindingName::Ident("__gens".to_string()),
        None,
        TsExpr::multiline_array(gens),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&gens_stmt, 2));

    // The `where` filter and the predicate body, as closures over the tuple.
    let mut typed = synthetic_typed_commons_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        hidden,
        tys,
    );
    let roots_mark = runtime_use.json_codec_roots_len();
    // #2834: the suite's helpers, declared ahead of the `where` filter and body.
    if !suite_fns.is_empty()
        && let Some((mut helper_view, _)) = test_suites::build_privileged_resolved(
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
        )
    {
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
    let binding_names: Vec<String> = prop
        .forall
        .bindings
        .iter()
        .map(|b| b.name.name.clone())
        .collect();
    let binding_types: Vec<Option<checker::TyId>> = prop
        .forall
        .bindings
        .iter()
        .map(|b| {
            resolved
                .as_ref()
                .and_then(|r| checker::resolve_type_ref(&b.type_ref, &r.types, tys))
        })
        .collect();
    let destructure = destructure_vals(
        &binding_names,
        &binding_types,
        resolved.as_ref().map(|r| &r.types),
        tys,
    );
    // #1427: the `for all` bindings' own resolved types, in scope for both
    // the `where`-clause and body re-type-check passes below — mirrors
    // `emit_test_history_property_function`'s own `run: List[Step]` scope
    // entry, generalised to every binding a property declares.
    let initial_scope: HashMap<String, checker::TyId> = binding_names
        .iter()
        .cloned()
        .zip(binding_types.iter().copied())
        .filter_map(|(name, t)| t.map(|t| (name, t)))
        .collect();

    if let Some(w) = &prop.forall.where_pred {
        let synth = Block {
            statements: Vec::new(),
            tail: Box::new(w.clone()),
            span: w.span,
            tail_leading_comments: Vec::new(),
            implicit_tail: false,
        };
        // #1427: re-type-check the `where` clause, the same pass
        // `emit_test_case_function` already runs for a plain `case` body —
        // without it, `typed.expr_types` has no entry for anything here, so
        // a match's own discriminant type is unresolvable and payload-
        // binding lowering (`positional_field_name`) silently falls back to
        // the generic single-field `"value"` name instead of a variant's
        // own real field name.
        if let Some(r) = resolved.as_ref() {
            let mut throwaway_errors: Vec<CompileError> = Vec::new();
            let mut throwaway_refs = RefSink::new();
            let (where_types, where_callees) = test_suites::typecheck_case_body(
                target_name,
                &synth,
                w.span,
                unit_tables,
                r,
                &mut throwaway_errors,
                &mut throwaway_refs,
                initial_scope.clone(),
                tys,
            );
            typed.expr_types.extend(where_types);
            typed.callees.extend(where_callees);
        }
        let (src, _) = emitter::lower_block_to_async_body(
            &synth,
            &TypeRef::Base(BaseType::Bool, w.span),
            &mut typed,
            &cross,
            runtime_use,
        );
        // P7.2: deferred — `__vals`'s elements are heterogeneous, one per binding,
        // some internally `bigint`-represented; same reason as `__gens`'s own
        // construction elsewhere in this function.
        out.push_str("    const __where = (__vals: any[]) => {\n");
        out.push_str(&format!("      {destructure}\n"));
        for line in src.lines() {
            out.push_str("      ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("    };\n");
    } else {
        let where_null = TsStmt::const_stmt(
            TsBindingName::Ident("__where".to_string()),
            None,
            TsExpr::Lit(TsLit::Null),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&where_null, 2));
    }

    let test_services: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.services.keys().cloned().collect())
        .unwrap_or_default();
    let test_agents: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.agents.keys().cloned().collect())
        .unwrap_or_default();
    // #1427: re-type-check the property body itself — same reason and same
    // pass as the `where` clause above.
    if let Some(r) = resolved.as_ref() {
        let mut throwaway_errors: Vec<CompileError> = Vec::new();
        let mut throwaway_refs = RefSink::new();
        let (body_types, body_callees) = test_suites::typecheck_case_body(
            target_name,
            &prop.forall.body,
            prop.span,
            unit_tables,
            r,
            &mut throwaway_errors,
            &mut throwaway_refs,
            initial_scope,
            tys,
        );
        typed.expr_types.extend(body_types);
        typed.callees.extend(body_callees);
    }
    // Property bodies are collaborator-free predicate scaffolding; like mock op
    // bodies, their source map is a deliberate scope cut (the `expect` location
    // still binds through `assert_loc`).
    // #2024: the `for all` binders share the body closure's scope.
    harness_bound.extend(binding_names.iter().cloned());
    let (body_src, _body_smb) = emitter::lower_test_case_body(
        &prop.forall.body,
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
    out.push_str(&suite_late_codecs(
        suite_fns,
        &typed,
        runtime_use,
        roots_mark,
    ));
    // P7.2: deferred, same reason as `__where`'s own `__vals` above.
    out.push_str("    const __body = async (__vals: any[]) => {\n");
    out.push_str(&format!("      {destructure}\n"));
    for line in body_src.lines() {
        out.push_str("      ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("    };\n");

    // Forward slashes so the emitted location/reproduce path is portable — on
    // Windows a `PathBuf` join yields `\`, which must not leak into the golden
    // `.ts` (mirrors `discovered_location`'s normalisation).
    let rel_path_fwd = rel_path.replace('\\', "/");
    let return_run = TsStmt::return_stmt(
        Some(await_expr(call(
            ident("__bynkRunProperty"),
            vec![TsExpr::object(vec![
                (
                    "seed".to_string(),
                    call(
                        ident("__bynkMix"),
                        vec![ident("__bynkSeed"), num_lit(prop_ordinal.to_string())],
                    ),
                ),
                ("cases".to_string(), num_lit("100")),
                ("gens".to_string(), ident("__gens")),
                ("where".to_string(), ident("__where")),
                ("body".to_string(), ident("__body")),
                ("name".to_string(), str_lit(prop.name.clone())),
                ("location".to_string(), str_lit(rel_path_fwd.clone())),
                ("file".to_string(), str_lit(rel_path_fwd)),
            ])],
        ))),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&return_run, 2));
    async_test_runner_fn(runner_name, out, None)
}

/// v0.119 (testing track slice 7, ADR 0155): emit one async runner for a history
/// property — `for all run: History[Agent]`. Builds a per-handler argument
/// generator table, a predicate `__body` over the driven `run` (a `List[Step]`),
/// and a `__drive` closure calling the agent module's `__bynkDriveHistory_<Agent>`,
/// then hands them to `__bynkRunHistory`, which generates, drives, shrinks, and
/// reports exactly like a value `property`. Mirrors [`emit_test_property_function`].
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_test_history_property_function(
    runner_name: &str,
    // #2834/#2841: the declaring suite's helper `fn`s and fixtures.
    suite_fns: test_suites::SuiteItems<'_>,
    prop: &PropertyDecl,
    prop_ordinal: usize,
    target_name: &str,
    target_kind: UnitKind,
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
    tys: &Arc<Types>,
) -> TsStmt {
    let roots_mark = runtime_use.json_codec_roots_len();
    let mut out = String::new();
    let mut harness_bound = emit_test_scope_setup(
        &mut out,
        "__makeTestDeps",
        target_name,
        target_kind,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        false,
        None,
        suite,
        hidden,
    );

    let Some((run_var, agent_name)) = prop_history_binding(prop) else {
        // Defensive: the checker rejects a malformed history property before emit,
        // so this is unreachable — emit a trivially-passing runner rather than panic.
        let return_pass_true = TsStmt::return_stmt(
            Some(TsExpr::object(vec![(
                "pass".to_string(),
                TsExpr::Lit(TsLit::Bool(true)),
            )])),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&return_pass_true, 2));
        return async_test_runner_fn(runner_name, out, None);
    };

    // The synthesised call/step/state types are checker-only (never emitted as
    // real TS), but the lowered predicate annotates the driven history with them.
    // Alias each to `any` so the driven plain objects type-check structurally.
    out.push_str(&format!(
        "    type __History_{agent_name}_Step = any; type __History_{agent_name}_Call = any; type __History_{agent_name}_State = any;\n"
    ));

    // The privileged view, plus the synthetic call/step/state types and the body's
    // expr types (with `run: List[Step]` in scope), so the lowering resolves the
    // predicate's `List` and value surface (`.call is …`, `.old`/`.new`, `.upTo`).
    let mut typed = synthetic_typed_commons_for_target(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        hidden,
        tys,
    );
    let mut handler_descs: Vec<TsExpr> = Vec::new();
    if let Some((mut resolved, _)) = test_suites::build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) {
        test_suites::register_call_record_types(&mut resolved, target_name, unit_tables);
        // #2834: the suite's helpers are callable from the predicate.
        let mut helper_view = test_suites::build_privileged_resolved(
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
        )
        .map(|(mut r, _)| {
            test_suites::register_call_record_types(&mut r, target_name, unit_tables);
            r
        });
        test_suites::add_suite_fns(&mut resolved, suite_fns);
        let inner = TypeRef::Named(Ident {
            name: agent_name.to_string(),
            span: prop.span,
        });
        let mut throwaway_refs = RefSink::new();
        let step_ty = test_suites::check_history_binding(
            &inner,
            prop.span,
            &mut resolved,
            &mut throwaway_refs,
            tys,
        )
        .ok();

        // Per-handler argument generators (the slice-2 value generator over each
        // handler parameter), in declaration order — the sequence generator picks a
        // handler uniformly and draws its arguments here.
        if let Some(agent) = resolved.agents.get(agent_name) {
            for h in test_suites::history_handlers(agent) {
                let tag = test_suites::history_variant_name(&h.method_name.as_ref().unwrap().name);
                // P7.2: deferred — same reason as `__gens`'s own construction
                // above (`Int`'s internal `bigint` representation).
                let gens: Vec<TsExpr> = h
                    .params
                    .iter()
                    .map(|p| {
                        let bg = checker::resolve_type_ref(&p.type_ref, &resolved.types, tys)
                            .map(|t| binding_gen(t, &resolved.types, tys))
                            .unwrap_or(BindingGen {
                                boundaries: Vec::new(),
                                gen_ts: ident("undefined"),
                                shrink: TsExpr::array(vec![]),
                            });
                        gen_descriptor_entry(None, &bg)
                    })
                    .collect();
                handler_descs.push(TsExpr::object(vec![
                    ("tag".to_string(), str_lit(tag)),
                    ("gens".to_string(), TsExpr::array(gens)),
                ]));
            }
        }

        if let Some(step_ty) = step_ty {
            let mut scope: HashMap<String, checker::TyId> = HashMap::new();
            scope.insert(run_var.to_string(), tys.intern(step_ty));
            let mut throwaway_errors: Vec<CompileError> = Vec::new();
            let mut throwaway_refs2 = RefSink::new();
            // P6.21 review: `callees` is real output here now — see
            // `typecheck_case_body`'s own doc comment.
            (typed.expr_types, typed.callees) = test_suites::typecheck_case_body(
                target_name,
                &prop.forall.body,
                prop.span,
                unit_tables,
                &resolved,
                &mut throwaway_errors,
                &mut throwaway_refs2,
                scope,
                tys,
            );
            if let Some(helper_view) = helper_view.take() {
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
        }
        // Carry the synthetic call/step/state types into the lowering commons so
        // `is` on the call sum and field access on the step resolve.
        for (n, d) in &resolved.types {
            typed.types.entry(n.clone()).or_insert_with(|| d.clone());
        }
    }

    let handlers_stmt = TsStmt::const_stmt(
        TsBindingName::Ident("__handlers".to_string()),
        None,
        TsExpr::multiline_array(handler_descs),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&handlers_stmt, 2));

    // The predicate body, as a closure over the driven history `run`.
    let cross = bynk_check::resolver::CrossContextInfo::default();
    let test_services: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.services.keys().cloned().collect())
        .unwrap_or_default();
    let test_agents: HashSet<String> = unit_tables
        .get(target_name)
        .map(|t| t.agents.keys().cloned().collect())
        .unwrap_or_default();
    // #2024: the driven history's binder shares the body closure's scope.
    harness_bound.insert(run_var.to_string());
    let (body_src, _body_smb) = emitter::lower_test_case_body(
        &prop.forall.body,
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
    out.push_str(&suite_late_codecs(
        suite_fns,
        &typed,
        runtime_use,
        roots_mark,
    ));
    // P7.2: matches `__bynkDriveHistory_*`'s own real signature — see
    // `emit.rs`'s own driver-signature narrowing and the matching `__drive`
    // call site below (`{target_ns}.__bynkDriveHistory_{agent_name}`).
    out.push_str(
        "    const __body = async (__run: globalThis.Array<{ h: number, args: unknown[] }>) => {\n",
    );
    out.push_str(&format!("      const {run_var} = __run;\n"));
    for line in body_src.lines() {
        out.push_str("      ");
        out.push_str(line);
        out.push('\n');
    }
    out.push_str("    };\n");

    // Drive a generated sequence through the real handlers via the agent module's
    // exported test driver, threading the test `deps` (real or `stub`-stubbed).
    let target_ns = test_ns(target_name);
    // P7.2: `seq` matches `__bynkDriveHistory_*`'s own real param type
    // (`emit.rs`'s own driver-signature narrowing). `(target_ns as any)` and
    // `deps` stay deferred — the callee's own `deps: any` param is itself
    // deferred (per-handler `given` sets can differ; see `emit.rs`), and
    // `target_ns`'s own generated namespace shape wasn't traced here. The
    // `Array<{ h: number, args: unknown[] }>` param type stays one opaque
    // `TsType::named` string — its own inline object type's real member
    // separator is `, ` (a comma), not `TsType::Object`'s own established
    // `; ` (semicolon) convention, so building it as a real `TsType::Object`
    // would silently change the emitted bytes; the same "odd, one-off shape
    // stays opaque text" posture this track already uses for `Query[T]`/
    // `this.state.storage.get<T>`/`ReturnType<typeof X>`.
    let drive_stmt = TsStmt::const_stmt(
        TsBindingName::Ident("__drive".to_string()),
        None,
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "seq".to_string(),
                ty: Some(TsType::named(
                    "globalThis.Array<{ h: number, args: unknown[] }>",
                )),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(method_call(
                TsExpr::As {
                    expr: Box::new(ident(target_ns)),
                    ty: TsType::named("any"),
                },
                &format!("__bynkDriveHistory_{agent_name}"),
                vec![ident("seq"), ident("deps")],
            )))),
        },
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&drive_stmt, 2));

    let rel_path_fwd = rel_path.replace('\\', "/");
    let return_run = TsStmt::return_stmt(
        Some(await_expr(call(
            ident("__bynkRunHistory"),
            vec![TsExpr::object(vec![
                (
                    "seed".to_string(),
                    call(
                        ident("__bynkMix"),
                        vec![ident("__bynkSeed"), num_lit(prop_ordinal.to_string())],
                    ),
                ),
                ("cases".to_string(), num_lit("60")),
                ("maxLen".to_string(), num_lit("16")),
                ("handlers".to_string(), ident("__handlers")),
                ("drive".to_string(), ident("__drive")),
                ("body".to_string(), ident("__body")),
                ("name".to_string(), str_lit(prop.name.clone())),
                ("location".to_string(), str_lit(rel_path_fwd.clone())),
                ("file".to_string(), str_lit(rel_path_fwd)),
            ])],
        ))),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&return_run, 2));
    async_test_runner_fn(runner_name, out, None)
}

/// v0.115 (testing track slice 3): emit one async runner that *attacks* a
/// contracted free function — generate arguments over the parameter domains
/// (v0.114 engine), filter by the conjunction of `requires` (`__where`), call
/// the function (`__body`), and let the dev/test call-site guard assert each
/// `ensures`, throwing a shrinkable `BynkContractError` on violation. Mirrors
/// [`emit_test_property_function`], but the body is a direct call (the guard is
/// the assertion) with `Int` arguments coerced to `number` (the generator makes
/// `bigint`; functions do `number` arithmetic).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_contract_attack_function(
    runner_name: &str,
    f: &FnDecl,
    resolved: &ResolvedCommons,
    prop_ordinal: usize,
    target_name: &str,
    target_kind: UnitKind,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    rel_path: &str,
    runtime_use: &RuntimeUse,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> Option<TsStmt> {
    let FnName::Free(fname) = &f.name else {
        return None;
    };
    let mut out = String::new();
    // The attack lowers no case body, so it has no `let` to rename.
    let _ = emit_test_scope_setup(
        &mut out,
        "__makeTestDeps",
        target_name,
        target_kind,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
        false,
        None,
        None,
        hidden,
    );
    let _ = target_kind;

    // Generator descriptors, one per parameter, over the target's privileged view.
    //
    // P7.2: deferred, not narrowed. `v`'s real type varies per binding, and
    // for `Int` (and refined-`Int`) bindings the *internal* property-test
    // representation is `bigint` (see `rng.int(...)`'s own `n`-suffixed
    // literals and `__bynkShrinkInt(v: bigint, ...)` above) — not `number`,
    // which is what a general `ts_type_ref`/`ts_ty` rendering of the same
    // declared type would produce. Narrowing correctly needs threading the
    // resolved type (or its internal representation) out of `BindingGen`
    // itself, not a same-line text change, and a wrong guess here risks a
    // real `tsc --strict` failure specifically on the shrink helpers' own
    // typed parameters.
    let gens: Vec<TsExpr> = f
        .params
        .iter()
        .map(|p| {
            let bg = checker::resolve_type_ref(&p.type_ref, &resolved.types, tys)
                .map(|t| binding_gen(t, &resolved.types, tys))
                .unwrap_or(BindingGen {
                    boundaries: Vec::new(),
                    gen_ts: ident("undefined"),
                    shrink: TsExpr::array(vec![]),
                });
            gen_descriptor_entry(Some(str_lit(p.name.name.clone())), &bg)
        })
        .collect();
    let gens_stmt = TsStmt::const_stmt(
        TsBindingName::Ident("__gens".to_string()),
        None,
        TsExpr::multiline_array(gens),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&gens_stmt, 2));

    let param_names: Vec<String> = f.params.iter().map(|p| p.name.name.clone()).collect();
    let param_types: Vec<Option<checker::TyId>> = f
        .params
        .iter()
        .map(|p| checker::resolve_type_ref(&p.type_ref, &resolved.types, tys))
        .collect();
    // #1426: the same top-level Int-binding representation gap `emit_test_
    // property_function`'s own `for all` bindings have — an attacked
    // function's own fuzzed `Int` parameter draws `bigint` exactly the same
    // way, and its `requires`/body can do ordinary arithmetic on it.
    let destructure = destructure_vals(&param_names, &param_types, Some(&resolved.types), tys);

    // `__where` — the conjunction of `requires`, lowered over the parameter
    // tuple. #1426: `destructure` above already coerces every `Int`-drawing
    // param to `number` before this closure's own body runs, so a `requires`
    // clause doing real arithmetic (not just comparison) on a param is safe
    // too — comparisons alone tolerated the pre-fix `bigint`/`number` split,
    // but arithmetic never did.
    let where_pred = f.requires.iter().rev().fold(None, |acc: Option<Expr>, c| {
        Some(match acc {
            None => c.predicate.clone(),
            Some(rest) => Expr {
                id: ExprId::SYNTHETIC,
                kind: ExprKind::BinOp(BinOp::And, Box::new(c.predicate.clone()), Box::new(rest)),
                span: f.span,
            },
        })
    });
    if let Some(w) = where_pred {
        let mut typed = synthetic_typed_commons_for_target(
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            hidden,
            tys,
        );
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let synth = Block {
            statements: Vec::new(),
            tail: Box::new(w.clone()),
            span: w.span,
            tail_leading_comments: Vec::new(),
            implicit_tail: false,
        };
        let (src, _) = emitter::lower_block_to_async_body(
            &synth,
            &TypeRef::Base(BaseType::Bool, w.span),
            &mut typed,
            &cross,
            runtime_use,
        );
        // P7.2: deferred — `__vals`'s elements are heterogeneous, one per binding,
        // some internally `bigint`-represented; same reason as `__gens`'s own
        // construction elsewhere in this function.
        out.push_str("    const __where = (__vals: any[]) => {\n");
        out.push_str(&format!("      {destructure}\n"));
        for line in src.lines() {
            out.push_str("      ");
            out.push_str(line);
            out.push('\n');
        }
        out.push_str("    };\n");
    } else {
        let where_null = TsStmt::const_stmt(
            TsBindingName::Ident("__where".to_string()),
            None,
            TsExpr::Lit(TsLit::Null),
            None,
        );
        out.push_str(&bynk_ts::print_stmt(&where_null, 2));
    }

    // `__body` — call the (guarded) function. #1426: every param is already
    // coerced to `number` by `destructure_vals` at the bind site above (an
    // `Int`-drawing local is never left `bigint`), so the call passes each
    // param straight through — no separate per-argument coercion needed here
    // any more (this used to `Number(…)`-wrap `Int` args itself; redundant
    // now that the bind site handles it, and would double-wrap). The guard
    // asserts the `ensures`.
    let call_args: Vec<String> = f.params.iter().map(|p| p.name.name.clone()).collect();
    // P7.2: deferred, same reason as `__where`'s own `__vals` above.
    out.push_str("    const __body = async (__vals: any[]) => {\n");
    out.push_str(&format!("      {destructure}\n"));
    out.push_str(&format!(
        "      {}({});\n",
        fname.name,
        call_args.join(", ")
    ));
    out.push_str("    };\n");

    let rel_path_fwd = rel_path.replace('\\', "/");
    let name = format!("contract {}", fname.name);
    let return_run = TsStmt::return_stmt(
        Some(await_expr(call(
            ident("__bynkRunProperty"),
            vec![TsExpr::object(vec![
                (
                    "seed".to_string(),
                    call(
                        ident("__bynkMix"),
                        vec![ident("__bynkSeed"), num_lit(prop_ordinal.to_string())],
                    ),
                ),
                ("cases".to_string(), num_lit("100")),
                ("gens".to_string(), ident("__gens")),
                ("where".to_string(), ident("__where")),
                ("body".to_string(), ident("__body")),
                ("name".to_string(), str_lit(name)),
                ("location".to_string(), str_lit(rel_path_fwd.clone())),
                ("file".to_string(), str_lit(rel_path_fwd)),
            ])],
        ))),
        None,
    );
    out.push_str(&bynk_ts::print_stmt(&return_run, 2));
    Some(async_test_runner_fn(runner_name, out, None))
}

// -- Small tree-construction helpers (#1325) ------------------------------
//
// Mirrors `workers.rs`'s/`workers_entry.rs`'s own local helper sets (#1321/
// #1323) — this file's own private set, not shared, matching this track's
// own established per-file scoping.
