//! `stub` clause emission: stub classes, platform doubles and overlays.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use bynk_emit::emitter::RuntimeUse;
// #1759: every namespace a test module imports, from one definition.
use bynk_check::checker::Types;
use bynk_check::d1_migrations::MigrationFile;
use bynk_check::test_suites::{self, ResolvedStub, StubScope};
use bynk_emit::emitter::emit::test_scaffold_ns as test_ns;
use bynk_syntax::ast::{
    ArgPattern, BaseType, CapabilityOp, Commons, CommonsForm, CommonsItem, Expr, Ident, Param,
    QualifiedName, SeqOutcome, StubClause, StubRhs, Trivia, TypeRef,
};
use bynk_syntax::span::Span;
use bynk_ts::{
    TsBinaryOp, TsBindingName, TsClassMethod, TsExpr, TsParam, TsStmt, TsSwitchCase, TsType,
};
use std::sync::Arc;

/// v0.118: emit the `__Stub_<Cap>` stub class for a capability seam
/// overridden by `stub` clauses (testing track slice 6). One async method
/// per overridden operation renders its clauses as a first-match-wins if-chain
/// over the call's argument patterns; a matched clause returns its lowered
/// value, throws an injected fault, or advances a per-call sequence cursor.
///
/// #1483: returns one real [`TsStmt`] (was `String`) — a `Raw` carrier for
/// the whole class, the same Decision-C treatment `emit_provider` (#1480)
/// already established: the class wrapper (header/fields) stays hand-written
/// text (cited in place below), each method printed as its own real
/// `TsClassMethod` fragment and spliced directly in. No `nested_map` — this
/// function has no `LowerCtx`/source-map involvement anywhere in its own
/// construction (`lower_stub_value_block`/`emit_stub_rhs` build already-
/// formed text, not through a body-local `SourceMapBuilder`), so there is
/// nothing to merge, unlike `emit_provider`/`emit_service`/`emit_agent`'s own
/// per-handler bodies.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_stub_class(
    rp: &ResolvedStub,
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    runtime_use: &RuntimeUse,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> TsStmt {
    let mut out = String::new();
    let cap = &rp.cap;
    // Value expressions are lowered in the target context's privileged view, so
    // its types, variants and `uses` vocabulary resolve unqualified.
    let owning_unit = target_name.to_string();
    let scope_ns = test_ns(&owning_unit);
    // Each in-scope type name's *owning* namespace — the target's own for a
    // locally-declared type, but the specific `uses`d commons' for one reached
    // only through it (never the target's), since `emit_context_rebrands`
    // only re-exports a `uses`-sourced type under the target's namespace when
    // the target's own lowered body references it by name — a stub class
    // implementing an adapter-flattened capability (e.g. `Locale`) generally
    // doesn't (Locale capability track, slice 1, #844).
    let mut type_ns: HashMap<String, String> = unit_tables
        .get(&owning_unit)
        .map(|t| {
            t.types
                .keys()
                .map(|n| (n.clone(), scope_ns.clone()))
                .collect()
        })
        .unwrap_or_default();
    if let Some(used) = unit_uses.get(&owning_unit) {
        for u in used {
            if let Some(table) = unit_tables.get(u) {
                let uns = test_ns(u);
                // #2702: a used commons' `private` type is not in scope.
                for n in public_types(table) {
                    type_ns.entry(n.clone()).or_insert_with(|| uns.clone());
                }
            }
        }
    }
    // #291: a consumed adapter's types (`bynk`'s `Response`, `FetchError`) are
    // what a platform capability's stub takes and returns.
    let adapters: Vec<&String> = unit_consumes
        .get(&owning_unit)
        .into_iter()
        .flatten()
        .filter(|u| unit_tables.get(*u).and_then(|t| t.kind) == Some(UnitKind::Adapter))
        .collect();
    for u in &adapters {
        let uns = test_ns(u);
        for n in unit_tables[*u].types.keys() {
            type_ns.entry(n.clone()).or_insert_with(|| uns.clone());
        }
    }
    // #2887: the stubbed capability's signature names its own unit's types by
    // the names that unit declares, which this target may have renamed
    // (`consumes U { T as Alias }`) to keep them off a type of its own: those
    // names are `U`'s whatever else the target binds them to.
    let mut sig_type_ns = type_ns.clone();
    if let Some(owner) = unit_tables
        .get(&owning_unit)
        .and_then(|t| t.flattened_caps.get(&rp.cap))
    {
        for a in unit_tables[&owning_unit]
            .consumed_type_aliases
            .values()
            .filter(|a| a.target == *owner)
        {
            sig_type_ns.insert(a.original.clone(), test_ns(owner));
        }
        // #2994: the signature's other names are the declaring unit's own, or a
        // commons it `uses` (`bynk`'s `Locale.current` returns the `uses`d
        // `bynk.locale.types`' `LocaleTag`) — whether or not the target uses that
        // commons. Qualify them by the namespace that declares them, over
        // the target's own view, which is not the one the signature was written
        // in. The unit's own types win over a used commons', as they do there.
        let mut declared: HashMap<String, String> = HashMap::new();
        for used in unit_uses.get(owner).into_iter().flatten() {
            if let Some(table) = unit_tables.get(used) {
                for n in public_types(table) {
                    declared.entry(n.clone()).or_insert_with(|| test_ns(used));
                }
            }
        }
        if let Some(table) = unit_tables.get(owner) {
            for n in table.types.keys() {
                declared.insert(n.clone(), test_ns(owner));
            }
        }
        for (n, ns) in declared {
            sig_type_ns.insert(n, ns);
        }
    }
    let scope_names: Vec<String> = if let Some(table) = unit_tables.get(&owning_unit) {
        let mut v: Vec<String> = table
            .types
            .keys()
            .chain(table.fns.keys())
            .cloned()
            .collect();
        v.sort();
        v.dedup();
        v
    } else {
        Vec::new()
    };
    let scope_type_names: Vec<(String, Vec<String>)> = unit_tables
        .get(&owning_unit)
        .map(|t| aliased_types(t, |_| true))
        .unwrap_or_default();

    // Group clause indices by method, preserving resolution order (case-scoped
    // clauses precede suite-scoped ones, so they win the first-match chain).
    let mut by_method: std::collections::BTreeMap<String, Vec<usize>> =
        std::collections::BTreeMap::new();
    for (idx, clause) in rp.clauses.iter().enumerate() {
        by_method
            .entry(clause.method.name.clone())
            .or_default()
            .push(idx);
    }

    // The class wrapper (header/fields) stays hand-written text — Decision C
    // (#1359's own `emit_provider` precedent): each method's own body is one
    // opaque `TsStmt::raw` (it mixes real structure with `lower_stub_value_
    // block`'s own already-lowered opaque text, an `async () => { ... }`
    // IIFE whose own multi-statement block body no `TsArrowBody::Block`
    // renderer can flatten onto one line — the same "block-bodied arrow
    // stays opaque" call `emit_composition_root`'s own `__eventsDispatch`
    // closure already made), so each method is printed as
    // its own real `TsClassMethod` fragment via `bynk_ts::print_class_method`
    // at depth 0 — the only depth `render_class_method`'s own debug_assert
    // allows a `Raw`-bodied method to print correctly at — and spliced
    // directly into this still-hand-written wrapper, with no blank line
    // between fields/methods or between methods (matching this function's
    // own zero-blank-line output exactly).
    out.push_str(&format!("class __Stub_{cap} {{\n"));
    // One per-call sequence cursor field per `returns each` clause.
    for (idx, clause) in rp.clauses.iter().enumerate() {
        if matches!(clause.rhs, StubRhs::ReturnsEach(..)) {
            out.push_str(&format!("  __seq_{idx} = 0;\n"));
        }
    }
    // #291: a case-scoped clause applies only while its own case runs, so a
    // class with one carries the running case's name (`__makeTestDeps`'s
    // argument), and `__applies` tells `__bynkOverlay` which operations this
    // case stubs at all — an operation stubbed only by other cases reaches the
    // tier default instead. #1860: a suite-scoped clause, when another suite
    // shares the module, applies only while its own suite's runs do, so the
    // class also carries the running suite's ordinal.
    let case_scoped = stub_is_scoped(rp);
    if case_scoped {
        out.push_str("  __case: string | undefined;\n");
        if stub_is_suite_scoped(rp) {
            out.push_str("  __suite: number | undefined;\n");
            out.push_str(
                "  constructor(c?: string, s?: number) {\n    this.__case = c;\n    this.__suite = s;\n  }\n",
            );
        } else {
            out.push_str("  constructor(c?: string) {\n    this.__case = c;\n  }\n");
        }
        out.push_str("  __applies(op: string): boolean {\n");
        for (method, clause_idxs) in &by_method {
            let scopes: Vec<&StubScope> =
                clause_idxs.iter().map(|&i| &rp.clause_scopes[i]).collect();
            // An operation with any clause for every run is always stubbed.
            if scopes.contains(&&StubScope::Every) {
                continue;
            }
            out.push_str(&format!(
                "    if (op === {}) return {};\n",
                bynk_ts::print_expr(&str_lit(method.as_str())),
                scopes_test(&scopes),
            ));
        }
        out.push_str("    return true;\n  }\n");
    }

    for (method, clause_idxs) in &by_method {
        let Some(op) = rp.cap_decl.ops.iter().find(|o| &o.name.name == method) else {
            continue;
        };
        let params: Vec<TsParam> = op
            .params
            .iter()
            .map(|p| TsParam {
                name: p.name.name.clone(),
                ty: Some(emitter::ts_type_ref_qualified_multi_ts_type(
                    &p.type_ref,
                    &sig_type_ns,
                )),
                optional: false,
            })
            .collect();
        let return_ty = emitter::ts_type_ref_qualified_multi_ts_type(&op.return_type, &sig_type_ns);

        let mut body_text = String::new();
        bynk_emit::emitter::extend_printed_at(
            &mut body_text,
            emit_ns_destructure(&scope_ns, &scope_names, &scope_type_names),
            2,
        );
        let mut taken: HashSet<String> = scope_names.iter().cloned().collect();
        // #2947: a suite helper inlined ahead of a value (#2907) spells its
        // signature's types unqualified, as in a case runner, so a method with
        // one binds the same `uses` commons' names `emit_test_scope_setup`
        // does (assets and embeds included). A method without one is unchanged.
        if clause_idxs.iter().any(|&i| !rp.clause_fns[i].is_empty()) {
            for (ns, names, types) in
                used_commons_destructures(&owning_unit, unit_tables, unit_uses, &mut HashSet::new())
            {
                taken.extend(names.iter().chain(types.iter().map(|(n, _)| n)).cloned());
                bynk_emit::emitter::extend_printed_at(
                    &mut body_text,
                    emit_ns_destructure(&ns, &names, &types),
                    2,
                );
            }
        }
        for u in &adapters {
            // #2887: a type this target renames from `u` binds as its alias.
            let renames: HashMap<String, String> = unit_tables
                .get(&owning_unit)
                .into_iter()
                .flat_map(|t| &t.consumed_type_aliases)
                .filter(|(_, a)| a.target == **u)
                .map(|(alias, a)| (a.original.clone(), alias.clone()))
                .collect();
            let mut names: Vec<String> = unit_tables[*u]
                .types
                .keys()
                .filter(|n| taken.insert(renames.get(*n).unwrap_or(*n).clone()))
                .cloned()
                .collect();
            if names.is_empty() {
                continue;
            }
            names.sort();
            let types = aliased_types(&unit_tables[*u], |n| names.contains(n));
            let declared: HashSet<String> = renames
                .keys()
                .filter(|o| names.contains(*o) && taken.insert((*o).clone()))
                .cloned()
                .collect();
            bynk_emit::emitter::extend_printed_at(
                &mut body_text,
                emit_ns_destructure_renamed(&test_ns(u), &names, &types, &renames, &declared),
                2,
            );
        }
        for &idx in clause_idxs {
            let clause = &rp.clauses[idx];
            // Argument-pattern consts: a `Value(e)` pattern lowers to a const the
            // condition compares structurally; an `Any` pattern contributes none.
            let mut cond_parts: Vec<String> = Vec::new();
            for (i, pat) in clause.args.iter().enumerate() {
                if let ArgPattern::Value(e) = pat
                    && let Some(param) = op.params.get(i)
                {
                    let body = lower_stub_value_block(
                        e,
                        &param.type_ref,
                        &[],
                        clause_items(rp, idx),
                        target_name,
                        unit_tables,
                        unit_uses,
                        unit_consumes,
                        unit_consumes_aliases,
                        runtime_use,
                        hidden,
                        tys,
                    );
                    let vname = format!("__pv_{idx}_{i}");
                    body_text.push_str(&format!("    const {vname} = await (async () => {{\n"));
                    for line in body.lines() {
                        body_text.push_str("      ");
                        body_text.push_str(line);
                        body_text.push('\n');
                    }
                    body_text.push_str("    })();\n");
                    runtime_use.note_eq();
                    cond_parts.push(format!("__bynkEq({}, {vname})", param.name.name));
                }
            }
            let scope = &rp.clause_scopes[idx];
            if *scope != StubScope::Every {
                cond_parts.insert(0, scopes_test(&[scope]));
            }
            let cond = if cond_parts.is_empty() {
                "true".to_string()
            } else {
                cond_parts.join(" && ")
            };
            body_text.push_str(&format!("    if ({cond}) {{\n"));
            let rhs_body = emit_stub_rhs(
                clause,
                idx,
                op,
                clause_items(rp, idx),
                target_name,
                unit_tables,
                unit_uses,
                unit_consumes,
                unit_consumes_aliases,
                runtime_use,
                hidden,
                tys,
            );
            for line in rhs_body.lines() {
                body_text.push_str("      ");
                body_text.push_str(line);
                body_text.push('\n');
            }
            body_text.push_str("    }\n");
        }
        body_text.push_str(&format!(
            "    throw new globalThis.Error(\"bynk: no stub clause matched for {cap}.{method}\");\n"
        ));

        let method_node = TsClassMethod {
            name: method.clone(),
            private: false,
            is_async: true,
            params,
            return_type: Some(return_ty),
            doc: None,
            body: vec![TsStmt::raw(body_text, None)],
        };
        out.push_str(&bynk_ts::print_class_method(&method_node, 0));
    }
    out.push_str("}\n");
    TsStmt::raw(out, None)
}

