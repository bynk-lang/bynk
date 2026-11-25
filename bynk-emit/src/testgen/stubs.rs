//! `stub` clause emission: stub classes, platform doubles and overlays.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use crate::emitter::RuntimeUse;
use crate::emitter::ts_types::ts_type_ref_qualified_multi_ts_type;
// #1759: every namespace a test module imports, from one definition.
use crate::emitter::emit::test_scaffold_ns as test_ns;
use bynk_check::checker::Types;
use bynk_check::d1_migrations::MigrationFile;
use bynk_check::test_suites::{self, ResolvedStub, StubScope};
use bynk_syntax::ast::{
    ArgPattern, BaseType, CapabilityOp, Commons, CommonsForm, CommonsItem, Expr, Ident, Param,
    QualifiedName, SeqOutcome, StubClause, StubRhs, Trivia, TypeRef,
};
use bynk_syntax::span::Span;
use bynk_ts::{
    TsBinaryOp, TsBindingName, TsClassCtor, TsClassField, TsClassMethod, TsDecl, TsExpr, TsParam,
    TsStmt, TsSwitchCase, TsType,
};
use std::sync::Arc;

/// The namespace each type name in the stubbed capability's *signature* resolves to.
fn signature_type_ns(
    rp: &ResolvedStub,
    owning_unit: &str,
    type_ns: &HashMap<String, String>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
) -> HashMap<String, String> {
    // #2887: the stubbed capability's signature names its own unit's types by
    // the names that unit declares, which this target may have renamed
    // (`consumes U { T as Alias }`) to keep them off a type of its own: those
    // names are `U`'s whatever else the target binds them to.
    let mut sig_type_ns = type_ns.clone();
    add_service_sig_types(rp, unit_tables, &mut sig_type_ns);
    if let Some(owner) = unit_tables
        .get(owning_unit)
        .and_then(|t| t.flattened_caps.get(&rp.cap))
    {
        for a in unit_tables[owning_unit]
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
    sig_type_ns
}

/// #3945: the types a clause's explicit type arguments resolve in, for its call key.
fn clause_key_types(
    owning_unit: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
) -> HashMap<String, Arc<bynk_syntax::ast::TypeDecl>> {
    test_suites::build_privileged_resolved(
        owning_unit,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    )
    .map(|(r, _)| r.types)
    .unwrap_or_default()
}

/// #3945: `__tk && __tk.__bynkTypeKey === "<key>"`: the clause answers the call made at its type
/// arguments.
fn type_key_cond(
    type_args: &[bynk_syntax::ast::TypeRef],
    key_types: &HashMap<String, Arc<bynk_syntax::ast::TypeDecl>>,
    tys: &Arc<Types>,
) -> TsExpr {
    and_expr(
        ident("__tk"),
        strict_eq(
            member(ident("__tk"), "__bynkTypeKey"),
            str_lit(bynk_check::checker::capability_type_key(
                type_args, key_types, tys,
            )),
        ),
    )
}

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
    let sig_type_ns = signature_type_ns(rp, &owning_unit, &type_ns, unit_tables, unit_uses);
    let key_types = clause_key_types(
        &owning_unit,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    );
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

    // One per-call sequence cursor field per `returns each` clause (`__seq_0 = 0;`).
    let mut fields: Vec<TsClassField> = rp
        .clauses
        .iter()
        .enumerate()
        .filter(|(_, c)| matches!(c.rhs, StubRhs::ReturnsEach(..)))
        .map(|(idx, _)| TsClassField::inferred(seq_field(idx), num_lit("0")))
        .collect();
    let mut constructor: Option<TsClassCtor> = None;
    let mut methods: Vec<TsClassMethod> = Vec::new();
    // #291: a case-scoped clause applies only while its own case runs, so a
    // class with one carries the running case's name (`__makeTestDeps`'s
    // argument), and `__applies` tells `__bynkOverlay` which operations this
    // case stubs at all — an operation stubbed only by other cases reaches the
    // tier default instead. #1860: a suite-scoped clause, when another suite
    // shares the module, applies only while its own suite's runs do, so the
    // class also carries the running suite's ordinal.
    let case_scoped = stub_is_scoped(rp);
    if case_scoped {
        let maybe = |ty: &str| TsType::union(vec![TsType::named(ty), TsType::named("undefined")]);
        let this_field = |name: &str| member(ident("this"), name);
        let optional = |name: &str, ty: &str| TsParam {
            optional: true,
            ..TsParam::new(name, Some(TsType::named(ty)))
        };
        fields.push(TsClassField::new("__case", maybe("string")));
        let mut params = vec![optional("c", "string")];
        let mut body = vec![assign(this_field("__case"), ident("c"))];
        if stub_is_suite_scoped(rp) {
            fields.push(TsClassField::new("__suite", maybe("number")));
            params.push(optional("s", "number"));
            body.push(assign(this_field("__suite"), ident("s")));
        }
        constructor = Some(TsClassCtor { params, body });
        let mut applies: Vec<TsStmt> = Vec::new();
        for (method, clause_idxs) in &by_method {
            let scopes: Vec<&StubScope> =
                clause_idxs.iter().map(|&i| &rp.clause_scopes[i]).collect();
            // An operation with any clause for every run is always stubbed.
            if scopes.contains(&&StubScope::Every) {
                continue;
            }
            applies.push(if_(
                strict_eq(ident("op"), str_lit(method.as_str())),
                ret(scopes_test(&scopes)),
            ));
        }
        applies.push(ret(bool_lit(true)));
        methods.push(TsClassMethod::new(
            "__applies",
            false,
            vec![TsParam::new("op", Some(TsType::named("string")))],
            Some(TsType::named("boolean")),
            applies,
        ));
    }

    for (method, clause_idxs) in &by_method {
        let Some(op) = rp.cap_decl.ops.iter().find(|o| &o.name.name == method) else {
            continue;
        };
        // #3945: a generic operation's signature names its own type parameters, which a class
        // method cannot spell, so its parameters and reply are `unknown`; each clause is typed
        // against its own substituted operation, and the call's trailing type-key argument
        // (`__tk`) picks which clauses answer.
        let generic = !op.type_params.is_empty();
        let mut params: Vec<TsParam> = op
            .params
            .iter()
            .map(|p| {
                let ty = if generic {
                    TsType::named("unknown")
                } else {
                    ts_type_ref_qualified_multi_ts_type(&p.type_ref, &sig_type_ns)
                };
                TsParam::new(p.name.name.clone(), Some(ty))
            })
            .collect();
        let return_ty = if generic {
            TsType::named("Promise<unknown>")
        } else {
            ts_type_ref_qualified_multi_ts_type(&op.return_type, &sig_type_ns)
        };
        if generic {
            params.push(TsParam {
                optional: true,
                ..TsParam::new(
                    "__tk",
                    Some(TsType::named("{ __bynkTypeKey?: string } | undefined")),
                )
            });
        }

        let (mut body, mut taken) =
            method_scope(rp, unit_tables, &scope_ns, &scope_names, &scope_type_names);
        // #2947: a suite helper inlined ahead of a value (#2907) spells its
        // signature's types unqualified, as in a case runner, so a method with
        // one binds the same `uses` commons' names `emit_test_scope_setup`
        // does (assets and embeds included). A method without one is unchanged.
        if clause_idxs.iter().any(|&i| !rp.clause_fns[i].is_empty()) {
            for (ns, names, types) in
                used_commons_destructures(&owning_unit, unit_tables, unit_uses, &mut HashSet::new())
            {
                taken.extend(names.iter().chain(types.iter().map(|(n, _)| n)).cloned());
                body.extend(emit_ns_destructure(&ns, &names, &types));
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
            body.extend(emit_ns_destructure_renamed(
                &test_ns(u),
                &names,
                &types,
                &renames,
                &declared,
            ));
        }
        for &idx in clause_idxs {
            let clause = &rp.clauses[idx];
            let clause_op = rp.op_for_clause(idx).unwrap_or_else(|| op.clone());
            let op = &clause_op;
            // Argument-pattern consts: a `Value(e)` pattern lowers to a const the
            // condition compares structurally; an `Any` pattern contributes none.
            let mut cond_parts: Vec<TsExpr> = Vec::new();
            for (i, pat) in clause.args.iter().enumerate() {
                if let ArgPattern::Value(e) = pat
                    && let Some(param) = op.params.get(i)
                {
                    let value = lower_stub_value_block(
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
                    body.push(const_(vname.clone(), await_expr(iife(value))));
                    cond_parts.push(call(
                        ident("__bynkEq"),
                        vec![ident(param.name.name.clone()), ident(vname)],
                    ));
                }
            }
            let scope = &rp.clause_scopes[idx];
            if *scope != StubScope::Every {
                cond_parts.insert(0, scopes_test(&[scope]));
            }
            if generic {
                cond_parts.push(type_key_cond(&clause.type_args, &key_types, tys));
            }
            let cond = cond_parts
                .into_iter()
                .reduce(and_expr)
                .unwrap_or_else(|| bool_lit(true));
            let rhs_body = if rp.agent {
                agent_stub_rhs(clause, cap)
            } else {
                emit_stub_rhs(
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
                )
            };
            body.push(if_(cond, block(rhs_body)));
        }
        body.push(no_stub_match_stmt(
            rp,
            format!("bynk: no stub clause matched for {cap}.{method}"),
        ));

        methods.push(TsClassMethod::new(
            method.clone(),
            true,
            params,
            Some(return_ty),
            body,
        ));
    }
    // The stub's values are scaffolding, not user test logic, so the class maps nothing: the
    // statements the lowering built carry source spans, which are dropped here (v0.70).
    let mut class = TsStmt::decl(
        TsDecl::class(stub_class_name(cap), fields, constructor, methods),
        None,
    );
    class.clear_spans();
    class
}

/// `(async () => { body })()`: a block run in place, for a value that needs its own statements.
fn iife(body: Vec<TsStmt>) -> TsExpr {
    call(paren(block_arrow(Vec::new(), true, body)), Vec::new())
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

/// #3894: a consumed service's stub that is `rejected` or `unreachable` throws an error marked
/// with the outcome it stands for (`bynkCallStub`), which `.outcome(…)` reads back as that
/// `CallOutcome` (`callOutcome` in the runtime). A plain call to it faults, as a real refused
/// call does.
fn stub_call_outcome_stmt(tag: &str, reason: Option<&str>) -> TsStmt {
    let mut marker = vec![bynk_ts::TsObjectEntry::prop("tag", str_lit(tag))];
    if let Some(reason) = reason {
        marker.push(bynk_ts::TsObjectEntry::prop("reason", str_lit(reason)));
    }
    let error = TsExpr::New {
        callee: Box::new(ident("globalThis.Error")),
        args: vec![str_lit("bynk: stubbed call outcome")],
    };
    TsStmt::throw_stmt(
        call(
            member(ident("globalThis.Object"), "assign"),
            vec![
                error,
                TsExpr::object_entries(vec![bynk_ts::TsObjectEntry::prop(
                    "bynkCallStub",
                    TsExpr::object_entries(marker),
                )]),
            ],
        ),
        None,
    )
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
) -> Vec<TsStmt> {
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
                let after_name = format!("__after_{clause_idx}");
                let mut out = vec![
                    const_(after_name.clone(), await_expr(iife(ms))),
                    expr_stmt(await_expr(call(
                        ident("__bynkSleep"),
                        vec![ident(after_name)],
                    ))),
                ];
                out.extend(lower(e));
                out
            }
            None => lower(e),
        },
        // #2683: `never` — the call stays in flight for ever.
        StubRhs::Never(_) => vec![ret(await_expr(call(ident("__bynkNever"), Vec::new())))],
        StubRhs::Fails(_) | StubRhs::Faulted(_) => vec![stub_fault_stmt()],
        StubRhs::Rejected(refusal, _) => {
            vec![stub_call_outcome_stmt("Rejected", Some(&refusal.name))]
        }
        StubRhs::Unreachable(_) => vec![stub_call_outcome_stmt("Unreachable", None)],
        StubRhs::ReturnsEach(outcomes, _) => {
            let n = outcomes.len();
            let mut stmts = vec![TsStmt::const_stmt(
                TsBindingName::Ident("__k".to_string()),
                None,
                member(ident("this"), seq_field(clause_idx)),
                None,
            )];
            if n > 1 {
                stmts.push(if_(
                    TsExpr::Binary {
                        op: TsBinaryOp::LessThan,
                        left: Box::new(member(ident("this"), seq_field(clause_idx))),
                        right: Box::new(num_lit((n - 1).to_string())),
                    },
                    TsStmt::increment(member(ident("this"), seq_field(clause_idx)), None),
                ));
            }
            let outcome_body = |o: &SeqOutcome| match o {
                SeqOutcome::Value(e) => lower(e),
                SeqOutcome::Fails(_) => vec![stub_fault_stmt()],
            };
            let mut cases: Vec<TsSwitchCase> = outcomes
                .iter()
                .enumerate()
                .take(n.saturating_sub(1))
                .map(|(j, o)| TsSwitchCase {
                    labels: vec![num_lit(j.to_string())],
                    body: outcome_body(o),
                    default_braced: false,
                    // Arc E slice 6 (#1445): unaffected by `case_braced`'s
                    // addition — this site's own non-`default` cases were
                    // already, and stay, `{ }`-blocked.
                    case_braced: true,
                    span: None,
                })
                .collect();
            cases.push(TsSwitchCase {
                labels: vec![],
                body: outcome_body(&outcomes[n - 1]),
                default_braced: true,
                case_braced: true,
                span: None,
            });
            stmts.push(TsStmt::switch_stmt(ident("__k"), cases, None));
            stmts
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
) -> Vec<TsStmt> {
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
        unit_consumes_aliases,
        hidden,
        tys,
        items.types,
    );
    let block = test_suites::value_block(e);
    let mut helpers: Vec<TsStmt> = Vec::new();
    let mut suite_decls: Vec<TsStmt> = Vec::new();
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
            let caps =
                test_suites::target_capability_map(&owning_unit, unit_tables, &resolved, tys);
            let (expr_types, callees) = test_suites::check_suite_fns(
                items.fns,
                &mut resolved,
                unit_tables.get(&owning_unit),
                Some(&caps),
                &mut errs,
                &mut RefSink::new(),
                &mut LocalsSink::new(),
                tys,
            );
            typed.expr_types.extend(expr_types);
            typed.callees.extend(callees);
            test_suites::add_suite_fns_to_typed(&mut typed, items.fns);
            helpers.extend(super::test_cases::suite_given_aliases(
                items.fns,
                &owning_unit,
                &resolved.cross_context,
            ));
            for f in items.fns {
                let stmts = emitter::emit_suite_fn(f, &typed, runtime_use);
                helpers.extend(stmts);
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
        case_body::lower_block_to_async_stmts(&block, ret_type, &mut typed, &cross, runtime_use);
    // #3111: the codecs the value reached for beyond the always-present ones.
    let late_codecs = suite_late_codec_stmts(items, &typed, runtime_use, roots_mark);
    let aliases = fixtures.iter().map(|f| {
        const_typed(
            f.name.name.clone(),
            TsType::named("string"),
            ident(fixture_ident(&f.path)),
        )
    });
    let mut stmts: Vec<TsStmt> = suite_decls
        .into_iter()
        .chain(late_codecs)
        .chain(aliases)
        .chain(helpers)
        .chain(body)
        .collect();
    // A stub value maps nothing (v0.70): drop the spans the lowering recorded.
    stmts.iter_mut().for_each(TsStmt::clear_spans);
    stmts
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

#[allow(clippy::too_many_arguments)]
pub(super) fn synthetic_typed_commons_for_target(
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    hidden: &BTreeMap<String, String>,
    tys: &Arc<checker::Types>,
    // #3536: the declaring suite's own `type`s. One named like a first-party
    // type the target's braces leave out is not that type in the case's scope.
    suite_types: &[bynk_syntax::ast::TypeDecl],
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
                let shadowed = table.unselected_consumed_types.contains(n)
                    && suite_types.iter().any(|s| s.name.name == *n);
                if d.private.is_none() && !shadowed {
                    types.entry(n.clone()).or_insert_with(|| d.clone());
                }
            }
        }
    }
    for (n, c, _) in platform_seams(target_name, unit_tables) {
        caps.entry(n.clone()).or_insert_with(|| c.clone());
    }
    // #3892: a consumed context's services are seams too (`with` and `trace`
    // read their parameter names).
    for (n, c) in test_suites::consumed_service_seam_decls(
        unit_tables,
        unit_consumes
            .get(target_name)
            .map_or(&[][..], Vec::as_slice),
        &unit_consumes_aliases
            .get(target_name)
            .cloned()
            .unwrap_or_default(),
    ) {
        caps.entry(n).or_insert(c);
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
            test_only: false,
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
            consumes: Vec::new(),
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
        unselected_types: Default::default(),
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
pub(super) fn scopes_test(scopes: &[&StubScope]) -> TsExpr {
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
    let this = |field: &str| member(ident("this"), field);
    let tests: Vec<TsExpr> = cases
        .into_iter()
        .map(|c| strict_eq(this("__case"), str_lit(c.as_str())))
        .chain(
            suites
                .into_iter()
                .map(|n| strict_eq(this("__suite"), num_lit(n.to_string()))),
        )
        .collect();
    debug_assert!(!tests.is_empty(), "scopes_test: every scope was `Every`");
    tests
        .into_iter()
        .reduce(or_expr)
        .unwrap_or_else(|| bool_lit(false))
}

/// #3892: a consumed service's signature names the types its own context
/// declares, qualified by that context's namespace.
fn add_service_sig_types(
    rp: &ResolvedStub,
    unit_tables: &HashMap<String, UnitTable>,
    sig_type_ns: &mut HashMap<String, String>,
) {
    if let Some(ctx) = &rp.service_context
        && let Some(table) = unit_tables.get(ctx)
    {
        for n in public_types(table) {
            sig_type_ns.insert(n.clone(), test_ns(ctx));
        }
    }
}

/// The names a stub method's body binds before its values: the target's own
/// types and functions, and (#3892) for a consumed service its own context's
/// types, whose variants a value may name bare (`Err(Declined)`). Returns the
/// statements and the names taken.
fn method_scope(
    rp: &ResolvedStub,
    unit_tables: &HashMap<String, UnitTable>,
    scope_ns: &str,
    scope_names: &[String],
    scope_type_names: &[(String, Vec<String>)],
) -> (Vec<TsStmt>, HashSet<String>) {
    let mut body = emit_ns_destructure(scope_ns, scope_names, scope_type_names);
    let mut taken: HashSet<String> = scope_names.iter().cloned().collect();
    if let Some(ctx) = &rp.service_context
        && let Some(table) = unit_tables.get(ctx)
    {
        let types = aliased_types(table, |n| {
            is_public_type(table, n)
                && !taken.contains(n)
                && !scope_type_names.iter().any(|(t, _)| t == n)
        });
        let names: Vec<String> = types.iter().map(|(n, _)| n.clone()).collect();
        taken.extend(names.iter().cloned());
        body.extend(emit_ns_destructure(&test_ns(ctx), &names, &types));
    }
    (body, taken)
}

/// The name of a seam's stub class: `__Stub_<Cap>`. A consumed service is
/// stubbed by its context's qualified path (#3892), whose dots are not identifier
/// characters.
pub(super) fn stub_class_name(cap: &str) -> String {
    format!("__Stub_{}", cap.replace('.', "_"))
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
                base.unwrap_or_else(absent),
                TsExpr::New {
                    callee: Box::new(ident(stub_class_name(cap))),
                    args,
                },
                str_lit(cap.as_str()),
            ],
        )),
        ty: TsType::named(ty),
    }
}