/// `throw new Error("bynk: injected capability fault (stubs … fails)");` —
/// the one real site anywhere in this cluster injecting a synthetic
/// capability failure, shared by [`emit_stub_rhs`]'s own `Fails`/
/// `ReturnsEach`-with-a-failing-outcome arms.
pub(super) fn stub_fault_stmt() -> TsStmt {
    TsStmt::throw_stmt(
        TsExpr::New {
            callee: Box::new(ident("globalThis.Error")),
            args: vec![str_lit("bynk: injected capability fault (stubs … fails)")],
        },
        None,
    )
}

/// Prefix every line of `text` with `prefix` — [`TsStmtKind::Raw`]'s own
/// "pre-indented at a fixed absolute depth by the caller" contract (its own
/// doc), used here to carry [`lower_stub_value_block`]'s already-lowered
/// opaque body text as one switch-case's own statement.
pub(super) fn reindent_by(text: &str, prefix: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        out.push_str(prefix);
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// v0.118: render the body of one matched `stub` clause — a `returns` value,
/// a `fails` fault, or a `returns each` per-call sequence (last outcome repeats).
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_stub_rhs(
    clause: &StubClause,
    clause_idx: usize,
    op: &CapabilityOp,
    fixtures: test_suites::SuiteItems<'_>,
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    runtime_use: &RuntimeUse,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> String {
    let lower = |e: &Expr| {
        lower_stub_value_block(
            e,
            &op.return_type,
            &op.params,
            fixtures,
            target_name,
            unit_tables,
            unit_uses,
            unit_consumes,
            unit_consumes_aliases,
            runtime_use,
            hidden,
            tys,
        )
    };
    match &clause.rhs {
        StubRhs::Returns(e) => match &clause.after {
            // #2683: `returns <v> after <d>` — the call completes only once the
            // harness clock has reached `d` from the call.
            Some(after) => {
                let ms = lower_stub_value_block(
                    after,
                    &TypeRef::Base(BaseType::Duration, after.span),
                    &[],
                    fixtures,
                    target_name,
                    unit_tables,
                    unit_uses,
                    unit_consumes,
                    unit_consumes_aliases,
                    runtime_use,
                    hidden,
                    tys,
                );
                let mut out = format!("const __after_{clause_idx} = await (async () => {{\n");
                out.push_str(&reindent_by(&ms, "  "));
                out.push_str("})();\n");
                out.push_str(&format!("await __bynkSleep(__after_{clause_idx});\n"));
                out.push_str(&lower(e));
                out
            }
            None => lower(e),
        },
        // #2683: `never` — the call stays in flight for ever.
        StubRhs::Never(_) => "return await __bynkNever();\n".to_string(),
        StubRhs::Fails(_) => bynk_ts::print_stmt(&stub_fault_stmt(), 0),
        StubRhs::ReturnsEach(outcomes, _) => {
            let n = outcomes.len();
            let mut stmts = vec![TsStmt::const_stmt(
                TsBindingName::Ident("__k".to_string()),
                None,
                member(ident("this"), format!("__seq_{clause_idx}")),
                None,
            )];
            if n > 1 {
                stmts.push(if_(
                    TsExpr::Binary {
                        op: TsBinaryOp::LessThan,
                        left: Box::new(member(ident("this"), format!("__seq_{clause_idx}"))),
                        right: Box::new(num_lit((n - 1).to_string())),
                    },
                    TsStmt::increment(member(ident("this"), format!("__seq_{clause_idx}")), None),
                ));
            }
            let outcome_body = |o: &SeqOutcome| match o {
                SeqOutcome::Value(e) => lower(e),
                SeqOutcome::Fails(_) => bynk_ts::print_stmt(&stub_fault_stmt(), 0),
            };
            let mut cases: Vec<TsSwitchCase> = outcomes
                .iter()
                .enumerate()
                .take(n.saturating_sub(1))
                .map(|(j, o)| TsSwitchCase {
                    test: Some(num_lit(j.to_string())),
                    body: vec![TsStmt::raw(reindent_by(&outcome_body(o), "    "), None)],
                    default_braced: false,
                    // Arc E slice 6 (#1445): unaffected by `case_braced`'s
                    // addition — this site's own non-`default` cases were
                    // already, and stay, `{ }`-blocked.
                    case_braced: true,
                })
                .collect();
            cases.push(TsSwitchCase {
                test: None,
                body: vec![TsStmt::raw(
                    reindent_by(&outcome_body(&outcomes[n - 1]), "    "),
                    None,
                )],
                default_braced: true,
                case_braced: true,
            });
            stmts.push(TsStmt::switch_stmt(ident("__k"), cases, None));
            stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect()
        }
    }
}

/// v0.118: lower a single `stub` value expression as if it were a provider
/// operation's tail — type-check it in the target's privileged view (so variant
/// constructors and `uses` names resolve) then lower it to an async body ending
/// in `return <value>;`. Mirrors the retired mock-op-body lowering.
#[allow(clippy::too_many_arguments)]
pub(super) fn lower_stub_value_block(
    e: &Expr,
    ret_type: &TypeRef,
    params: &[Param],
    items: test_suites::SuiteItems<'_>,
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    runtime_use: &RuntimeUse,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<Types>,
) -> String {
    // #2850: the declaring suite's fixtures are names for module-level
    // constants. An operation parameter of the same name shadows one, as in the
    // checker.
    let fixtures: Vec<bynk_syntax::ast::FixtureDecl> = items
        .fixtures
        .iter()
        .filter(|f| !params.iter().any(|p| p.name.name == f.name.name))
        .cloned()
        .collect();
    let roots_mark = runtime_use.json_codec_roots_len();
    let owning_unit = target_name.to_string();
    let mut typed = synthetic_typed_commons_for_target(
        &owning_unit,
        unit_tables,
        unit_uses,
        unit_consumes,
        hidden,
        tys,
    );
    let block = test_suites::value_block(e);
    let mut helpers: Vec<String> = Vec::new();
    let mut suite_decls = String::new();
    if let Some((resolved, _)) = test_suites::build_privileged_resolved(
        &owning_unit,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) {
        let mut resolved = resolved;
        test_suites::add_suite_fns(
            &mut resolved,
            test_suites::SuiteItems {
                fns: &[],
                fixtures: &fixtures,
                types: items.types,
                consts: items.consts,
            },
        );
        // #2907: and its helpers, declared ahead of the value like the fixtures.
        let mut errs: Vec<CompileError> = Vec::new();
        if !items.fns.is_empty() {
            let (expr_types, callees) = test_suites::check_suite_fns(
                items.fns,
                &mut resolved,
                unit_tables.get(&owning_unit),
                &mut errs,
                &mut RefSink::new(),
                &mut LocalsSink::new(),
                tys,
            );
            typed.expr_types.extend(expr_types);
            typed.callees.extend(callees);
            test_suites::add_suite_fns_to_typed(&mut typed, items.fns);
            for f in items.fns {
                let stmts = emitter::emit_suite_fn(f, &typed, runtime_use);
                helpers.extend(stmts.iter().map(|st| bynk_ts::print_stmt(st, 0)));
            }
        }
        // #3111: the suite's own types and constants, declared ahead of the
        // helpers and the value like a case runner's (#3066).
        if !items.types.is_empty() || !items.consts.is_empty() {
            let (decl_types, decl_callees) = checker::check_suite_decls(
                items.types,
                items.consts,
                &resolved,
                tys,
                &mut errs,
                &mut RefSink::new(),
                &mut LocalsSink::new(),
            );
            typed.expr_types.extend(decl_types);
            typed.callees.extend(decl_callees);
            for t in items.types {
                typed.types.insert(t.name.name.clone(), Arc::new(t.clone()));
            }
            for c in items.consts {
                typed.consts.insert(c.name.name.clone(), c.clone());
            }
            suite_decls = emitter::emit_suite_decls(
                items.types,
                items.consts,
                &typed,
                &runtime_use.json_codec_qual(),
                runtime_use,
            );
        }
        test_suites::add_suite_fns(
            &mut resolved,
            test_suites::SuiteItems {
                fns: items.fns,
                fixtures: &[],
                types: &[],
                consts: &[],
            },
        );
        // Build-mode re-check for the lowering's expr types; the analyse exit has
        // already passed, so nothing records (fresh sink).
        checker::check_handler_body(
            &resolved,
            checker::HandlerBodyCheck::new(&block, ret_type, params, &[]),
            checker::CheckSinks {
                tys,
                expr_types: &mut typed.expr_types,
                errors: &mut errs,
                refs: &mut RefSink::new(),
                hints: &mut HintSink::new(),
                locals: &mut LocalsSink::new(),
                requirements: &mut RequirementSink::new(),
                callees: &mut typed.callees,
            },
        );
    }
    let cross = bynk_check::resolver::CrossContextInfo::default();
    // v0.70: `stub` value scaffolding is not user test logic, so its source
    // map is discarded — it stays unmapped (a deliberate scope cut).
    let body =
        emitter::lower_block_to_async_body(&block, ret_type, &mut typed, &cross, runtime_use).0;
    // #3111: the codecs the value reached for beyond the always-present ones.
    let late_codecs = suite_late_codecs(items, &typed, runtime_use, roots_mark);
    let aliases: String = fixtures
        .iter()
        .map(|f| {
            format!(
                "const {}: string = {};\n",
                f.name.name,
                fixture_ident(&f.path)
            )
        })
        .collect();
    format!(
        "{suite_decls}{late_codecs}{aliases}{}{body}",
        helpers.concat()
    )
}

/// #2850/#2907: what a stub clause's values may name of its suite.
pub(super) fn clause_items(
    rp: &test_suites::ResolvedStub,
    idx: usize,
) -> test_suites::SuiteItems<'_> {
    test_suites::SuiteItems {
        fns: &rp.clause_fns[idx],
        fixtures: &rp.clause_fixtures[idx],
        types: &rp.clause_types[idx],
        consts: &rp.clause_consts[idx],
    }
}

pub(super) fn synthetic_typed_commons_for_target(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<checker::Types>,
) -> checker::TypedCommons {
    let table = unit_tables.get(target_name).cloned().unwrap_or_default();
    let table_plan = Arc::clone(&table.exec_plan);
    let mut types = table.types;
    let mut fns = table.fns;
    let mut methods = table.methods;
    // v0.117: carry the target's capability declarations into the synthetic
    // commons items so observation lowering (`with` param destructure,
    // `trace(Cap.op)` record fields) can look up each op's parameter names.
    // #291: a flattened platform capability (`bynk`'s `Fetch`, `Logger`, …) is
    // a seam too, and a consumed adapter's types (`Response`, `FetchError`) are
    // the values a stub returns and a case names — so both come in, the
    // target's own winning a name clash.
    let mut caps: std::collections::BTreeMap<String, bynk_syntax::ast::CapabilityDecl> = table
        .capabilities
        .iter()
        .map(|(n, c)| (n.clone(), c.clone()))
        .collect();
    // #2062: a consumed *context*'s types too. The scope setup destructures
    // them (`emit_test_scope_setup`), and the lowering qualifies a bare
    // nullary variant (`Domestic` → `Region.Domestic`) only when its sum is
    // here, as a handler body's lowering does from the context's own view.
    // #2702: only the public ones, the filter the codec qualifier uses.
    for unit in unit_consumes.get(target_name).into_iter().flatten() {
        if let Some(t) = unit_tables.get(unit) {
            for (n, d) in &t.types {
                if d.private.is_none() {
                    types.entry(n.clone()).or_insert_with(|| d.clone());
                }
            }
        }
    }
    for (n, c, _) in platform_seams(target_name, unit_tables) {
        caps.entry(n.clone()).or_insert_with(|| c.clone());
    }
    let capability_items: Vec<CommonsItem> =
        caps.into_values().map(CommonsItem::Capability).collect();
    // Pull in names that come into scope via the target's `uses` clauses, so
    // the test-body lowering's static-call check (`<Type>.of(...)` etc.)
    // resolves against the same set of names the source can mention.
    if let Some(used) = unit_uses.get(target_name) {
        for u in used {
            if let Some(t) = unit_tables.get(u) {
                // #2702: a used commons' `private` declarations are not in the
                // case's scope, so they must not claim a name (and with it the
                // codec, the methods or the call a visible one resolves to).
                for (n, d) in &t.types {
                    if d.private.is_none() {
                        types.entry(n.clone()).or_insert_with(|| d.clone());
                    }
                }
                for (n, f) in &t.fns {
                    if f.private.is_none() {
                        fns.entry(n.clone()).or_insert_with(|| f.clone());
                    }
                }
                for (n, mt) in &t.methods {
                    if t.types.get(n).is_some_and(|d| d.private.is_some()) {
                        continue;
                    }
                    let entry = methods.entry(n.clone()).or_default();
                    for (m, decl) in &mt.instance {
                        entry
                            .instance
                            .entry(m.clone())
                            .or_insert_with(|| decl.clone());
                    }
                    for (m, decl) in &mt.statics {
                        entry
                            .statics
                            .entry(m.clone())
                            .or_insert_with(|| decl.clone());
                    }
                }
            }
        }
    }
    // #1814: and the types the target's privileged view reaches (`hidden`,
    // each with its owning commons), which the case was checked against,
    // with their methods.
    for (name, owner) in hidden {
        let Some(owner_table) = unit_tables.get(owner) else {
            continue;
        };
        if let Some(d) = owner_table.types.get(name) {
            types.entry(name.clone()).or_insert_with(|| d.clone());
        }
        if let Some(mt) = owner_table.methods.get(name) {
            methods.entry(name.clone()).or_insert_with(|| mt.clone());
        }
    }
    checker::TypedCommons {
        commons: Arc::new(Commons {
            name: QualifiedName {
                parts: target_name
                    .split('.')
                    .map(|p| Ident {
                        name: p.to_string(),
                        span: Span::default(),
                    })
                    .collect(),
                span: Span::default(),
            },
            items: capability_items,
            uses: Vec::new(),
            documentation: None,
            form: CommonsForm::Brace,
            span: Span::default(),
            trivia: Trivia::default(),
            trailing_comments: Vec::new(),
        }),
        types,
        fns,
        methods,
        expr_types: HashMap::new(),
        callees: HashMap::new(),
        warnings: vec![],
        // T3.6b (R4.1): the *caller's* table, not a fresh one. A synthesised
        // commons starts with an empty `expr_types`, but the case/property
        // checks below fill it in — interning into the table they were handed
        // — and the lowering then resolves those ids back through this field.
        // A table of its own would make every one of those reads a
        // wrong-table lookup.
        ty_intern: Arc::clone(tys),
        actor_bindings: HashMap::new(),
        assets: HashMap::new(),
        embeds: HashMap::new(),
        consts: HashMap::new(),
        shadowed_imports: HashMap::new(),
        extensible_enums: Default::default(),
        stability: Default::default(),
        exec_plan: table_plan,
    }
}

/// `undefined as unknown as <ty>` — the placeholder value for an un-stubbed
/// capability/surface entry, shared by every branch below.
pub(super) fn undefined_as_unknown_as(ty: impl Into<String>) -> TsExpr {
    TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(ident("undefined")),
            ty: TsType::named("unknown"),
        }),
        ty: TsType::named(ty),
    }
}

/// #291: the platform capabilities `target` flattens in from a consumed adapter
/// (`consumes bynk { Logger }`) — seams of the unit under test, observable and
/// stubbable like its own — as `(capability, declaration, owning adapter)`.
pub(super) fn platform_seams<'a>(
    target: &str,
    unit_tables: &'a HashMap<String, UnitTable>,
) -> Vec<(&'a String, &'a bynk_syntax::ast::CapabilityDecl, &'a String)> {
    let mut out: Vec<_> = unit_tables
        .get(target)
        .into_iter()
        .flat_map(|t| t.flattened_caps.iter())
        .filter_map(|(cap, owner)| {
            let t = unit_tables.get(owner)?;
            if t.kind != Some(UnitKind::Adapter) {
                return None;
            }
            Some((cap, t.capabilities.get(cap)?, owner))
        })
        .collect();
    out.sort_by_key(|(cap, _, _)| cap.as_str());
    out
}

/// #291: does any of `rp`'s clauses apply to some runs only? Its stub class
/// then carries the running case's name, and the deps factory passes it.
pub(super) fn stub_is_scoped(rp: &ResolvedStub) -> bool {
    rp.clause_scopes.iter().any(|s| *s != StubScope::Every)
}

/// #1860: does any of `rp`'s clauses apply to one suite's runs? Its stub
/// class then also carries the running suite's ordinal.
pub(super) fn stub_is_suite_scoped(rp: &ResolvedStub) -> bool {
    rp.clause_scopes
        .iter()
        .any(|s| matches!(s, StubScope::Suite(_)))
}