/// `Idempotency`'s double: an in-memory store whose entries and claims expire
/// on the clock `Clock.now()` reads (`__NOW__`, substituted as for `Kv`; `__CLAIM__` is the module's `Claim`), so a
/// test that `advance`s the harness clock sees a window lapse.
pub(super) const IDEMPOTENCY_DOUBLE_BODY: &str = r#"  const store = new globalThis.Map<string, { value: unknown; until: number }>();
  const claims = new globalThis.Map<string, number>();
  return {
    // `dedup` and `remember` carry JSON: the call site encodes what `remember` is given and
    // decodes the reply, so a case runs the round trip production does (a value written as
    // one type and read as another reads as a miss).
    async dedup(key: string) {
      const hit = store.get(key);
      return hit !== undefined && hit.until > __NOW__ ? { kind: "Some", value: hit.value } : { kind: "None" };
    },
    async remember(key: string, value: unknown, expiresAfter: number) {
      store.set(key, { value: globalThis.JSON.parse(globalThis.JSON.stringify(value)), until: __NOW__ + expiresAfter });
    },
    async claim(key: string, expiresAfter: number) {
      const until = claims.get(key);
      if (until !== undefined && until > __NOW__) {
        return __CLAIM__.AlreadyClaimed;
      }
      claims.set(key, __NOW__ + expiresAfter);
      return __CLAIM__.Claimed;
    },
    // Hidden from the capability: a handler's claim scope, as the real
    // providers take it (a lease, then released on failure or confirmed).
    __track() {
      const held: { key: string; expiresAfter: number; until: number }[] = [];
      return {
        lease: (expiresAfter: number) => globalThis.Math.min(expiresAfter, 30000),
        note(key: string, claim: { tag: string }, expiresAfter: number) {
          const until = claims.get(key);
          if (claim.tag === "Claimed" && until !== undefined) held.push({ key, expiresAfter, until });
          return claim;
        },
        async settle(result: { tag?: string; status?: number } | null | undefined, threw: boolean) {
          const tag = result?.tag;
          const failed =
            threw ||
            tag === "Err" ||
            (tag !== undefined && ["ServerError", "NotImplemented", "BadGateway", "ServiceUnavailable", "GatewayTimeout"].includes(tag)) ||
            (tag === "RawError" && (result?.status ?? 0) >= 500);
          for (const h of held) {
            if (claims.get(h.key) !== h.until) continue;
            if (failed) claims.delete(h.key);
            else claims.set(h.key, __NOW__ + h.expiresAfter);
          }
        },
      };
    },
  } as never;