/// #291/#1860: a stub class's test that the running case or suite is one of
/// `scopes` (none of them [`StubScope::Every`]): `this.__case === "a" ||
/// this.__suite === 1`, cases in name order and then suites.
pub(super) fn scopes_test(scopes: &[&StubScope]) -> String {
    let mut cases: BTreeSet<&String> = BTreeSet::new();
    let mut suites: BTreeSet<usize> = BTreeSet::new();
    for scope in scopes {
        match scope {
            StubScope::Case(c) => {
                cases.insert(c);
            }
            StubScope::Suite(n) => {
                suites.insert(*n);
            }
            StubScope::Every => {}
        }
    }
    cases
        .into_iter()
        .map(|c| {
            format!(
                "this.__case === {}",
                bynk_ts::print_expr(&str_lit(c.as_str()))
            )
        })
        .chain(suites.into_iter().map(|n| format!("this.__suite === {n}")))
        .collect::<Vec<_>>()
        .join(" || ")
}

/// #291: a stubbed capability's `__makeTestDeps` entry — its `__Stub_<Cap>`
/// layered over `base`, the tier default (the context's own provider, a
/// platform capability's test double, or nothing). A stubbed operation answers
/// from the stub; every other operation reaches the base, so `stub` overrides
/// one operation and leaves the rest of the seam working (case `stub` > suite
/// `stub` > the tier default).
pub(super) fn stub_overlay(rp: &ResolvedStub, ty: &str, base: Option<TsExpr>) -> TsExpr {
    let cap = &rp.cap;
    // A class with case-scoped clauses is told which case is running, and one
    // with suite-scoped clauses which suite (#1860).
    let mut args = Vec::new();
    if stub_is_scoped(rp) {
        args.push(ident("__case"));
    }
    if stub_is_suite_scoped(rp) {
        args.push(ident("__suite"));
    }
    TsExpr::As {
        expr: Box::new(call(
            ident("__bynkOverlay"),
            vec![
                base.unwrap_or_else(|| ident("undefined")),
                TsExpr::New {
                    callee: Box::new(ident(format!("__Stub_{cap}"))),
                    args,
                },
                str_lit(cap.as_str()),
            ],
        )),
        ty: TsType::named(ty),
    }
}

/// #291: `__bynkOverlay`, emitted into a test module that has a `stub`.
pub(super) const STUB_OVERLAY_TS: &str = "\
function __bynkOverlay(base: unknown, stub: object, cap: string): unknown {
  return new globalThis.Proxy(stub, {
    get(target, prop) {
      const t = target as Record<PropertyKey, unknown> & { __applies?: (op: string) => boolean };
      // An own property (the stub's state, or a wrapper the recording proxy
      // installed) always answers; a stubbed operation answers when it applies
      // to the running case.
      if (globalThis.Object.prototype.hasOwnProperty.call(t, prop)) return t[prop];
      if (prop in t && (t.__applies === undefined || typeof prop !== \"string\" || t.__applies(prop))) return t[prop];
      if (base !== undefined) {
        const v = (base as Record<PropertyKey, unknown>)[prop];
        return typeof v === \"function\" ? v.bind(base) : v;
      }
      return () => {
        throw new globalThis.Error(`bynk: ${cap}.${String(prop)} is not stubbed, and the capability has no provider in this test`);
      };
    },
  });
}
";

/// P1 (#2590): the body of `bynk.cloudflare`'s `Kv` unit-tier double
/// ([`platform_double`]). Expiry is kept and listed, not enforced, as on the
/// integration tier, on the clock `Clock.now()` reads (`__NOW__`, substituted
/// as `RateLimit`'s is).
pub(super) const KV_DOUBLE_BODY: &str = r#"  type Entry = { bytes: Uint8Array; metadata: globalThis.Map<string, string>; expiration: number | undefined };
  type Expiration = { tag: string; ttl?: number; instant?: number };
  const store = new globalThis.Map<string, Entry>();
  const enc = new globalThis.TextEncoder();
  const dec = new globalThis.TextDecoder();
  const fault = (why: string): never => {
    throw new globalThis.Error(`bynk.cloudflare.Kv: ${why}`);
  };
  const checkKey = (key: string) => {
    const n = enc.encode(key).length;
    if (n > 512) fault(`a key is at most 512 bytes, not ${n}`);
  };
  const expiryOf = (e: Expiration): number | undefined => {
    if (e.tag === "After") {
      const ttl = e.ttl ?? 0;
      if (globalThis.Math.ceil(ttl / 1000) < 60) fault(`an expiry is at least 60 seconds away, not ${ttl} ms`);
      return __NOW__ + globalThis.Math.ceil(ttl / 1000) * 1000;
    }
    if (e.tag === "At") {
      const instant = e.instant ?? 0;
      const away = globalThis.Math.ceil(instant / 1000) - __NOW__ / 1000;
      if (away < 60) fault(`an expiry is at least 60 seconds away, not ${globalThis.Math.floor(away)} s`);
      return globalThis.Math.ceil(instant / 1000) * 1000;
    }
    return undefined;
  };
  const write = (key: string, bytes: Uint8Array, options: { expiration: Expiration; metadata: ReadonlyMap<string, string> }) => {
    checkKey(key);
    const size = enc.encode(globalThis.JSON.stringify(globalThis.Object.fromEntries(options.metadata))).length;
    if (size > 1024) fault(`metadata is at most 1024 bytes as JSON, not ${size}`);
    store.set(key, { bytes, metadata: new globalThis.Map(options.metadata), expiration: expiryOf(options.expiration) });
  };
  const plain = (bytes: Uint8Array): Entry => ({ bytes, metadata: new globalThis.Map<string, string>(), expiration: undefined });
  return {
    async get(key) {
      const e = store.get(key);
      return e === undefined ? None : Some(dec.decode(e.bytes));
    },
    async put(key, value) {
      store.set(key, plain(enc.encode(value)));
    },
    // Unchecked, as `putTtl` always was; its expiry is kept and listed, as
    // the platform and the integration tier do.
    async putTtl(key, value, ttlSeconds) {
      store.set(key, { ...plain(enc.encode(value)), expiration: __NOW__ + ttlSeconds * 1000 });
    },
    async delete(key) {
      store.delete(key);
    },
    async list(prefix) {
      return [...store.keys()].filter((k) => prefix.tag === "None" || k.startsWith(prefix.value)).sort();
    },
    async getWith(key, options) {
      checkKey(key);
      if (options.cacheTtl.tag === "Some" && globalThis.Math.ceil(options.cacheTtl.value / 1000) < 30) {
        fault(`a cacheTtl is at least 30 seconds, not ${options.cacheTtl.value} ms`);
      }
      const e = store.get(key);
      return e === undefined ? None : Some({ value: dec.decode(e.bytes), metadata: new globalThis.Map(e.metadata) });
    },
    async putWith(key, value, options) {
      write(key, enc.encode(value), options);
    },
    async getBytes(key) {
      checkKey(key);
      const e = store.get(key);
      return e === undefined ? None : Some(e.bytes.slice());
    },
    async putBytes(key, value, options) {
      write(key, value.slice(), options);
    },
    async listPage(options) {
      const limit = options.limit.tag === "Some" ? options.limit.value : 1000;
      if (!globalThis.Number.isInteger(limit) || limit < 1 || limit > 1000) fault(`a page holds 1 to 1000 keys, not ${limit}`);
      const after = options.cursor.tag === "Some" ? options.cursor.value : undefined;
      const names = [...store.keys()]
        .filter((k) => (options.prefix.tag === "None" || k.startsWith(options.prefix.value)) && (after === undefined || k > after))
        .sort();
      const page = names.slice(0, limit);
      return {
        keys: page.map((name) => {
          const e = store.get(name)!;
          return { name, expiration: e.expiration === undefined ? None : Some(e.expiration), metadata: new globalThis.Map(e.metadata) };
        }),
        cursor: page.length < names.length ? Some(page[page.length - 1]) : None,
      };
    },
  };
"#;

/// P5 (#2323): the body of `bynk.cloudflare`'s `R2` unit-tier double
/// ([`platform_double`]). P5b (#2626) adds the streamed reads and writes and
/// multipart uploads, which check sizes and parts as the integration tier's
/// `makeIntegrationR2` does (the 5 MiB minimum part aside).
pub(super) const R2_DOUBLE_BODY: &str = r#"  const store = new globalThis.Map<string, any>();
  let version = 0;
  const meta = (key: string) => {
    const e = store.get(key);
    return { key, size: e.body.length, etag: e.etag, uploaded: 0, contentType: e.contentType, metadata: e.metadata };
  };
  const write = (key: string, body: Uint8Array, contentType: any, metadata: any) => {
    version += 1;
    store.set(key, { body: body.slice(), etag: `etag-${version}`, contentType, metadata });
    return meta(key);
  };
  type Upload = {
    options: { readonly contentType: Option<string>; readonly metadata: globalThis.ReadonlyMap<string, string> };
    parts: globalThis.Map<number, { etag: string; body: globalThis.Uint8Array }>;
  };
  const uploads = new globalThis.Map<string, Upload>();
  const claim = (stream: object) => {
    const s = stream as { [k: symbol]: boolean };
    const k = globalThis.Symbol.for('bynk.stream.read');
    if (s[k]) throw new globalThis.Error("bynk: a stream can be read only once");
    s[k] = true;
  };
  const collect = async (op: string, body: AsyncIterable<globalThis.Uint8Array>, size: number) => {
    if (!globalThis.Number.isSafeInteger(size) || size < 0) {
      throw new globalThis.Error(`bynk.cloudflare.R2.${op}: size must be a whole number of bytes, not ${size}`);
    }
    claim(body);
    const chunks: globalThis.Uint8Array[] = [];
    let held = 0;
    for await (const c of body) {
      held += c.length;
      if (held > size) break;
      chunks.push(c);
    }
    if (held !== size) {
      throw new globalThis.Error(`bynk.cloudflare.R2.${op}: the stream held ${held > size ? `at least ${held}` : held} bytes, not the declared size ${size}`);
    }
    const out = new globalThis.Uint8Array(size);
    let at = 0;
    for (const c of chunks) {
      out.set(c, at);
      at += c.length;
    }
    return out;
  };
  const open = (u: { key: string; uploadId: string }) => {
    const e = uploads.get(`${u.key}\u0000${u.uploadId}`);
    if (e === undefined) throw new globalThis.Error(`R2: no multipart upload ${u.uploadId} of ${u.key}`);
    return e;
  };
  return {
    async get(key) {
      const e = store.get(key);
      return e === undefined ? None : Some(e.body.slice());
    },
    async head(key) {
      return store.has(key) ? Some(meta(key)) : None;
    },
    async put(key, body) {
      return write(key, body, None, new globalThis.Map());
    },
    async putWith(key, body, options) {
      const c = options.onlyIf;
      if (c.tag === "Absent" && store.has(key)) return None;
      if (c.tag === "EtagMatches" && store.get(key)?.etag !== c.etag) return None;
      return Some(write(key, body, options.contentType, options.metadata));
    },
    async delete(key) {
      store.delete(key);
    },
    async list(prefix, cursor) {
      const keys = [...store.keys()]
        .filter((k) => (prefix.tag === "None" || k.startsWith(prefix.value)) && (cursor.tag === "None" || k > cursor.value))
        .sort();
      const page = keys.slice(0, 1000);
      return { keys: page.map(meta), cursor: keys.length > page.length ? Some(page[page.length - 1]) : None };
    },
    async getStream(key) {
      const e = store.get(key);
      if (e === undefined) return None;
      const bytes = e.body.slice();
      const chunks = async function* () {
        for (let at = 0; at < bytes.length; at += 65536) yield bytes.slice(at, at + 65536);
      };
      return Some({ object: meta(key), body: chunks() });
    },
    async putStream(key, body, size) {
      return write(key, await collect("putStream", body, size), None, new globalThis.Map());
    },
    async putStreamWith(key, body, size, options) {
      const bytes = await collect("putStreamWith", body, size);
      const c = options.onlyIf;
      if (c.tag === "Absent" && store.has(key)) return None;
      if (c.tag === "EtagMatches" && store.get(key)?.etag !== c.etag) return None;
      return Some(write(key, bytes, options.contentType, options.metadata));
    },
    async createUpload(key, options) {
      version += 1;
      const uploadId = `upload-${version}`;
      uploads.set(`${key}\u0000${uploadId}`, { options, parts: new globalThis.Map() });
      return { key, uploadId };
    },
    async uploadPart(upload, partNumber, body) {
      const e = open(upload);
      if (!globalThis.Number.isInteger(partNumber) || partNumber < 1 || partNumber > 10000) {
        throw new globalThis.Error(`R2: part number ${partNumber} is not between 1 and 10,000`);
      }
      version += 1;
      const etag = `part-${version}`;
      e.parts.set(partNumber, { etag, body: body.slice() });
      return { partNumber, etag };
    },
    async completeUpload(upload, parts) {
      const e = open(upload);
      const listed = [...parts].sort((a, b) => a.partNumber - b.partNumber);
      const bodies = listed.map((p, i) => {
        const part = e.parts.get(p.partNumber);
        if (part === undefined || part.etag !== p.etag) throw new globalThis.Error(`R2: part ${p.partNumber} was not uploaded with etag ${p.etag}`);
        if (i > 0 && listed[i - 1].partNumber === p.partNumber) throw new globalThis.Error(`R2: part ${p.partNumber} is listed twice`);
        return part.body;
      });
      if (bodies.length === 0) throw new globalThis.Error("R2: a multipart upload needs at least one part");
      if (bodies.slice(0, -1).some((b) => b.length !== bodies[0].length)) {
        throw new globalThis.Error("R2: every part but the last must be the same size");
      }
      if (bodies.length > 1 && bodies[bodies.length - 1].length > bodies[0].length) {
        throw new globalThis.Error("R2: the last part must be no larger than the others");
      }
      const out = new globalThis.Uint8Array(bodies.reduce((n, b) => n + b.length, 0));
      let at = 0;
      for (const b of bodies) {
        out.set(b, at);
        at += b.length;
      }
      uploads.delete(`${upload.key}\u0000${upload.uploadId}`);
      return write(upload.key, out, e.options.contentType, e.options.metadata);
    },
    async abortUpload(upload) {
      uploads.delete(`${upload.key}\u0000${upload.uploadId}`);
    },
  };
"#;

/// P2 (#2578): the body of `bynk.cloudflare`'s `Queue` unit-tier double
/// ([`platform_double`]).
pub(super) const QUEUE_DOUBLE_BODY: &str = r#"  const json = (body: string) => {
    try {
      globalThis.JSON.parse(body);
    } catch {
      throw new globalThis.Error("bynk.cloudflare.Queue sends JSON text (normally `Json.encode(value)`), and this body is not JSON");
    }
  };
  return {
    async send(body) {
      json(body);
    },
    async sendWith(body, options) {
      json(body);
      if (options.delay.tag === "Some" && (options.delay.value < 0 || globalThis.Math.ceil(options.delay.value / 1000) > 43200)) {
        throw new globalThis.Error(`bynk.cloudflare.Queue: a delay is between 0 seconds and 12 hours, not ${options.delay.value} ms`);
      }
    },
    async sendBatch(bodies) {
      for (const b of bodies) json(b);
    },
    async sendValue(message) {
      json(globalThis.JSON.stringify(message));
    },
    async sendValueWith(message, options) {
      json(globalThis.JSON.stringify(message));
      if (options.delay.tag === "Some" && (options.delay.value < 0 || globalThis.Math.ceil(options.delay.value / 1000) > 43200)) {
        throw new globalThis.Error(`bynk.cloudflare.Queue: a delay is between 0 seconds and 12 hours, not ${options.delay.value} ms`);
      }
    },
    async sendValueBatch(messages) {
      for (const m of messages as readonly unknown[]) json(globalThis.JSON.stringify(m));
    },
  };