"#;

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
      // `KvTtl` carries the 60-second floor; whole seconds round towards zero.
      return __NOW__ + globalThis.Math.trunc((e.ttl ?? 0) / 1000) * 1000;
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
    // Unchecked, as `putTtl` always was (its `KvTtl` carries the floor); its
    // expiry is kept and listed, as the platform and the integration tier do.
    async putTtl(key, value, ttl) {
      store.set(key, { ...plain(enc.encode(value)), expiration: __NOW__ + globalThis.Math.trunc(ttl / 1000) * 1000 });
    },
    async delete(key) {
      store.delete(key);
    },
    async list(prefix) {
      return [...store.keys()].filter((k) => prefix.tag === "None" || k.startsWith(prefix.value)).sort();
    },
    async getWith(key, options) {
      checkKey(key);
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
      if (options.delay.tag === "Some" && (options.delay.value < 0 || globalThis.Math.ceil(options.delay.value / 1000) > 86400)) {
        throw new globalThis.Error(`bynk.cloudflare.Queue: a delay is between 0 seconds and 24 hours, not ${options.delay.value} ms`);
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
      if (options.delay.tag === "Some" && (options.delay.value < 0 || globalThis.Math.ceil(options.delay.value / 1000) > 86400)) {
        throw new globalThis.Error(`bynk.cloudflare.Queue: a delay is between 0 seconds and 24 hours, not ${options.delay.value} ms`);
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
    async dataset() {
      return "events.analyticsEngine.__DATASET__";
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
/// reads the harness clock (`runtime/src/testing/clock.ts`) instead,
/// resetting it to the epoch as each case builds its deps.
pub(super) fn platform_double(
    owner: &str,
    cap: &str,
    ns: &str,
    name: &str,
    harness_clock: bool,
    // P4b (#2608): a `D1` instance's migrations, run on its fresh database.
    // #3321: for a `Postgres` instance, its `schema.sql` when the project opts
    // in to PGlite (`Some`, possibly empty), else `None`.
    d1_migrations: Option<&[MigrationFile]>,
) -> Option<TsStmt> {
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
        // #3458: `bynk.testing`'s in-memory typed-read store is already
        // deterministic and does no I/O, and it keeps the JSON a `write` was
        // given, so a case exercises the same encode, store and decode round
        // trip the call sites do. The double is the real provider.
        ("bynk.testing", "TypedReads") => "  return new bynk_testing__binding.MemoryTypedReads();\n".to_string(),
        ("bynk", "Secrets") => "  return {\n    async get() {\n      return None;\n    },\n  };\n".to_string(),
        ("bynk", "Locale") => "  return {\n    async current() {\n      return \"en\" as never;\n    },\n  };\n".to_string(),
        ("bynk", "Fetch") => "  return {\n    async send() {\n      throw new globalThis.Error(\"bynk: `Fetch.send` is not stubbed, and a test never reaches the network — add `stub Fetch.send(_) returns …`\");\n    },\n    async sendWith() {\n      throw new globalThis.Error(\"bynk: `Fetch.sendWith` is not stubbed, and a test never reaches the network — add `stub Fetch.sendWith(_, _) returns …`\");\n    },\n    async receiveBytes() {\n      throw new globalThis.Error(\"bynk: `Fetch.receiveBytes` is not stubbed, and a test never reaches the network — add `stub Fetch.receiveBytes(_, _) returns …`\");\n    },\n    async sendBytes() {\n      throw new globalThis.Error(\"bynk: `Fetch.sendBytes` is not stubbed, and a test never reaches the network — add `stub Fetch.sendBytes(_, _, _) returns …`\");\n    },\n    async receiveStream() {\n      throw new globalThis.Error(\"bynk: `Fetch.receiveStream` is not stubbed, and a test never reaches the network — add `stub Fetch.receiveStream(_, _) returns …`\");\n    },\n    async sendStream() {\n      throw new globalThis.Error(\"bynk: `Fetch.sendStream` is not stubbed, and a test never reaches the network — add `stub Fetch.sendStream(_, _, _) returns …`\");\n    },\n  };\n".to_string(),
        ("bynk", "Idempotency") => IDEMPOTENCY_DOUBLE_BODY
            .replace(
                "__NOW__",
                if harness_clock { "__bynkClockNow" } else { "0" },
            )
            .replace("__CLAIM__", &format!("{ns}.Claim")),
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
            let params = vec![
                TsParam {
                    default: Some(ident("globalThis.Number.POSITIVE_INFINITY")),
                    ..TsParam::new("limit", Some(TsType::named("number")))
                },
                TsParam {
                    default: Some(num_lit("60")),
                    ..TsParam::new("period", Some(TsType::named("number")))
                },
            ];
            let body = format!(
                "  const counts = new globalThis.Map<string, {{ window: number; count: number }}>();\n  return {{\n    async allow(key) {{\n      const window = globalThis.Math.floor({now} / (period * 1000));\n      const e = counts.get(key);\n      const c = e !== undefined && e.window === window ? e : {{ window, count: 0 }};\n      c.count += 1;\n      counts.set(key, c);\n      return c.count <= limit;\n    }},\n  }};\n"
            );
            return Some(double_fn(name, ns, cap, params, body, &double_uses(owner, cap, harness_clock, false)));
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
        // #3513 B: `dataset()` names a stand-in (`test_<name>`), since no deploy qualifies one.
        ("bynk.cloudflare", "Analytics") => ANALYTICS_DOUBLE_BODY.replace(
            "__DATASET__",
            &format!("test_{}", name.to_ascii_lowercase()),
        ),
        // P4a (#2602): each `D1` instance is the real `WorkersD1` over its own
        // in-memory SQLite database (`node:sqlite`), so a test runs its
        // statements as D1 would. P4b (#2608): the database's migrations run
        // on it first.
        ("bynk.cloudflare", "D1") => format!(
            "  return new bynk_cloudflare__binding.WorkersD1({{ D1: __makeIntegrationD1({}) }}, \"D1\");\n",
            d1_migrations_text(d1_migrations.unwrap_or_default())
        ),
        // #3302: the real `WorkersEmail` over an in-memory outbox, so a test
        // sees the binding's own limit and header faults, `expect Mail.send
        // called with …` works with no stub (observation records the call),
        // and a message gets a deterministic id. `stub Mail.send(_) returns
        // Err(…)` forces an `EmailError`.
        ("bynk.cloudflare", "Email") => "  return new bynk_cloudflare__binding.WorkersEmail({ EMAIL: __makeIntegrationEmail() }, \"EMAIL\");\n".to_string(),
        // Hyperdrive H1 (#3318, DECISION A): a test never reaches Postgres, so
        // each instance faults until its operation is stubbed, as `Service`
        // does. The message names the instance and never a connection string:
        // there is none to name.
        // Hyperdrive H4 (#3321): with `[test] postgres = "pglite"` the real
        // `WorkersPostgres` over PGlite instead, reset for each case (the
        // double is built per case) and given the project's `schema.sql`.
        ("bynk.cloudflare", "Postgres") if d1_migrations.is_some() => {
            let schema: String = d1_migrations
                .unwrap_or_default()
                .iter()
                .map(|f| f.sql.as_str())
                .collect();
            format!(
                "  const __pg = __makeIntegrationPostgres({name}, {schema});\n  return new bynk_cloudflare__binding.WorkersPostgres(__pg.env, {name}, __pg.execute);\n",
                name = bynk_project::json_string(name),
                schema = bynk_project::json_string(&schema),
            )
        }
        ("bynk.cloudflare", "Postgres") => {
            let op = |op: &str, args: &str| {
                format!(
                    "    async {op}() {{\n      throw new globalThis.Error(\"bynk: `{name}.{op}` is not stubbed, and a test never reaches Postgres — add `stub {name}.{op}({args}) returns …`\");\n    }},\n"
                )
            };
            // `transaction` (#3320) takes one argument; the typed reads lower
            // over `all` and `first`, so stubbing those stubs them.
            format!(
                "  return {{\n{}{}{}{}  }};\n",
                op("all", "_, _"),
                op("first", "_, _"),
                op("run", "_, _"),
                op("transaction", "_")
            )
        }
        // Hyperdrive H5 (#3322): a `MySql` instance faults until stubbed, as a
        // `Postgres` one does. There is no database double: it is a stub-only
        // kind, and the unit double still records what `expect … called with`
        // reads.
        ("bynk.cloudflare", "MySql") => {
            let op = |op: &str, args: &str| {
                format!(
                    "    async {op}() {{\n      throw new globalThis.Error(\"bynk: `{name}.{op}` is not stubbed, and a test never reaches MySQL — add `stub {name}.{op}({args}) returns …`\");\n    }},\n"
                )
            };
            format!(
                "  return {{\n{}{}{}{}  }};\n",
                op("all", "_, _"),
                op("first", "_, _"),
                op("run", "_, _"),
                op("transaction", "_")
            )
        }
        // #3300: a fixed version (id `"test"`, no tag, created at the
        // epoch), so a case's result never depends on the deployment;
        // `stub Version.current() returns …` gives another.
        ("bynk.cloudflare", "Version") => "  return {\n    async current() {\n      return { id: \"test\", tag: None, deployed: 0 } as never;\n    },\n  };\n".to_string(),
        // #3478: a test never reaches Workers AI, so every operation faults
        // until stubbed. There is no fake model: a fabricated reply would make
        // a test pass for the wrong reason.
        ("bynk.cloudflare", "Ai") => {
            let op = |op: &str, args: &str| {
                format!(
                    "    async {op}() {{\n      throw new globalThis.Error(\"bynk: `Ai.{op}` is not stubbed, and a test never reaches Workers AI — add `stub Ai.{op}({args}) returns …`\");\n    }},\n"
                )
            };
            format!(
                "  return {{\n{}{}{}{}  }};\n",
                op("run", "_, _"),
                op("generate", "_, _"),
                op("embed", "_, _"),
                op("runStream", "_, _")
            )
        }
        // #3515: a test never reaches Cloudflare Images, so every operation
        // faults until stubbed. There is no fake transform: an image returned
        // unchanged would make a test pass for the wrong reason.
        ("bynk.cloudflare", "Images") => {
            let op = |op: &str, args: &str| {
                format!(
                    "    async {op}() {{\n      throw new globalThis.Error(\"bynk: `Images.{op}` is not stubbed, and a test never reaches Cloudflare Images — add `stub Images.{op}({args}) returns …`\");\n    }},\n"
                )
            };
            format!(
                "  return {{\n{}{}{}  }};\n",
                op("info", "_"),
                op("transform", "_, _, _"),
                op("transformStream", "_, _, _")
            )
        }
        // #3513 B: a test never reads the account's datasets, so `query` faults until it is
        // stubbed. A fabricated result set would pass for the wrong reason.
        ("bynk.cloudflare", "AnalyticsSql") => "  return {\n    async query() {\n      throw new globalThis.Error(\"bynk: `AnalyticsSql.query` is not stubbed, and a test never reads Analytics Engine — add `stub AnalyticsSql.query(_, _) returns …`\");\n    },\n  };\n".to_string(),
        // #3479: the real `WorkersVectorize` over an exact in-memory index built
        // from the instance's declaration, so a retrieval flow (store, filter,
        // rank) runs in a test. Ranking is exact and a write is visible at once,
        // which production's approximate, eventually consistent index is not.
        // `stub Docs.query(_, _) returns …` still replaces an operation.
        ("bynk.cloudflare", "Vectorize") => {
            let params = vec![
                TsParam {
                    default: Some(num_lit("1")),
                    ..TsParam::new("dimensions", Some(TsType::named("number")))
                },
                TsParam {
                    default: Some(str_lit("cosine")),
                    ..TsParam::new("metric", Some(TsType::named("string")))
                },
                TsParam {
                    default: Some(TsExpr::array(vec![])),
                    ..TsParam::new(
                        "indexed",
                        Some(TsType::Array {
                            element: Box::new(TsType::named("string")),
                            readonly: true,
                        }),
                    )
                },
            ];
            let body = "  return new bynk_cloudflare__binding.WorkersVectorize({ INDEX: __makeIntegrationVectorize(dimensions, metric, indexed) }, \"INDEX\");\n".to_string();
            return Some(double_fn(name, ns, cap, params, body, &double_uses(owner, cap, harness_clock, false)));
        }
        // #3480: a test never reaches AI Search, so each instance faults until
        // its operation is stubbed (no double could keep the service's
        // contract: chunking, embeddings and ranking are the service).
        ("bynk.cloudflare", "AiSearch") => {
            let op = |op: &str| {
                format!(
                    "    async {op}() {{\n      throw new globalThis.Error(\"bynk: `{name}.{op}` is not stubbed, and a test never reaches AI Search — add `stub {name}.{op}(_, _) returns …`\");\n    }},\n"
                )
            };
            format!("  return {{\n{}{}  }};\n", op("search"), op("answer"))
        }
        // #3513 A: a test never presents a certificate to an origin, so each instance faults
        // until its `send` is stubbed. A fake origin would pass cases the real handshake fails.
        ("bynk.cloudflare", "MtlsCertificate") => {
            let op = |op: &str, args: &str| {
                format!(
                    "    async {op}() {{\n      throw new globalThis.Error(\"bynk: `{name}.{op}` is not stubbed, and a test never reaches the origin — add `stub {name}.{op}({args}) returns …`\");\n    }},\n"
                )
            };
            format!(
                "  return {{\n{}{}{}  }};\n",
                op("send", "_"),
                op("receiveBytes", "_"),
                op("sendBytes", "_, _")
            )
        }
        // #3477: a test never starts a container, so each operation faults,
        // naming the instance it was asked for, until it is stubbed. There is
        // no in-memory echo: it would pass cases a real image fails.
        ("bynk.cloudflare", "Container") => {
            let op = |op: &str, args: &str| {
                format!(
                    "    async {op}(instance: string) {{\n      throw new globalThis.Error(`bynk: \\`{name}.{op}\\` on instance ${{JSON.stringify(instance)}} is not stubbed, and a test never starts a container — add \\`stub {name}.{op}({args}) returns …\\``);\n    }},\n"
                )
            };
            format!(
                "  return {{\n{}{}{}  }};\n",
                op("send", "_, _"),
                op("receiveBytes", "_, _"),
                op("sendBytes", "_, _, _")
            )
        }
        // #3301: a test never reads the account's Secrets Store, so each
        // instance faults until its `get` is stubbed. The message names the
        // instance and never a value: there is none to name.
        ("bynk.cloudflare", "StoredSecret") => format!("  return {{\n    async get() {{\n      throw new globalThis.Error(\"bynk: `{name}.get` is not stubbed, and a test never reads the Secrets Store — add `stub {name}.get() returns …`\");\n    }},\n  }};\n"),
        // Interop I5 (#1972): like `Fetch`, a test never reaches the foreign
        // Worker, so each instance faults until its `send` is stubbed.
        ("bynk.cloudflare", "Service") => format!("  return {{\n    async send() {{\n      throw new globalThis.Error(\"bynk: `{name}.send` is not stubbed, and a test never reaches the Worker behind it — add `stub {name}.send(_) returns …`\");\n    }},\n  }};\n"),
        _ => return None,
    };
    let pglite = d1_migrations.is_some();
    Some(double_fn(
        name,
        ns,
        cap,
        Vec::new(),
        body,
        &double_uses(owner, cap, harness_clock, pglite),
    ))
}

/// A platform double as a function declaration: `function __bynkTest_<name>(params): <ns>.<cap> {
/// body }`. The body is one authored text statement (a fixed object literal or a call into the
/// capability's real binding, with named substitutions), carrying the runtime and sibling names it
/// spells. It stays text because it is test-double code written to be read as TypeScript
/// (#4054 Decision A).
fn double_fn(
    name: &str,
    ns: &str,
    cap: &str,
    params: Vec<TsParam>,
    body: String,
    uses: &[&str],
) -> TsStmt {
    TsStmt::decl(
        bynk_ts::TsDecl::Function {
            name: format!("__bynkTest_{name}"),
            generics: Vec::new(),
            params,
            return_type: Some(TsType::named(format!("{ns}.{cap}"))),
            body: vec![TsStmt::raw_using(body, uses)],
            is_async: false,
            inline: false,
        },
        None,
    )
}

/// The runtime and sibling names a platform double's body spells, by what the body of that
/// capability's double names: the result variants it returns, the harness clock it reads, and the
/// binding or integration factory it calls.
fn double_uses(owner: &str, cap: &str, harness_clock: bool, pglite: bool) -> Vec<&'static str> {
    let clock = |mut v: Vec<&'static str>| {
        if harness_clock {
            v.push("__bynkClockNow");
        }
        v
    };
    match (owner, cap) {
        ("bynk", "Clock") if harness_clock => vec!["__bynkClockNow", "__bynkClock"],
        ("bynk", "Secrets") | ("bynk.cloudflare", "Version") => vec!["None"],
        ("bynk", "Idempotency") => clock(Vec::new()),
        ("bynk.cloudflare", "Kv") => clock(vec!["None", "Some"]),
        ("bynk.cloudflare", "R2") => vec!["None", "Some"],
        ("bynk.cloudflare", "RateLimit") => clock(Vec::new()),
        ("bynk.crypto", "Crypto") => vec!["bynk_crypto__binding"],
        ("bynk.testing", "TypedReads") => vec!["bynk_testing__binding"],
        ("bynk.cloudflare", "D1") => vec!["bynk_cloudflare__binding", "__makeIntegrationD1"],
        ("bynk.cloudflare", "Email") => {
            vec!["bynk_cloudflare__binding", "__makeIntegrationEmail"]
        }
        ("bynk.cloudflare", "Postgres") if pglite => {
            vec!["bynk_cloudflare__binding", "__makeIntegrationPostgres"]
        }
        ("bynk.cloudflare", "Vectorize") => {
            vec!["bynk_cloudflare__binding", "__makeIntegrationVectorize"]
        }
        _ => Vec::new(),
    }
}

/// #2134: the `Secrets` test double of a module whose cases run with `env`
/// entries: it answers `Some(value)` for a name the running case's `env`
/// gives (`__bynkEnv`), and `None` for any other, as [`platform_double`]'s
/// does for every name. `None` for any other capability.
pub(super) fn env_secrets_double(owner: &str, cap: &str, ns: &str, name: &str) -> Option<TsStmt> {
    if (owner, cap) != ("bynk", "Secrets") {
        return None;
    }
    Some(double_fn(
        name,
        ns,
        cap,
        Vec::new(),
        "  return {\n    async get(key) {\n      const v = __bynkEnv[key];\n      return v === undefined ? None : Some(v);\n    },\n  };\n".to_string(),
        &["None", "Some", "__bynkEnv"],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// The `(owner, capability)` pairs [`platform_double`] has a double for.
    const DOUBLES: &[(&str, &str)] = &[
        ("bynk", "Logger"),
        ("bynk", "Events"),
        ("bynk", "Clock"),
        ("bynk", "Random"),
        ("bynk.crypto", "Crypto"),
        ("bynk.testing", "TypedReads"),
        ("bynk", "Secrets"),
        ("bynk", "Locale"),
        ("bynk", "Fetch"),
        ("bynk", "Idempotency"),
        ("bynk.cloudflare", "Kv"),
        ("bynk.cloudflare", "R2"),
        ("bynk.cloudflare", "RateLimit"),
        ("bynk.cloudflare", "EdgeCache"),
        ("bynk.cloudflare", "Queue"),
        ("bynk.cloudflare", "Analytics"),
        ("bynk.cloudflare", "D1"),
        ("bynk.cloudflare", "Email"),
        ("bynk.cloudflare", "Postgres"),
        ("bynk.cloudflare", "MySql"),
        ("bynk.cloudflare", "Version"),
        ("bynk.cloudflare", "Ai"),
        ("bynk.cloudflare", "Images"),
        ("bynk.cloudflare", "AnalyticsSql"),
        ("bynk.cloudflare", "Vectorize"),
        ("bynk.cloudflare", "AiSearch"),
        ("bynk.cloudflare", "MtlsCertificate"),
        ("bynk.cloudflare", "Container"),
        ("bynk.cloudflare", "StoredSecret"),
        ("bynk.cloudflare", "Service"),
    ];

    /// The runtime and sibling names a double's text spells: the result variants, `__bynk…` and
    /// `__make…` helpers and `…__binding` namespaces, outside strings, templates and comments.
    fn spelled_names(text: &str) -> BTreeSet<String> {
        let bytes = text.as_bytes();
        let mut names = BTreeSet::new();
        let mut i = 0;
        while i < bytes.len() {
            match bytes[i] {
                b'/' if bytes.get(i + 1) == Some(&b'/') => {
                    while i < bytes.len() && bytes[i] != b'\n' {
                        i += 1;
                    }
                }
                q @ (b'"' | b'`') => {
                    i += 1;
                    while i < bytes.len() && bytes[i] != q {
                        i += if bytes[i] == b'\\' { 2 } else { 1 };
                    }
                    i += 1;
                }
                c if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {
                    let start = i;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_alphanumeric()
                            || bytes[i] == b'_'
                            || bytes[i] == b'$')
                    {
                        i += 1;
                    }
                    let word = &text[start..i];
                    if matches!(word, "Some" | "None" | "Ok" | "Err")
                        || word.starts_with("__bynk")
                        || word.starts_with("__make")
                        || word.ends_with("__binding")
                    {
                        names.insert(word.to_string());
                    }
                }
                _ => i += 1,
            }
        }
        names
    }

    /// A double's declared names are the names its body spells. The list is written beside the
    /// body text, so this is what stops an edit to one from leaving the other behind.
    #[test]
    fn a_platform_double_declares_the_names_its_body_spells() {
        let no_files: &[MigrationFile] = &[];
        for &(owner, cap) in DOUBLES {
            for harness_clock in [false, true] {
                for pglite in [false, true] {
                    let migrations = pglite.then_some(no_files);
                    let def = platform_double(owner, cap, "ns", "X", harness_clock, migrations)
                        .unwrap_or_else(|| panic!("no double for {owner}.{cap}"));
                    let printed = bynk_ts::print_stmt(&def, 0);
                    // Drop the declaration's own header line, which names `__bynkTest_X`.
                    let body = printed.split_once('\n').map_or("", |(_, rest)| rest);
                    let declared: BTreeSet<String> = double_uses(owner, cap, harness_clock, pglite)
                        .into_iter()
                        .map(str::to_string)
                        .collect();
                    assert_eq!(
                        spelled_names(body),
                        declared,
                        "{owner}.{cap} (clock {harness_clock}, pglite {pglite})"
                    );
                }
            }
        }
    }

    /// The `Secrets` double that answers from a case's `env` declares what it spells.
    #[test]
    fn the_env_secrets_double_declares_the_names_it_spells() {
        let def = env_secrets_double("bynk", "Secrets", "ns", "X").expect("a Secrets double");
        let printed = bynk_ts::print_stmt(&def, 0);
        let body = printed.split_once('\n').map_or("", |(_, rest)| rest);
        let declared: BTreeSet<String> = ["None", "Some", "__bynkEnv"]
            .into_iter()
            .map(str::to_string)
            .collect();
        assert_eq!(spelled_names(body), declared);
    }
}