"#;

/// P3b (#2582): the body of `bynk.cloudflare`'s `Analytics` unit-tier double
/// ([`platform_double`]), which checks a point as `WorkersAnalytics` does.
pub(super) const ANALYTICS_DOUBLE_BODY: &str = r#"  const bytes = (text: string) => new globalThis.TextEncoder().encode(text).length;
  const fault = (why: string): never => {
    throw new globalThis.Error(`bynk.cloudflare.Analytics: ${why}`);
  };
  return {
    async write(point) {
      if (point.blobs.length > 20) fault(`a data point has at most 20 blobs, not ${point.blobs.length}`);
      if (point.doubles.length > 20) fault(`a data point has at most 20 doubles, not ${point.doubles.length}`);
      if (point.index.tag === "Some" && bytes(point.index.value) > 96) fault(`an index is at most 96 bytes, not ${bytes(point.index.value)}`);
      const blobBytes = point.blobs.reduce((n: number, b: string) => n + bytes(b), 0);
      if (blobBytes > 16384) fault(`a data point's blobs total at most 16 KB, not ${blobBytes} bytes`);
      if (!point.doubles.every(globalThis.Number.isFinite)) fault("a double is finite (not NaN or an infinity)");
    },
  };
"#;

/// #291: the deterministic test double for platform capability `cap` (owned by
/// first-party unit `owner`, imported in the test module as `ns`), as the
/// definition of a `__bynkTest_<name>()` factory — `name` is `cap`, or a
/// derived capability's own name (S4a, #1966: `capability Sessions = Kv` has
/// `__bynkTest_Sessions()`, a `Kv` double of its own) — or `None` for a capability
/// with no double (a user adapter's). Each double is the same on every run and
/// does no I/O: `Logger` and `Events` do nothing (observation records the
/// calls), `Clock` reads `0`, `Random` counts its UUIDs and draws integers
/// from a fixed-seed generator, `Secrets` has none, `Locale` is `"en"`,
/// `Idempotency` and `Kv` are in-memory (a TTL or `expiresAfter` is accepted
/// and ignored: expiry is not modelled), and `Fetch` faults — a test never
/// reaches the network, so it must `stub Fetch.send` — as does each
/// `bynk.cloudflare` `Service` instance (`stub Billing.send`). `__makeTestDeps`
/// builds fresh ones per case.
///
/// #2137: with `harness_clock` (a module whose cases `advance Clock`), `Clock`
/// reads the harness clock (`emitter/test_runtime/clock.ts`) instead,
/// resetting it to the epoch as each case builds its deps.
pub(super) fn platform_double(
    owner: &str,
    cap: &str,
    ns: &str,
    name: &str,
    harness_clock: bool,
    // P4b (#2608): a `D1` instance's migrations, run on its fresh database.
    d1_migrations: Option<&[MigrationFile]>,
) -> Option<String> {
    let body = match (owner, cap) {
        // K5a (#2586): every level a no-op, as `info` and `error` are.
        ("bynk", "Logger") => "  return {\n    async info() {},\n    async error() {},\n    async warn() {},\n    async debug() {},\n    async log() {},\n  };\n".to_string(),
        ("bynk", "Events") => "  return {\n    async emit() {},\n  };\n".to_string(),
        ("bynk", "Clock") if harness_clock => {
            "  __bynkClockNow = 0;\n  return __bynkClock;\n".to_string()
        }
        ("bynk", "Clock") => "  return {\n    async now() {\n      return 0;\n    },\n  };\n".to_string(),
        ("bynk", "Random") => "  let n = 0;\n  let seed = 0x2545f491;\n  return {\n    async uuid() {\n      n += 1;\n      return `00000000-0000-4000-8000-${n.toString(16).padStart(12, \"0\")}` as never;\n    },\n    async int(lo, hi) {\n      seed = (globalThis.Math.imul(seed, 1664525) + 1013904223) >>> 0;\n      return hi > lo ? lo + (seed % (hi - lo)) : lo;\n    },\n    async bytes(n) {\n      if (!globalThis.Number.isInteger(n) || n < 1 || n > 65536) {\n        throw new globalThis.Error(`Random.bytes(n) requires 1 <= n <= 65536, got ${n}`);\n      }\n      const out = new globalThis.Uint8Array(n);\n      for (let i = 0; i < n; i++) {\n        seed = (globalThis.Math.imul(seed, 1664525) + 1013904223) >>> 0;\n        out[i] = seed >>> 24;\n      }\n      return out;\n    },\n  };\n".to_string(),
        // K4: `bynk.crypto` has no fake. Digests, MACs and signatures are
        // deterministic for given inputs (Ed25519 and RSASSA-PKCS1-v1_5
        // included), so the double is the real provider, from the binding the
        // module imports as `bynk_crypto__binding`.
        ("bynk.crypto", "Crypto") => "  return new bynk_crypto__binding.CryptoProvider();\n".to_string(),
        ("bynk", "Secrets") => "  return {\n    async get() {\n      return None;\n    },\n  };\n".to_string(),
        ("bynk", "Locale") => "  return {\n    async current() {\n      return \"en\" as never;\n    },\n  };\n".to_string(),
        ("bynk", "Fetch") => "  return {\n    async send() {\n      throw new globalThis.Error(\"bynk: `Fetch.send` is not stubbed, and a test never reaches the network — add `stub Fetch.send(_) returns …`\");\n    },\n    async sendWith() {\n      throw new globalThis.Error(\"bynk: `Fetch.sendWith` is not stubbed, and a test never reaches the network — add `stub Fetch.sendWith(_, _) returns …`\");\n    },\n    async receiveBytes() {\n      throw new globalThis.Error(\"bynk: `Fetch.receiveBytes` is not stubbed, and a test never reaches the network — add `stub Fetch.receiveBytes(_, _) returns …`\");\n    },\n    async sendBytes() {\n      throw new globalThis.Error(\"bynk: `Fetch.sendBytes` is not stubbed, and a test never reaches the network — add `stub Fetch.sendBytes(_, _, _) returns …`\");\n    },\n    async receiveStream() {\n      throw new globalThis.Error(\"bynk: `Fetch.receiveStream` is not stubbed, and a test never reaches the network — add `stub Fetch.receiveStream(_, _) returns …`\");\n    },\n    async sendStream() {\n      throw new globalThis.Error(\"bynk: `Fetch.sendStream` is not stubbed, and a test never reaches the network — add `stub Fetch.sendStream(_, _, _) returns …`\");\n    },\n  };\n".to_string(),
        ("bynk", "Idempotency") => "  const store = new globalThis.Map<string, unknown>();\n  return {\n    async dedup(key) {\n      return (store.has(key) ? Some(store.get(key)) : None) as never;\n    },\n    async remember(key, value) {\n      store.set(key, value);\n    },\n  };\n".to_string(),
        // P1 (#2590): values are bytes (text is its UTF-8, so the two views
        // agree as on the platform), with metadata and an expiry beside them;
        // the new operations check the platform's limits as `WorkersKv` does.
        // Expiry reads the clock `Clock.now()` does, so an `At` built from
        // it is judged on the same timeline (the harness clock at `unit`).
        ("bynk.cloudflare", "Kv") => KV_DOUBLE_BODY.replace(
            "__NOW__",
            if harness_clock { "__bynkClockNow" } else { "0" },
        ),
        // P5 (#2323): each `R2` instance is an in-memory bucket honouring
        // `putWith`'s conditions, its metadata and `list`'s paging (R2's own
        // page of 1000, the cursor being the last key). `uploaded` reads `0`,
        // as `Clock` does, and an etag is a per-bucket counter.
        ("bynk.cloudflare", "R2") => R2_DOUBLE_BODY.to_string(),
        // P3 (#2342): each `RateLimit` instance counts calls per key over
        // fixed windows of its `@period`, on the harness clock when the
        // module's cases `advance Clock` (else time stands still at 0), so a
        // case can exhaust a limiter and, by advancing, recover. The bare
        // capability's double (a `consumes … { RateLimit }` entry, never
        // usable bare) is built without them, and never refuses.
        ("bynk.cloudflare", "RateLimit") => {
            let now = if harness_clock { "__bynkClockNow" } else { "0" };
            return Some(format!(
                "function __bynkTest_{name}(limit: number = globalThis.Number.POSITIVE_INFINITY, period: number = 60): {ns}.{cap} {{\n  const counts = new globalThis.Map<string, {{ window: number; count: number }}>();\n  return {{\n    async allow(key) {{\n      const window = globalThis.Math.floor({now} / (period * 1000));\n      const e = counts.get(key);\n      const c = e !== undefined && e.window === window ? e : {{ window, count: 0 }};\n      c.count += 1;\n      counts.set(key, c);\n      return c.count <= limit;\n    }},\n  }};\n}}\n"
            ));
        }
        // L7 (#2414): there is no edge cache in a test, so each instance
        // accepts every purge, checking its prefixes as the binding does;
        // `stub Edge.purge(_) returns false` refuses one, and observation
        // sees what was purged.
        ("bynk.cloudflare", "EdgeCache") => "  return {\n    async purge(prefixes) {\n      for (const p of prefixes) {\n        if (!p.startsWith(\"/\") || /[\\u0000-\\u001f\\u007f?#]/.test(p)) throw new globalThis.Error(`bynk.cloudflare.EdgeCache.purge takes paths starting with \\`/\\`, with no query, fragment or control characters; got ${globalThis.JSON.stringify(p)}`);\n      }\n      return true;\n    },\n  };\n".to_string(),
        // P2 (#2578): there is no queue in a test, so each instance accepts
        // every send, checking its bodies are JSON (and a delay in range) as
        // the binding does; observation sees what was sent.
        ("bynk.cloudflare", "Queue") => QUEUE_DOUBLE_BODY.to_string(),
        // P3b (#2582): there is no dataset in a test, so each instance accepts
        // every write within the platform's limits; observation (the generic
        // call wrapper, as for every double) sees what was written.
        ("bynk.cloudflare", "Analytics") => ANALYTICS_DOUBLE_BODY.to_string(),
        // P4a (#2602): each `D1` instance is the real `WorkersD1` over its own
        // in-memory SQLite database (`node:sqlite`), so a test runs its
        // statements as D1 would. P4b (#2608): the database's migrations run
        // on it first.
        ("bynk.cloudflare", "D1") => format!(
            "  return new bynk_cloudflare__binding.WorkersD1({{ D1: __makeIntegrationD1({}) }}, \"D1\");\n",
            d1_migrations_text(d1_migrations.unwrap_or_default())
        ),
        // #3300: a fixed version (id `"test"`, no tag, created at the
        // epoch), so a case's result never depends on the deployment;
        // `stub Version.current() returns …` gives another.
        ("bynk.cloudflare", "Version") => "  return {\n    async current() {\n      return { id: \"test\", tag: None, deployed: 0 } as never;\n    },\n  };\n".to_string(),
        // Interop I5 (#1972): like `Fetch`, a test never reaches the foreign
        // Worker, so each instance faults until its `send` is stubbed.
        ("bynk.cloudflare", "Service") => format!("  return {{\n    async send() {{\n      throw new globalThis.Error(\"bynk: `{name}.send` is not stubbed, and a test never reaches the Worker behind it — add `stub {name}.send(_) returns …`\");\n    }},\n  }};\n"),
        _ => return None,
    };
    Some(format!(
        "function __bynkTest_{name}(): {ns}.{cap} {{\n{body}}}\n"
    ))
}

/// #2134: the `Secrets` test double of a module whose cases run with `env`
/// entries: it answers `Some(value)` for a name the running case's `env`
/// gives (`__bynkEnv`), and `None` for any other, as [`platform_double`]'s
/// does for every name. `None` for any other capability.
pub(super) fn env_secrets_double(owner: &str, cap: &str, ns: &str, name: &str) -> Option<String> {
    if (owner, cap) != ("bynk", "Secrets") {
        return None;
    }
    Some(format!(
        "function __bynkTest_{name}(): {ns}.{cap} {{\n  return {{\n    async get(key) {{\n      const v = __bynkEnv[key];\n      return v === undefined ? None : Some(v);\n    }},\n  }};\n}}\n"
    ))
}
