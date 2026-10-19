//! Test/integration-suite checking (P5.4,
//! `design/tracks/semantics-in-the-checker.md` §6) — closes category 7 of
//! `bynk-check/src/analysis.rs`'s own residual-gap accounting, the last of
//! the seven. `bynk-emit/src/project/tests_emit.rs` held
//! `process_tests`/`process_integration_tests`, real production code (not
//! fixture noise, despite the filename) checking + emitting `suite`/`test
//! integration` bodies — but it ran only inside `bynk-emit::run_checks`
//! (`Mode::Analyse` included), never inside `bynk_check::analysis::analyse_project`,
//! the entry point the LSP now uses. That gap meant no diagnostics *and* no
//! `RefSink` bindings (go-to-definition/find-references) for anything inside
//! a test file, in the editor — see `analysis.rs`'s own module doc for the
//! full accounting this closes.
//!
//! **What moved here** — every check-only helper `process_tests`/
//! `process_integration_tests` used, plus every function that is genuinely
//! **dual-use**: called both by this module's own [`phase_test_bodies`]/
//! [`phase_integration_bodies`] (the checking half, real diagnostic/`RefSink`
//! sinks) *and* by `bynk-emit`'s TypeScript lowering (throwaway sinks, needed
//! only for the resolved-type view a body's emission depends on). Dual-use
//! functions are `pub`, and `bynk-emit` calls them qualified
//! (`bynk_check::test_suites::foo(...)`) rather than duplicating them — see
//! [`build_privileged_resolved`], [`typecheck_case_body`],
//! [`check_history_binding`], [`register_call_record_types`],
//! [`history_handlers`], [`history_variant_name`], [`prop_binding_generable`]
//! and [`infer_participants`] for which and why (each names its own emit-side
//! call sites). Duplicating a dual-use function instead of relocating it is
//! exactly the drift risk this whole design track exists to close (§9,
//! "Relocating checks risks a quiet R4.6/R4.11 regression").
//!
//! **What stayed in `bynk-emit`** (pure TypeScript emission, or verified
//! emit-only by call-site count): `block_uses_observation`,
//! `target_service_handler_kinds`, `is_attackable_contract`,
//! `numeric_or_scalar_base`, `attackable_contracts`,
//! `json_codec_qual_for_target`, `prop_history_binding`, `prop_is_history`,
//! `SystemCaseInput`, `RunnableTest`, `discovered_location`,
//! `discovery_manifest`, `sanitise_suite`, `emit_integration_module` and its
//! http-driver/harness helpers, and the ~2,600-line TypeScript-codegen tail
//! starting at `emit_test_module` (`emit_stub_class`, `gen_ts_for_ty`,
//! `emit_test_property_function`, `emit_test_history_property_function`, and
//! the rest).
//!
//! `bynk-emit/src/project/tests_emit.rs`'s own `process_tests`/
//! `process_integration_tests` keep their exact signatures (`run_checks`'s
//! callers need no change) — their bodies now call
//! [`phase_test_bodies`]/[`phase_integration_bodies`] for the checking half,
//! then proceed to their existing, unmoved Phase-5 emission logic using the
//! "ready for emission" data these return.
//!
//! `bynk-emit` depends on `bynk-check` (a production dependency, never the
//! reverse), so this move has no circular-dependency subtlety to solve —
//! unlike P5.3's `phase_platform_lock`, which needed a from-scratch pure
//! reimplementation because its old home reached into a `bynk-emit`
//! TypeScript-codegen helper. This is a plain code-motion job, just a large
//! one.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::checker::{self, Types};
use crate::context_checks::{build_capability_op_info, ts_type_ref_display};
use crate::hints::HintSink;
use crate::index::{RefSink, SymbolKind};
use crate::locals::LocalsSink;
use crate::requirements::RequirementSink;
use crate::resolver::{self, MethodTable as ResolverMethodTable, ResolvedCommons};
use crate::symbols::{UnitTable, build_cross_context_info};
use bynk_project::ParsedFile;
use bynk_project::UnitKind;
use bynk_project::discovery::{case_effective_tier, suite_effective_tier_is_system};
use bynk_syntax::ast::*;
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// Which runs of a target's test module a `stub` clause applies to (#291).
/// Every suite targeting a unit is emitted into one module, so a clause's
/// scope is narrowed to its own suite whenever another suite shares the
/// module (#1860).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StubScope {
    /// Every run: a suite-scoped clause of the target's only suite.
    Every,
    /// The run of the case of this name (a case-scoped clause). Case names
    /// are unique across a target's suites (`bynk.suite.duplicate_case_name`).
    Case(String),
    /// Every run of the suite with this ordinal among the target's suites
    /// ([`suite_ordinal`]): its cases and its properties. A suite-scoped
    /// clause, when the target has more than one suite.
    Suite(usize),
}

/// #1860: the ordinal of the suite at `parsed[i]` among the suites in
/// `indices` (each index whose file is a suite), or `None` when `i` is not
/// one. [`StubScope::Suite`] is keyed by it, so emission computes it the same
/// way for each run.
pub fn suite_ordinal(i: usize, indices: &[usize], parsed: &[ParsedFile]) -> Option<usize> {
    indices
        .iter()
        .filter(|&&j| parsed[j].test().is_some())
        .position(|&j| j == i)
}

/// v0.118: a capability seam with one or more `stub` overrides applied
/// (testing track slice 6). Groups every `stub Cap.method(…)` clause — both
/// suite-scoped and case-scoped — targeting the same capability `cap`. The
/// resolved [`CapabilityDecl`] supplies each overridden method's parameter names
/// and return type for stub emission.
#[derive(Debug, Clone)]
pub struct ResolvedStub {
    /// The capability being overridden (a declared/consumed seam of the target).
    pub cap: String,
    /// The capability declaration, for op parameter names and return types.
    pub cap_decl: CapabilityDecl,
    /// The `stub` clauses for this capability, in match order (case-scoped
    /// first so they take precedence over suite-scoped in the emitted if-chain).
    pub clauses: Vec<StubClause>,
    /// #291: parallel to `clauses` — which runs each clause applies to.
    pub clause_scopes: Vec<StubScope>,
    /// The test file declaring the first clause — the recording context for
    /// edges in its value expressions (v0.25).
    ///
    /// ADR 0198/0201: a *recording context* is an index key, so this is the
    /// file's **identity** (project-relative), not its `include`-root-relative
    /// unit path. Everything the index keys must name a file the round
    /// analysed.
    pub identity_path: PathBuf,
}

/// P5.4 (`design/tracks/semantics-in-the-checker.md` §6): the checking half
/// of `test <target>` suite processing — target resolution, duplicate-case-
/// name detection, `stub`-clause resolution, and case/property body
/// type-checking. Formerly Phases 2-4 of `bynk-emit`'s own `process_tests`;
/// Phase 5 (TypeScript emission) stays in
/// `bynk-emit::project::tests_emit::process_tests`, which calls this
/// function for its checking half and then emits only for the targets this
/// returns — every target this function resolves, has no duplicate case
/// names, and whose bodies type-check clean is exactly "ready for
/// emission". `bynk_check::analysis::analyse_project` calls this too and
/// discards the returned map — it never emits, so only the diagnostic/
/// `RefSink` side effects matter there. Closes category 7 of
/// `bynk-check/src/analysis.rs`'s own residual-gap accounting, alongside
/// [`phase_integration_bodies`].
#[allow(clippy::too_many_arguments)]
pub fn phase_test_bodies(
    test_groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    exports_visibility: &HashMap<String, HashMap<String, Visibility>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_uses: &HashMap<String, Vec<String>>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) -> HashMap<String, HashMap<String, ResolvedStub>> {
    let mut ready: HashMap<String, HashMap<String, ResolvedStub>> = HashMap::new();

    let mut sorted_targets: Vec<&String> = test_groups.keys().collect();
    sorted_targets.sort();

    for target_name in sorted_targets {
        let indices = test_groups.get(target_name).unwrap();
        // -- Phase 2: target resolution --
        let target_kind = match kinds.get(target_name) {
            Some(k) => *k,
            None => {
                let span = first_test_target_span(indices, parsed);
                errors.push(
                    CompileError::new(
                        "bynk.suite.unknown_target",
                        span,
                        format!(
                            "test target `{target_name}` is not a declared commons or context in this project",
                        ),
                    )
                    .with_note(
                        "the target of a `test` declaration must be a commons or context declared elsewhere in the project",
                    ),
                );
                continue;
            }
        };

        // -- Phase 2: duplicate test case names --
        let mut seen_cases: HashMap<String, Span> = HashMap::new();
        let mut had_dup = false;
        for &i in indices {
            if let Some(t) = parsed[i].test() {
                for case in &t.cases {
                    if let Some(prev) = seen_cases.get(&case.name) {
                        had_dup = true;
                        errors.push(
                            CompileError::new(
                                "bynk.suite.duplicate_case_name",
                                case.name_span,
                                format!(
                                    "test case `\"{}\"` is declared more than once in tests targeting `{target_name}`",
                                    case.name
                                ),
                            )
                            .with_label(*prev, "previously declared here"),
                        );
                    } else {
                        seen_cases.insert(case.name.clone(), case.name_span);
                    }
                }
            }
        }

        // -- #2134: each suite's and case's `env` clauses. --
        for &i in indices {
            if let Some(t) = parsed[i].test() {
                crate::test_env::check_env_clauses(t, errors);
            }
        }

        // -- Phase 3: resolve `stub` clauses (v0.118, testing track slice 6).
        // Both suite-scoped and case-scoped `stub` fold into one per-seam
        // override map. Case-scoped clauses are collected first so they take
        // precedence over suite-scoped ones in the emitted first-match if-chain
        // (the case > suite > default order). Each clause applies only to its
        // own case's run, or its own suite's cases (#291, #1860). Runs
        // unconditionally, even when `had_dup` — its own diagnostics still
        // fire, matching `process_tests`'s original Phase 2/3 ordering.
        let target_stubs = resolve_stubs(
            target_name,
            target_kind,
            indices,
            parsed,
            unit_tables,
            unit_consumes,
            errors,
        );

        if had_dup {
            // Skip body/type-checking for this target; we have name conflicts.
            continue;
        }

        // -- Phase 4: type-check bodies. --
        // (We build a resolved view targeting either commons or context;
        // mock bodies are type-checked with the mocked entity's privileges.)
        let bodies_errs = check_test_bodies(
            target_name,
            target_kind,
            indices,
            parsed,
            &target_stubs,
            unit_tables,
            exports_visibility,
            unit_consumes,
            unit_consumes_aliases,
            unit_uses,
            refs,
            tys,
        );
        let bodies_failed = !bodies_errs.is_empty();
        errors.extend(bodies_errs);

        if bodies_failed {
            continue;
        }

        ready.insert(target_name.clone(), target_stubs);
    }

    ready
}

/// v0.118: resolve every `stub Cap.method(…)` clause targeting a unit into a
/// per-capability [`ResolvedStub`] (testing track slice 6, ADR 0154). Both
/// suite-scoped and case-scoped clauses fold in; a capability that is neither a
/// declared seam of the target nor reachable through a consumed context is
/// `bynk.stub.not_a_seam`, an unknown method is `bynk.stub.unknown_op`,
/// and an empty `returns each []` is `bynk.stub.bad_sequence`.
fn resolve_stubs(
    target_name: &str,
    target_kind: UnitKind,
    indices: &[usize],
    parsed: &[ParsedFile],
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    errors: &mut Vec<CompileError>,
) -> HashMap<String, ResolvedStub> {
    let target_table = unit_tables.get(target_name);
    let target_consumed = unit_consumes.get(target_name).cloned().unwrap_or_default();

    // Collect clauses tagged with the declaring file and the cases each
    // applies to. Case-scoped first so they precede suite-scoped clauses in
    // each capability's match order.
    let mut collected: Vec<(StubClause, StubScope, PathBuf)> = Vec::new();
    for &i in indices {
        let Some(t) = parsed[i].test() else { continue };
        for case in &t.cases {
            for pc in &case.stubs {
                collected.push((
                    pc.clone(),
                    StubScope::Case(case.name.clone()),
                    parsed[i].identity_path(),
                ));
            }
        }
    }
    // #1860: a suite-scoped clause applies to its own suite's runs, which
    // matters only when another suite targets the same unit: all of them are
    // emitted into one module.
    let suites = indices
        .iter()
        .filter(|&&i| parsed[i].test().is_some())
        .count();
    for &i in indices {
        let Some(t) = parsed[i].test() else { continue };
        let scope = match suite_ordinal(i, indices, parsed) {
            Some(n) if suites > 1 => StubScope::Suite(n),
            _ => StubScope::Every,
        };
        for pc in &t.stubs {
            collected.push((pc.clone(), scope.clone(), parsed[i].identity_path()));
        }
    }

    // Resolve a capability name to its declaration: a capability the target
    // declares (or has flattened in via `consumes U { Cap }`), else a capability
    // of a consumed context.
    let resolve_cap = |name: &str| -> Option<CapabilityDecl> {
        target_table
            .and_then(|t| t.capabilities.get(name).cloned())
            .or_else(|| {
                target_consumed.iter().find_map(|q| {
                    unit_tables
                        .get(q)
                        .and_then(|t| t.capabilities.get(name).cloned())
                })
            })
    };

    let mut out: HashMap<String, ResolvedStub> = HashMap::new();
    for (pc, scope, identity_path) in collected {
        let cap_name = pc.capability.name.clone();
        let Some(cap_decl) = resolve_cap(&cap_name) else {
            // Commons have no seams at all; contexts may still name a
            // non-existent capability. Either way it is not a seam.
            let note = if target_kind == UnitKind::Commons {
                "commons have no capability seams — `stub` overrides a capability the target context declares or consumes"
            } else {
                "a `stub` clause names a capability the target context declares or reaches through a consumed context"
            };
            errors.push(
                CompileError::new(
                    "bynk.stub.not_a_seam",
                    pc.capability.span,
                    format!("`{cap_name}` is not a capability seam of `{target_name}`",),
                )
                .with_note(note),
            );
            continue;
        };
        let Some(op_decl) = cap_decl.ops.iter().find(|o| o.name.name == pc.method.name) else {
            errors.push(CompileError::new(
                "bynk.stub.unknown_op",
                pc.method.span,
                format!(
                    "`{}` is not an operation of capability `{cap_name}`",
                    pc.method.name
                ),
            ));
            continue;
        };
        // #926 (Decision F): a generic capability operation cannot be stubbed
        // — `__Stub_Cap`'s per-op method body has no way to construct a
        // value of the op's unconstrained `T`. Deferred rather than
        // supported: the stub class carries no `implements` clause (its
        // members are duck-typed through an untyped `deps` seam), so
        // stubbing another, non-generic op of the same capability keeps
        // type-checking.
        if !op_decl.type_params.is_empty() {
            errors.push(
                CompileError::new(
                    "bynk.stub.generic_op",
                    pc.method.span,
                    format!(
                        "`{cap_name}.{}` declares its own type parameter — a generic capability operation cannot be stubbed at v1",
                        pc.method.name
                    ),
                )
                .with_note(
                    "test through the capability's real (external) provider instead, or restructure the test to avoid stubbing this operation",
                ),
            );
            continue;
        }
        if let StubRhs::ReturnsEach(outcomes, span) = &pc.rhs
            && outcomes.is_empty()
        {
            errors.push(CompileError::new(
                "bynk.stub.bad_sequence",
                *span,
                format!(
                    "`stub {cap_name}.{} returns each []` has no outcomes — a sequence needs at least one",
                    pc.method.name
                ),
            ));
            continue;
        }
        let entry = out.entry(cap_name.clone()).or_insert_with(|| ResolvedStub {
            cap: cap_name.clone(),
            cap_decl: cap_decl.clone(),
            clauses: Vec::new(),
            clause_scopes: Vec::new(),
            identity_path: identity_path.clone(),
        });
        entry.clauses.push(pc);
        entry.clause_scopes.push(scope);
    }
    out
}

/// v0.118: infer a `system`-tier suite's wired participants — the target's
/// transitive `consumes` closure (testing track slice 6). A BFS from the target
/// following `consumes` edges; the returned list starts with the target and
/// includes every context reachable through it (deterministic breadth order).
/// #1856: contexts only. A unit consumed for its capabilities alone
/// (`consumes bynk { Clock }`, an adapter) is provided in-process by the
/// composition root: no Worker, no Service Binding, no wire to cross. The
/// walk still passes through one, but it is no participant.
pub fn infer_participants(
    target: &str,
    unit_consumes: &HashMap<String, Vec<String>>,
    kinds: &BTreeMap<String, UnitKind>,
) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut queue: Vec<String> = vec![target.to_string()];
    seen.insert(target.to_string());
    let mut head = 0;
    while head < queue.len() {
        let node = queue[head].clone();
        head += 1;
        order.push(node.clone());
        if let Some(deps) = unit_consumes.get(&node) {
            for d in deps {
                if seen.insert(d.clone()) {
                    queue.push(d.clone());
                }
            }
        }
    }
    order.retain(|u| kinds.get(u) == Some(&UnitKind::Context));
    order
}

/// #1959: the contexts a `system`-tier suite's harness stands up as Workers:
/// [`infer_participants`]' set, plus every context subscribing to an event
/// one of them declares, plus *its* `consumes` closure, transitively. A
/// subscriber consumes its publisher, and `consumes` cycles are refused, so
/// no subscriber of the target is ever in the target's own closure; without
/// it here, the fan-out would deliver to nobody. Only the harness widens: a
/// case still reaches just [`infer_participants`]' set. Target first, then
/// breadth order.
pub fn infer_harness_participants(
    target: &str,
    unit_consumes: &HashMap<String, Vec<String>>,
    kinds: &BTreeMap<String, UnitKind>,
    event_subscribers: &BTreeMap<(String, String), Vec<(String, String)>>,
) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    let mut order: Vec<String> = Vec::new();
    let mut queue: Vec<String> = vec![target.to_string()];
    seen.insert(target.to_string());
    let mut head = 0;
    while head < queue.len() {
        let node = queue[head].clone();
        head += 1;
        order.push(node.clone());
        let consumed = unit_consumes.get(&node).into_iter().flatten();
        let subscribers = event_subscribers
            .iter()
            .filter(|((owner, _), _)| *owner == node)
            .flat_map(|(_, subs)| subs.iter().map(|(ctx, _)| ctx));
        for next in consumed.chain(subscribers) {
            if seen.insert(next.clone()) {
                queue.push(next.clone());
            }
        }
    }
    order.retain(|u| kinds.get(u) == Some(&UnitKind::Context));
    order
}

/// P5.4 (`design/tracks/semantics-in-the-checker.md` §6): the checking half
/// of `test integration "name"` suite processing — participant inference,
/// the `system`-needs-a-serialisation-edge gate, duplicate-case-name
/// detection, the harness-root cross-context view, and per-case body
/// type-checking (including the `Wire`/`by Nobody` tier gates). Formerly the
/// pre-emission logic of `bynk-emit`'s own `process_integration_tests`;
/// emission stays in `bynk-emit::project::tests_emit::process_integration_tests`,
/// which calls this function for its checking half and then emits only for
/// the groups this returns. Unlike [`phase_test_bodies`]'s `ResolvedStub`
/// map, the only thing worth handing back here is the harness's
/// [`resolver::CrossContextInfo`] — it's built from clone-heavy maps
/// (`harness_consumes`/`harness_uses`), so recomputing it a second time on
/// the emit side would be wasted work. `participants`/`uses_targets`/
/// `case_inputs` are cheap and pure (a BFS, a linear scan), so the emit-side
/// loop recomputes those itself from `parsed`/`unit_consumes`, using the
/// now-relocated [`infer_participants`]. `bynk_check::analysis::analyse_project`
/// calls this too and discards the returned map — it never emits. Closes
/// category 7 of `bynk-check/src/analysis.rs`'s own residual-gap accounting,
/// alongside [`phase_test_bodies`].
#[allow(clippy::too_many_arguments)]
pub fn phase_integration_bodies(
    integration_groups: &BTreeMap<String, Vec<usize>>,
    parsed: &[ParsedFile],
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_uses: &HashMap<String, Vec<String>>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) -> HashMap<String, resolver::CrossContextInfo> {
    let mut ready: HashMap<String, resolver::CrossContextInfo> = HashMap::new();

    let mut sorted: Vec<&String> = integration_groups.keys().collect();
    sorted.sort();

    for group_name in sorted {
        let indices = integration_groups.get(group_name).unwrap();
        let first = indices[0];
        let Some(decl) = parsed[first].integration() else {
            continue;
        };
        // v0.118: there is no `suite` string any more — the wired suite is named
        // for its target context. The participant set is INFERRED from the
        // target's transitive `consumes` closure (no `wires` list).
        let suite_target = decl.target.joined();
        let participants = infer_participants(&suite_target, unit_consumes, kinds);

        let mut bad = false;

        // v0.118 / testing-the-boundary Slice B: a `system` suite needs a real
        // **serialisation edge** — not merely ≥ 2 participants. The original rule
        // (`participants.len() < 2`) was a proxy for "nothing to serialise
        // across", exact only when the sole edge was cross-context. A single
        // context that exposes an `http` service has a real edge (the public
        // boundary: deserialise → handler → serialise), so it qualifies.
        //
        // `http` and `queue` are admitted, because the system driver wires both:
        // `emit_system_http_support` drives `worker.fetch`, and
        // `emit_system_job_support` (#2136) drives `worker.queue` with a batch
        // whose message body has been through the platform's JSON
        // serialisation. `cron` never qualifies on its own — a scheduled event
        // carries only a cron string and a time, so there is nothing to
        // serialise — but a cron address is legal in a suite that already
        // qualifies, and is driven through `worker.scheduled` there.
        let has_serialisation_edge = unit_tables.get(&suite_target).is_some_and(|t| {
            t.services.values().any(|s| {
                matches!(
                    s.protocol,
                    bynk_syntax::ast::ServiceProtocol::Http
                        | bynk_syntax::ast::ServiceProtocol::Queue { .. }
                )
            })
        });
        if participants.len() < 2 && !has_serialisation_edge {
            errors.push(
                CompileError::new(
                    "bynk.tier.system_needs_wire",
                    decl.target.span,
                    format!(
                        "`system`-tier suite for `{suite_target}` has no serialisation edge — the target consumes no other context and exposes no `http` or `queue` service",
                    ),
                )
                .with_note(
                    "a `system` case crosses a real serialise → JSON → deserialise boundary; this target has none to cross, so `unit` already covers it",
                ),
            );
            bad = true;
        }

        // -- Duplicate case names within the suite. --
        let mut seen_cases: HashMap<String, Span> = HashMap::new();
        for &i in indices {
            let Some(d) = parsed[i].integration() else {
                continue;
            };
            for case in &d.cases {
                if let Some(prev) = seen_cases.get(&case.name) {
                    errors.push(
                        CompileError::new(
                            "bynk.suite.duplicate_case_name",
                            case.name_span,
                            format!(
                                "test case `\"{}\"` is declared more than once in tests targeting `{suite_target}`",
                                case.name
                            ),
                        )
                        .with_label(*prev, "previously declared here"),
                    );
                    bad = true;
                } else {
                    seen_cases.insert(case.name.clone(), case.name_span);
                }
            }
        }

        // -- #2134: each suite's and case's `env` clauses. --
        for &i in indices {
            if let Some(d) = parsed[i].integration() {
                crate::test_env::check_env_clauses(d, errors);
            }
        }

        // -- #1738: a `system` case can't share a suite with lower-tier ones. --
        // Reported without marking the suite `bad`: the per-case tier gates
        // below (`Wire`, `by Nobody`) still check its bodies, and any error
        // already stops emission.
        // A suite with any `system` case is emitted, whole, as the wired module
        // that drives deployed Workers, where a service is addressed by its
        // context path (`shop.orders.place(…)`), not `place.call(…)`. A lower-
        // tier case there has no in-process target to call (it lowered to
        // `/* unknown */`), so the tiers are kept to separate suites.
        for &i in indices {
            let Some(d) = parsed[i].integration() else {
                continue;
            };
            let suite_is_system = d.tier == Some(bynk_syntax::ast::TestTier::System);
            let has_lower = d
                .cases
                .iter()
                .any(|c| case_effective_tier(c, d) != bynk_syntax::ast::TestTier::System);
            if !has_lower {
                continue;
            }
            for case in &d.cases {
                let tier = case_effective_tier(case, d);
                // In a `system` suite, flag each case that steps down; in any
                // other suite, flag each case that steps up to `system`.
                let flagged = if suite_is_system {
                    tier != bynk_syntax::ast::TestTier::System
                } else {
                    case.tier == Some(bynk_syntax::ast::TestTier::System)
                };
                if !flagged {
                    continue;
                }
                let (message, advice) = if suite_is_system {
                    (
                        format!(
                            "case `\"{}\"` is `{}`-tier, but its suite is `as system`",
                            case.name,
                            tier.as_str()
                        ),
                        format!(
                            "move this case into a separate `suite {suite_target}` without `as system`"
                        ),
                    )
                } else {
                    (
                        format!(
                            "case `\"{}\"` is `as system`, but other cases in its suite run below `system`",
                            case.name
                        ),
                        format!(
                            "move this case into its own `suite {suite_target} as system {{ … }}`, addressing services by context path (`{suite_target}.<service>(…)`)"
                        ),
                    )
                };
                errors.push(
                    CompileError::new("bynk.tier.mixed_system_suite", case.name_span, message)
                        .with_note(format!(
                            "a `system` case runs against deployed Workers, so its suite holds `system` cases only; {advice}"
                        )),
                );
            }
        }

        // -- #2026: a `stub` has nothing to override at `system`. --
        // Each participant runs as the Worker it deploys as, and its
        // composition root builds every capability provider from its own
        // `env`, so the harness has no seam to put a double in. Refused
        // rather than accepted and dropped. Reported without marking the
        // suite `bad`, like #1738 above.
        for &i in indices {
            let Some(d) = parsed[i].integration() else {
                continue;
            };
            let system = bynk_syntax::ast::TestTier::System;
            let suite_stubs = d
                .stubs
                .iter()
                .filter(|_| suite_effective_tier_is_system(d))
                .map(|s| (s, "suite"));
            let case_stubs = d
                .cases
                .iter()
                .filter(|c| case_effective_tier(c, d) == system)
                .flat_map(|c| c.stubs.iter().map(|s| (s, "case")));
            for (stub, scope) in suite_stubs.chain(case_stubs) {
                errors.push(
                    CompileError::new(
                        "bynk.stub.at_system",
                        stub.span,
                        format!(
                            "`stub {}.{}` is in a `system`-tier {scope}, where a `stub` has no effect",
                            stub.capability.name, stub.method.name
                        ),
                    )
                    .with_note(
                        "a `system` case runs each context as the Worker it deploys as, which builds its own capability providers; no seam can be overridden there",
                    )
                    .with_note(format!(
                        "move the case into a `suite {suite_target}` below `system` (`unit`, or `as integration`), where a `stub` overrides the seam"
                    )),
                );
            }
        }

        if bad {
            continue;
        }

        // -- Build the harness-root cross-context view (consumes all). --
        let harness_name = group_name.clone();
        let mut uses_targets: Vec<String> = Vec::new();
        for &i in indices {
            if let Some(d) = parsed[i].integration() {
                for u in &d.uses {
                    let q = u.target.joined();
                    if !uses_targets.contains(&q) {
                        uses_targets.push(q);
                    }
                }
            }
        }
        let mut harness_consumes = unit_consumes.clone();
        harness_consumes.insert(harness_name.clone(), participants.clone());
        let mut harness_uses = unit_uses.clone();
        harness_uses.insert(harness_name.clone(), uses_targets.clone());
        let cross_context = build_cross_context_info(
            &harness_name,
            &harness_consumes,
            unit_consumes_aliases,
            &harness_uses,
            unit_tables,
        );

        // -- Type-check each case body. --
        let mut body_errs: Vec<CompileError> = Vec::new();
        // #1814: the harness view every case is checked in, closed over the
        // types its declarations reach.
        let view = integration_view(&uses_targets, &participants, unit_tables, unit_uses);
        // Review of #1814: the harness pairs the suite's own `uses` with the
        // participants, which no unit's closure sees together, so a reached
        // name one of them already binds is reported here, at the suite's
        // `uses` the reaching declaration came through (or its target, for a
        // participant's).
        for c in &view.conflicts {
            let span = indices
                .iter()
                .filter_map(|&i| parsed[i].integration())
                .flat_map(|d| &d.uses)
                .find(|u| u.target.joined() == c.via)
                .map(|u| u.target.span)
                .unwrap_or(decl.target.span);
            let names = c
                .bound
                .as_ref()
                .map(|b| format!("`{b}`'s type"))
                .unwrap_or_else(|| "another declaration".to_string());
            body_errs.push(view_conflict_error(c, span, &names));
        }
        // v0.25: the harness root is a synthetic namespace — declare its
        // resolution order (uses first, then participants) for assembly.
        let mut harness_resolution = uses_targets.clone();
        harness_resolution.extend(participants.iter().cloned());
        refs.declare_namespace(&harness_name, harness_resolution);
        for &i in indices {
            let Some(d) = parsed[i].integration() else {
                continue;
            };
            refs.enter_file(
                &parsed[i].identity_path(),
                &harness_name,
                parsed[i].is_synthetic(),
            );
            // #1814: a reached type resolves in the view, but the suite may
            // not name it.
            body_errs.extend(resolver::check_hidden_type_names_in_suite(
                d,
                None,
                &view.hidden,
            ));
            // #2132: the target's services and actors, to tell which route a
            // `system` case's address reaches and how that route is secured.
            let system_services = target_test_services(
                participants.first().map(String::as_str),
                unit_tables,
                &cross_context.consumed_contexts,
            );
            let system_actors =
                target_test_actors(participants.first().and_then(|t| unit_tables.get(t)));
            for case in &d.cases {
                let (_, case_callees) = typecheck_integration_case_body(
                    &participants,
                    &view,
                    case,
                    &cross_context,
                    unit_tables,
                    &mut body_errs,
                    refs,
                    tys,
                );
                check_faults_tier(case, case_effective_tier(case, d), &mut body_errs);
                check_event_address_tier(
                    case,
                    case_effective_tier(case, d),
                    &case_callees,
                    &mut body_errs,
                );
                if case_effective_tier(case, d) == bynk_syntax::ast::TestTier::System {
                    check_unsigned_route_at_system(
                        case,
                        &case_callees,
                        &system_services,
                        &system_actors,
                        &mut body_errs,
                    );
                }
                // Slice C: `Wire(…)` is a `system`-only raw argument (it drives the
                // real wire); in a non-`system` case it has no wire to be raw
                // about, so lowering it would silently pass raw text to a direct
                // in-process handler call. Reject it at the tier where it is known.
                if !matches!(
                    case_effective_tier(case, d),
                    bynk_syntax::ast::TestTier::System
                ) && block_uses_wire(&case.body)
                {
                    body_errs.push(CompileError::new(
                        "bynk.test.wire_needs_system",
                        case.name_span,
                        format!(
                            "case `\"{}\"` uses `Wire(...)` but is not a `system`-tier case",
                            case.name
                        ),
                    ).with_note(
                        "`Wire` hands raw, pre-validation input to the real boundary; promote the case with `as system`, or pass a typed argument",
                    ));
                }
                // #706: `by Nobody` presents no credential to the real auth seam
                // (the 401 path), which exists only at `system`; at a lower tier
                // the handler just runs with no identity, silently not a 401.
                if !matches!(
                    case_effective_tier(case, d),
                    bynk_syntax::ast::TestTier::System
                ) && block_uses_nobody(&case.body)
                {
                    body_errs.push(CompileError::new(
                        "bynk.test.credential_needs_system",
                        case.name_span,
                        format!(
                            "case `\"{}\"` drives `by Nobody` but is not a `system`-tier case",
                            case.name
                        ),
                    ).with_note(
                        "`by Nobody` presents no credential to the real auth seam (the 401 path), which exists only at `system`; promote the case with `as system`, or supply `by <Actor>(<identity>)`",
                    ));
                }
            }
        }
        let bodies_failed = !body_errs.is_empty();
        errors.extend(body_errs);
        if bodies_failed {
            continue;
        }

        ready.insert(group_name.clone(), cross_context);
    }

    ready
}

/// Type-check one integration test case body. The body lives in a synthetic
/// harness root that consumes every participant; entry calls
/// (`ctx.service(args)`) are therefore ordinary cross-context calls. The body
/// has type `Effect[Result[(), ExpectationError]]` (modelled as
/// `Effect[Result[(), ValidationError]]`, as in unit tests).
///
/// Returns the body's `expr_types` and `Callee` classification. As
/// [`typecheck_case_body`] does for the unit tier, the **check** path feeds
/// real sinks and the **emit** path reuses it with throwaway ones. #1855: the
/// emit path once lowered with no types at all, so a literal at a refined
/// record field was never branded and the test module failed `tsc`.
#[allow(clippy::too_many_arguments)]
pub fn typecheck_integration_case_body(
    participants: &[String],
    view: &SuiteView,
    case: &Case,
    cross_context: &resolver::CrossContextInfo,
    unit_tables: &HashMap<String, UnitTable>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) -> (
    HashMap<ExprId, checker::TypedExpr>,
    HashMap<ExprId, checker::Callee>,
) {
    // Names in scope: the harness view (#1814: closed over reached types).
    let SuiteView {
        types,
        fns,
        methods,
        ..
    } = view.clone();

    let synthetic_commons = Commons {
        name: QualifiedName {
            parts: vec![Ident {
                name: "integration".to_string(),
                span: Span::default(),
            }],
            span: Span::default(),
        },
        items: Vec::new(),
        uses: Vec::new(),
        documentation: None,
        form: CommonsForm::Brace,
        span: Span::default(),
        trivia: Trivia::default(),
        trailing_comments: Vec::new(),
    };
    // `synthetic_commons` declares nothing of its own (`items: Vec::new()`
    // above) — every entry in `types`/`fns`/`methods` was merged in from a
    // `uses`/participant unit, so an empty local table (no local types, no
    // local events) is the correct answer here, not a stand-in for one.
    let no_local_types = HashMap::new();
    let no_local_events = HashMap::new();
    let resolved = ResolvedCommons::new(
        synthetic_commons,
        types,
        &no_local_types,
        fns,
        methods,
        HashMap::new(),
        &no_local_events,
        cross_context.clone(),
        HashMap::new(),
        // Test-scaffold body, not a real context emission — never rebranded.
        false,
        HashSet::new(),
    )
    // #1883, slice 3: the assets the case may name and fetch.
    .with_assets(view.assets.clone());

    let unit_span = case.span;
    let synthetic_return = TypeRef::Effect(
        Box::new(TypeRef::Result(
            Box::new(TypeRef::Unit(unit_span)),
            Box::new(TypeRef::ValidationError(unit_span)),
            unit_span,
        )),
        unit_span,
    );
    let return_ty = checker::resolve_type_ref(&synthetic_return, &resolved.types, tys).unwrap();
    let mut expr_types: HashMap<ExprId, checker::TypedExpr> = HashMap::new();
    let mut callees: HashMap<ExprId, checker::Callee> = HashMap::new();
    // Test bodies record no hints (out of v0.27 scope) — a throwaway sink.
    let mut no_hints = HintSink::new();
    let mut no_locals = LocalsSink::new();
    // Test bodies record no capability requirements either — muted sink.
    let mut no_requirements = RequirementSink::new();
    let _ = checker::check_body(
        &resolved,
        &case.body,
        return_ty,
        case.span,
        HashMap::new(),
        checker::CapabilityCtx::default(),
        // Slice B: a `system` case addresses the target's own service (`api.POST`)
        // and names a principal (`by User(...)`), so the checker needs the
        // target's services and actors — the same resolution the unit tier does.
        target_test_services(
            participants.first().map(String::as_str),
            unit_tables,
            &cross_context.consumed_contexts,
        ),
        target_test_actors(participants.first().and_then(|t| unit_tables.get(t))),
        None,
        checker::CheckSinks {
            tys,
            expr_types: &mut expr_types,
            errors,
            refs,
            hints: &mut no_hints,
            locals: &mut no_locals,
            requirements: &mut no_requirements,
            callees: &mut callees,
        },
    );
    (expr_types, callees)
}

/// Review of #1814: `bynk.uses.name_conflict` for a reached name a suite's
/// view binds to another declaration, at `span`. `names` says what the view
/// binds the name to. The same diagnostic a unit's closure gives (#1807).
fn view_conflict_error(
    c: &crate::project_model::ViewConflict,
    span: Span,
    names: &str,
) -> CompileError {
    CompileError::new(
        "bynk.uses.name_conflict",
        span,
        format!(
            "`{via}` brings in declarations that use type `{t}` from `{owner}`, but `{t}` in \
             this suite names {names}",
            via = c.via,
            t = c.name,
            owner = c.owner,
        ),
    )
    .with_note(
        "rename one of the two types; the imported declarations cannot be checked against a \
         different type of the same name",
    )
}

fn first_test_target_span(indices: &[usize], parsed: &[ParsedFile]) -> Span {
    indices
        .first()
        .and_then(|&i| parsed[i].test().map(|t| t.target.span))
        .unwrap_or_default()
}

/// Type-check test/property bodies for a target and validate `stub` RHS
/// value types (v0.118). Bodies use the target's privileged view; a `stub`
/// value whose type disagrees with the overridden op's return is
/// `bynk.stub.rhs_type`.
#[allow(clippy::too_many_arguments)]
fn check_test_bodies(
    target_name: &str,
    target_kind: UnitKind,
    indices: &[usize],
    parsed: &[ParsedFile],
    stubs: &HashMap<String, ResolvedStub>,
    unit_tables: &HashMap<String, UnitTable>,
    exports_visibility: &HashMap<String, HashMap<String, Visibility>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    unit_uses: &HashMap<String, Vec<String>>,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) -> Vec<CompileError> {
    let mut errors = Vec::new();

    // The target's privileged view, built once for the `stub` check and the
    // #1814 naming gate and conflict report below.
    let privileged = build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    );

    // v0.118: validate each `stub` RHS value's type against the overridden
    // op's declared return type, in the target's privileged view. A best-effort
    // check: the value expression is type-checked as if it were the op body's
    // tail; any resulting error surfaces as `bynk.stub.rhs_type`.
    if !stubs.is_empty()
        && let Some((resolved, _)) = &privileged
    {
        for rp in stubs.values() {
            refs.enter_file(&rp.identity_path, target_name, false);
            for clause in &rp.clauses {
                let Some(op) = rp
                    .cap_decl
                    .ops
                    .iter()
                    .find(|o| o.name.name == clause.method.name)
                else {
                    continue;
                };
                let check_value = |e: &Expr, errors: &mut Vec<CompileError>| {
                    if !stub_value_typechecks(e, op, resolved, tys) {
                        errors.push(CompileError::new(
                            "bynk.stub.rhs_type",
                            e.span,
                            format!(
                                "the value provided for `{}.{}` does not match the operation's declared return type `{}`",
                                rp.cap,
                                op.name.name,
                                ts_type_ref_display(&op.return_type),
                            ),
                        ));
                    }
                };
                match &clause.rhs {
                    StubRhs::Returns(e) => check_value(e, &mut errors),
                    StubRhs::ReturnsEach(outcomes, _) => {
                        for o in outcomes {
                            if let SeqOutcome::Value(e) = o {
                                check_value(e, &mut errors);
                            }
                        }
                    }
                    StubRhs::Fails(_) => {}
                }
            }
        }
    }

    // #1737: which target services reach another context, for the tier gate
    // in the loop below. Computed once per target.
    let crossing = CrossingServices::of(
        target_name,
        unit_tables,
        unit_consumes,
        unit_consumes_aliases,
    );

    // #1814: the types the target's privileged view reaches but does not
    // name. A case resolves them; no suite may write one.
    // #1927: the imported types the target hides behind its own values,
    // with the target's types to find a shadowed sum's variants.
    let target_shadowed = privileged
        .as_ref()
        .map(|(r, _)| (r.shadowed_imports.clone(), r.types.clone()))
        .unwrap_or_default();
    let reach = privileged.map(|(_, r)| r).unwrap_or_default();
    let hidden = reach.hidden;

    // Review of #1814: a reached name the view binds to a consumed context's
    // unexported type. The target's own table holds only that context's
    // exports, so its closure resolved the name to the reached commons type
    // and reported nothing; every other conflict here is the target's own,
    // which its closure already reported as `bynk.uses.name_conflict`.
    for c in &reach.conflicts {
        let Some(bound) = &c.bound else {
            continue;
        };
        let consumed = unit_consumes
            .get(target_name)
            .is_some_and(|cs| cs.contains(bound));
        let exported = exports_visibility
            .get(bound)
            .is_some_and(|e| e.contains_key(&c.name));
        if consumed && !exported {
            errors.push(view_conflict_error(
                c,
                first_test_target_span(indices, parsed),
                &format!("`{bound}`'s unexported type"),
            ));
        }
    }

    // Type-check test case bodies — they live in the target's privileged
    // view, with `stub` overriding individual capability seams.
    for &i in indices {
        let Some(test_decl) = parsed[i].test() else {
            continue;
        };
        errors.extend(resolver::check_hidden_type_names_in_suite(
            test_decl,
            Some(target_name),
            &hidden,
        ));
        // #1927: nor a type the target hides behind its own value.
        errors.extend(resolver::check_shadowed_type_names_in_suite(
            test_decl,
            &target_shadowed.0,
            &target_shadowed.1,
        ));
        // v0.25: test-case edges record in the test file, resolving bare
        // names through the *target* unit's namespace.
        refs.enter_file(
            &parsed[i].identity_path(),
            target_name,
            parsed[i].is_synthetic(),
        );
        for case in &test_decl.cases {
            check_test_case_body(
                target_name,
                target_kind,
                case,
                unit_tables,
                unit_uses,
                unit_consumes,
                unit_consumes_aliases,
                &mut errors,
                refs,
                tys,
            );
            let tier = case_effective_tier(case, test_decl);
            check_faults_tier(case, tier, &mut errors);
            if tier != bynk_syntax::ast::TestTier::System {
                crossing.check_case(target_name, case, tier, &mut errors);
            }
        }
        // v0.114: generative `property` blocks — check their `for all` bindings,
        // `where` filter, and predicate body (testing track slice 2).
        // v0.118: a `property` never carries a tier — `as <tier>` is a
        // `case`-only affordance, and the grammar has no property-tier
        // production (`PropertyDecl` has no tier field), so there is nothing to
        // check here (#1662 removed an unreachable guard for it).
        for prop in &test_decl.properties {
            check_property_body(
                target_name,
                target_kind,
                prop,
                unit_tables,
                unit_uses,
                unit_consumes,
                unit_consumes_aliases,
                &mut errors,
                refs,
                tys,
            );
            crossing.check_property(target_name, prop, &mut errors);
            check_advance_in_property(prop, &mut errors);
        }
    }

    errors
}

/// #2137: `advance Clock by …` moves a `case`'s harness clock. A `property`
/// runs its body once per generated input with no clock of its own, so an
/// `advance` there is `bynk.test.advance_outside_case`, as it is anywhere else
/// outside a `case` body.
fn check_advance_in_property(
    prop: &bynk_syntax::ast::PropertyDecl,
    errors: &mut Vec<CompileError>,
) {
    fn collect(b: &Block, out: &mut Vec<Span>) {
        for s in &b.statements {
            if let Statement::Advance(a) = s {
                out.push(a.keyword.merge(a.cap.span));
            }
        }
        let mut exprs = Vec::new();
        for s in &b.statements {
            bynk_syntax::ast::statement_exprs(s, &mut exprs);
        }
        exprs.push(&b.tail);
        for e in exprs {
            walk(e, out);
        }
    }
    // Descend through every expression, entering a nested block through
    // `collect` so its statements are seen once.
    fn walk(e: &Expr, out: &mut Vec<Span>) {
        match &e.kind {
            ExprKind::Block(inner) => collect(inner, out),
            ExprKind::If {
                cond,
                then_block,
                else_block,
            } => {
                walk(cond, out);
                collect(then_block, out);
                collect(else_block, out);
            }
            ExprKind::Match { discriminant, arms } => {
                walk(discriminant, out);
                for arm in arms {
                    if let Some(g) = &arm.guard {
                        walk(g, out);
                    }
                    match &arm.body {
                        bynk_syntax::ast::MatchBody::Block(b) => collect(b, out),
                        bynk_syntax::ast::MatchBody::Expr(x) => walk(x, out),
                    }
                }
            }
            _ => {
                for c in bynk_syntax::ast::expr_children(e) {
                    walk(c, out);
                }
            }
        }
    }
    let mut spans = Vec::new();
    collect(&prop.forall.body, &mut spans);
    for span in spans {
        errors.push(
            CompileError::new(
                "bynk.test.advance_outside_case",
                span,
                format!(
                    "`advance` is only valid inside a `case` body, but this is in property `\"{}\"`",
                    prop.name
                ),
            )
            .with_note(
                "a property runs its body once per generated input, with no clock of its own; move the time-dependent check into a `case`",
            ),
        );
    }
}

/// v0.118: wrap a single expression as a `{ tail: e }` block, so a `stub`
/// value can be type-checked or lowered in the same op-body position a provider
/// operation's tail occupies.
///
/// Dual-use (found during P5.4's move, not in the original slice plan):
/// `stub_value_typechecks` (in this module) uses it for the checking path;
/// `bynk-emit`'s `lower_stub_value_block` also calls it, qualified, to lower
/// a `stub` RHS value in the same op-body tail position. `pub` for that
/// second caller, same as every other dual-use function in this module.
pub fn value_block(e: &Expr) -> Block {
    Block {
        statements: Vec::new(),
        tail: Box::new(e.clone()),
        span: e.span,
        tail_leading_comments: Vec::new(),
        implicit_tail: false,
    }
}

/// v0.118: whether a `stub` value expression type-checks against the
/// overridden capability op's declared return type (best-effort — a throwaway
/// check against the target's privileged view). A mismatch drives
/// `bynk.stub.rhs_type`.
fn stub_value_typechecks(
    e: &Expr,
    op: &CapabilityOp,
    resolved: &ResolvedCommons,
    tys: &Arc<Types>,
) -> bool {
    let block = value_block(e);
    let mut expr_types: HashMap<ExprId, checker::TypedExpr> = HashMap::new();
    let mut callees: HashMap<ExprId, checker::Callee> = HashMap::new();
    let mut errs: Vec<CompileError> = Vec::new();
    checker::check_handler_body(
        resolved,
        checker::HandlerBodyCheck::new(&block, &op.return_type, &op.params, &[]),
        checker::CheckSinks {
            tys,
            expr_types: &mut expr_types,
            errors: &mut errs,
            refs: &mut RefSink::new(),
            hints: &mut HintSink::new(),
            locals: &mut LocalsSink::new(),
            requirements: &mut RequirementSink::new(),
            callees: &mut callees,
        },
    );
    errs.is_empty()
}

/// Slice C: whether a `case` body uses a `Wire(…)` raw argument anywhere. A
/// `Wire` is only meaningful at `system` (it hands pre-validation input to the
/// real boundary); used at any other tier it is `bynk.test.wire_needs_system`.
fn block_uses_wire(block: &Block) -> bool {
    // Ported onto `bynk_syntax::ast::expr_children` (P5.4) — `bynk-emit`'s
    // `crate::emitter::walk_exprs` this used before the move is emission-
    // private and unreachable from `bynk-check`. `expr_children` is the
    // exhaustive total child iterator the checker already walks the same way
    // (see `context_checks.rs`/`checker.rs`); this reimplements the original
    // statement-value + tail walk faithfully, not a rewrite of its behaviour.
    fn contains_wire(e: &Expr) -> bool {
        matches!(e.kind, ExprKind::Wire(_))
            || bynk_syntax::ast::expr_children(e)
                .into_iter()
                .any(contains_wire)
    }
    // #1851: every expression of a statement, a principal's identity
    // included, not a hand-picked value.
    let mut exprs = Vec::new();
    for s in &block.statements {
        bynk_syntax::ast::statement_exprs(s, &mut exprs);
    }
    exprs.into_iter().any(contains_wire) || contains_wire(&block.tail)
}

/// A cross-context service call: `(context, service)`.
type CrossCall = (String, String);

/// A same-context agent handler: `(agent, handler)`.
type AgentHandler = (String, String);

/// #1737: what in the target reaches another context, for the
/// `bynk.tier.cross_context_needs_system` gate: each service, and each agent
/// handler, that does, with the cross-context call it reaches.
///
/// Below `system` a case runs in-process: a consumed context is not stood up,
/// and `stub` doubles capabilities only, never a context's services. So a case
/// that reaches another context's service (directly, or through a target
/// service or agent handler that calls one) had no collaborator to call, and
/// crashed at runtime on an `undefined` surface. Crossing a context boundary is
/// the `system` tier's job; this finds the test bodies that try it below.
///
/// A cross-context call is recognised the way the resolver resolves one
/// (`CrossContextInfo::resolve_prefix`): a method call whose receiver chain is a
/// consumed context's alias or qualified name, naming a service that context
/// declares. A service or agent handler reaches another context if its own
/// body makes such a call, or dispatches to an agent handler that
/// (transitively) does. An agent call is recognised inline
/// (`Counter(k).bump(…)`) and through a `let`-bound instance
/// (`let c = Counter(k)` then `c.bump(…)`).
struct CrossingServices<'a> {
    /// service name → the cross-context call it reaches.
    services: HashMap<String, CrossCall>,
    /// agent handler → the cross-context call it reaches.
    agents: HashMap<AgentHandler, CrossCall>,
    /// The target's own table, for recognising agent calls in a test body.
    table: Option<&'a UnitTable>,
    /// The target's consumed contexts and aliases, for direct calls in a body.
    consumed: &'a [String],
    aliases: Option<&'a HashMap<String, String>>,
    unit_tables: &'a HashMap<String, UnitTable>,
}

impl<'a> CrossingServices<'a> {
    fn of(
        target_name: &str,
        unit_tables: &'a HashMap<String, UnitTable>,
        unit_consumes: &'a HashMap<String, Vec<String>>,
        unit_consumes_aliases: &'a HashMap<String, HashMap<String, String>>,
    ) -> Self {
        let mut this = CrossingServices {
            services: HashMap::new(),
            agents: HashMap::new(),
            table: unit_tables.get(target_name),
            consumed: unit_consumes
                .get(target_name)
                .map(Vec::as_slice)
                .unwrap_or(&[]),
            aliases: unit_consumes_aliases.get(target_name),
            unit_tables,
        };
        let Some(table) = this.table else {
            return this;
        };
        if this.consumed.is_empty() {
            return this;
        }
        // Each agent handler's own cross-context call (if any) and the agent
        // handlers it dispatches to, then a fixpoint over the dispatch edges.
        let mut agent_edges: HashMap<AgentHandler, Vec<AgentHandler>> = HashMap::new();
        for (agent, decl) in &table.agents {
            for h in &decl.handlers {
                let Some(method) = &h.method_name else {
                    continue;
                };
                let key = (agent.clone(), method.name.clone());
                let (hit, edges) = this.scan(&h.body);
                if let Some(hit) = hit {
                    this.agents.insert(key.clone(), hit);
                }
                agent_edges.insert(key, edges);
            }
        }
        loop {
            let mut changed = false;
            for (key, edges) in &agent_edges {
                if this.agents.contains_key(key) {
                    continue;
                }
                if let Some(hit) = edges.iter().find_map(|e| this.agents.get(e).cloned()) {
                    this.agents.insert(key.clone(), hit);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for (name, decl) in &table.services {
            for h in &decl.handlers {
                let (hit, edges) = this.scan(&h.body);
                let reached =
                    hit.or_else(|| edges.iter().find_map(|e| this.agents.get(e).cloned()));
                if let Some(reached) = reached {
                    this.services.entry(name.clone()).or_insert(reached);
                }
            }
        }
        this
    }

    /// The consumed context `chain` names, by alias or qualified name.
    fn resolve(&self, chain: &str) -> Option<&String> {
        self.aliases
            .and_then(|a| a.get(chain))
            .or_else(|| self.consumed.iter().find(|c| *c == chain))
    }

    /// `e` as a cross-context service call.
    fn cross_call(&self, e: &Expr) -> Option<CrossCall> {
        let ExprKind::MethodCall {
            receiver, method, ..
        } = &e.kind
        else {
            return None;
        };
        let ctx = self.resolve(&receiver_chain(receiver)?)?;
        let callee = self.unit_tables.get(ctx)?;
        callee
            .services
            .contains_key(&method.name)
            .then(|| (ctx.clone(), method.name.clone()))
    }

    /// `e` as an agent handler call, inline (`Counter(k).bump(…)`) or through
    /// an instance `bound` by a `let` (`c.bump(…)`).
    fn dispatch(&self, e: &Expr, bound: &HashMap<String, String>) -> Option<AgentHandler> {
        let table = self.table?;
        let ExprKind::MethodCall {
            receiver, method, ..
        } = &e.kind
        else {
            return None;
        };
        let agent = match &receiver.kind {
            ExprKind::Call { name, .. } if table.agents.contains_key(&name.name) => {
                name.name.clone()
            }
            ExprKind::Ident(x) => bound.get(&x.name)?.clone(),
            _ => return None,
        };
        Some((agent, method.name.clone()))
    }

    /// The first cross-context call in `block` (in source order), and every
    /// agent handler it dispatches to.
    fn scan(&self, block: &Block) -> (Option<CrossCall>, Vec<AgentHandler>) {
        let bound = self.agent_bindings(block);
        let mut hit = None;
        let mut edges = Vec::new();
        for e in exprs_in_order(block) {
            if hit.is_none() {
                hit = self.cross_call(e);
            }
            if let Some(edge) = self.dispatch(e, &bound) {
                edges.push(edge);
            }
        }
        (hit, edges)
    }

    /// The names `block` binds, at any depth, to an agent instance
    /// (`let c = Counter(k)`), mapped to the agent.
    fn agent_bindings(&self, block: &Block) -> HashMap<String, String> {
        let mut bound = HashMap::new();
        let Some(table) = self.table else {
            return bound;
        };
        for b in blocks_deep(block) {
            for s in &b.statements {
                if let Statement::Let(l) | Statement::EffectLet(l) = s
                    && let ExprKind::Call { name, .. } = &l.value.kind
                    && table.agents.contains_key(&name.name)
                {
                    bound.insert(l.name.name.clone(), name.name.clone());
                }
            }
        }
        bound
    }

    /// Report each call in a non-`system` test body that reaches another
    /// context: a target service that crosses (`check.call(…)`), an agent
    /// handler that crosses (`Counter(k).bump(…)`), or a consumed context's
    /// service called directly. `subject` names the body (``case `"…"` ``),
    /// and `below` says why it runs in-process.
    fn check_block(
        &self,
        target_name: &str,
        subject: &str,
        below: &str,
        body: &Block,
        errors: &mut Vec<CompileError>,
    ) {
        if self.consumed.is_empty() {
            return;
        }
        let bound = self.agent_bindings(body);
        // A name a `let` in the body rebinds is the binding, not a service.
        let shadowed: HashSet<String> = blocks_deep(body)
            .iter()
            .flat_map(|b| &b.statements)
            .filter_map(|s| match s {
                Statement::Let(l) | Statement::EffectLet(l) => Some(l.name.name.clone()),
                _ => None,
            })
            .collect();
        for e in exprs_in_order(body) {
            let ExprKind::MethodCall {
                receiver,
                method,
                args,
                ..
            } = &e.kind
            else {
                continue;
            };
            // #1883 (review of #1923): `svc.<VERB>(x)` whose first argument is
            // not a route string reaches no handler — it is an asset fetch
            // (`svc.GET(asset)`, served by the asset tier) or a bad address
            // reported elsewhere — so it crosses into no other context.
            if bynk_syntax::ast::HttpMethod::from_ident(&method.name).is_some()
                && args
                    .first()
                    .is_some_and(|a| !matches!(a.kind, ExprKind::StrLit(_)))
            {
                continue;
            }
            let through = match &receiver.kind {
                // `svc.call(…)` / `svc.GET(…)` / … on a target service.
                ExprKind::Ident(svc) if !shadowed.contains(&svc.name) => self
                    .services
                    .get(&svc.name)
                    .map(|reached| (format!("`{}`", svc.name), Some(svc.name.clone()), reached)),
                _ => None,
            };
            let through = through.or_else(|| {
                let (agent, handler) = self.dispatch(e, &bound)?;
                let reached = self.agents.get(&(agent.clone(), handler.clone()))?;
                Some((format!("`{agent}.{handler}`"), None, reached))
            });
            let message = match through {
                Some((via, _, (ctx, svc))) => {
                    format!(
                        "{subject} calls {via}, which calls `{ctx}.{svc}` in another context, but {below}"
                    )
                }
                None => match self.cross_call(e) {
                    Some((ctx, svc)) => {
                        format!("{subject} calls `{ctx}.{svc}` in another context, but {below}")
                    }
                    None => continue,
                },
            };
            let service = match &receiver.kind {
                ExprKind::Ident(svc) if self.services.contains_key(&svc.name) => {
                    Some(svc.name.clone())
                }
                _ => None,
            };
            let promote = match service {
                Some(s) => format!(
                    "move it into a `system` suite (`suite {target_name} as system {{ … }}`), where the service is addressed by its context path (`{target_name}.{s}(…)`)"
                ),
                None => format!(
                    "move it into a `system` suite (`suite {target_name} as system {{ … }}`) and drive it through one of `{target_name}`'s services (`{target_name}.<service>(…)`), which reaches the other context over the real wire"
                ),
            };
            errors.push(
                CompileError::new("bynk.tier.cross_context_needs_system", e.span, message)
                    .with_note(format!(
                        "below `system` a test runs in-process, with no other context stood up to call (`stub` doubles capabilities, not a context's services); {promote}"
                    )),
            );
        }
    }

    /// The gate for a non-`system` `case`.
    fn check_case(
        &self,
        target_name: &str,
        case: &Case,
        tier: TestTier,
        errors: &mut Vec<CompileError>,
    ) {
        let tier = tier.as_str();
        // "an integration", but "a unit" (said with a "y" sound).
        let article = if tier == "integration" { "an" } else { "a" };
        self.check_block(
            target_name,
            &format!("case `\"{}\"`", case.name),
            &format!("it is {article} `{tier}`-tier case"),
            &case.body,
            errors,
        );
    }

    /// The gate for a `property`, which has no tier and always runs in-process.
    fn check_property(
        &self,
        target_name: &str,
        prop: &PropertyDecl,
        errors: &mut Vec<CompileError>,
    ) {
        self.check_block(
            target_name,
            &format!("property `\"{}\"`", prop.name),
            "a property runs in-process (it generates, and has no `system` tier)",
            &prop.forall.body,
            errors,
        );
    }
}

/// `e` as a dotted name (`Vault`, `demo.vault`), or `None` for anything that
/// is not a chain of identifiers.
fn receiver_chain(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::Ident(id) => Some(id.name.clone()),
        ExprKind::FieldAccess { receiver, field } => {
            Some(format!("{}.{}", receiver_chain(receiver)?, field.name))
        }
        _ => None,
    }
}

/// Every expression in `block` at any depth, in source order: each statement's
/// expressions, then the tail, each visited before its children (via
/// [`bynk_syntax::ast::expr_children`]).
fn exprs_in_order(block: &Block) -> Vec<&Expr> {
    let mut roots: Vec<&Expr> = Vec::new();
    for s in &block.statements {
        bynk_syntax::ast::statement_exprs(s, &mut roots);
    }
    roots.push(&block.tail);
    // A stack popped from the end, so push in reverse to visit in order.
    let mut stack: Vec<&Expr> = roots.into_iter().rev().collect();
    let mut out = Vec::new();
    while let Some(e) = stack.pop() {
        out.push(e);
        stack.extend(bynk_syntax::ast::expr_children(e).into_iter().rev());
    }
    out
}

/// `block` and every block nested in it (a block expression, an `if`'s
/// branches, a `match` arm's block body), so a `let` at any depth is seen.
fn blocks_deep(block: &Block) -> Vec<&Block> {
    let mut out = vec![block];
    for e in exprs_in_order(block) {
        match &e.kind {
            ExprKind::Block(b) => out.push(b),
            ExprKind::If {
                then_block,
                else_block,
                ..
            } => {
                out.push(then_block);
                out.push(else_block);
            }
            ExprKind::Match { arms, .. } => {
                for arm in arms {
                    if let MatchBody::Block(b) = &arm.body {
                        out.push(b);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

/// #1706: whether a `case` body claims a fault anywhere (`expect <call>
/// faults`). The claim observes a call *throwing*, which only an in-process
/// call does: at `system` a handler's fault crosses the real Worker boundary
/// as an error response, never a throw at the harness, so the claim could
/// never hold there (`bynk.test.faults_needs_in_process`).
fn block_uses_faults(block: &Block) -> bool {
    !faults_claims(block).is_empty()
}

/// Every fault claim (`expect <call> faults`) in `block`, at any depth, in
/// source order.
fn faults_claims(block: &Block) -> Vec<&bynk_syntax::ast::FaultsExpr> {
    fn collect<'a>(e: &'a Expr, out: &mut Vec<&'a bynk_syntax::ast::FaultsExpr>) {
        if let ExprKind::Faults(f) = &e.kind {
            out.push(f);
        }
        for c in bynk_syntax::ast::expr_children(e) {
            collect(c, out);
        }
    }
    let mut exprs = Vec::new();
    for s in &block.statements {
        bynk_syntax::ast::statement_exprs(s, &mut exprs);
    }
    exprs.push(&block.tail);
    let mut out = Vec::new();
    for e in exprs {
        collect(e, &mut out);
    }
    out
}

/// #2052: report a `system`-tier case that delivers an event with
/// `svc.event(e)`. At `system` an event reaches a subscriber only through its
/// publisher's fan-out, which emits to every subscriber of the event rather
/// than addressing one service, over a per-edge signed request. A case-built
/// delivery to one subscriber has no counterpart there, so the address is an
/// in-process one: the case drives the emitting handler instead.
fn check_event_address_tier(
    case: &Case,
    tier: bynk_syntax::ast::TestTier,
    callees: &HashMap<ExprId, checker::Callee>,
    errors: &mut Vec<CompileError>,
) {
    if tier != bynk_syntax::ast::TestTier::System {
        return;
    }
    let mut services: Vec<&str> = callees
        .values()
        .filter_map(|c| match c {
            checker::Callee::TestService { service, address } if address == "event" => {
                Some(service.as_str())
            }
            _ => None,
        })
        .collect();
    services.sort_unstable();
    services.dedup();
    for service in services {
        errors.push(
            CompileError::new(
                "bynk.test.event_needs_in_process",
                case.name_span,
                format!(
                    "case `\"{}\"` delivers an event with `{service}.event(…)`, but is a `system`-tier case",
                    case.name
                ),
            )
            .with_note(
                "at `system` an event reaches a subscriber only through its publisher's fan-out; drive the handler that emits it, or deliver the event at `unit` or `integration`",
            ),
        );
    }
}

/// #2132, #2150: report a `system`-tier case that calls an http route whose
/// actor is `Signature`- or `Oidc`-secured. The system driver presents a
/// credential only for a Bearer route (`bearer_seam_for`); it has no
/// `Signature` signer (ADR 0489) and no `Oidc` token issuer yet, so the
/// request reaches the real seam without a credential and can only be refused
/// with a `401`. ADR 0212 D1: the check gates exactly what the emitter emits.
/// Each scheme has its own code (`bynk.test.signature_needs_in_process`,
/// `bynk.test.oidc_needs_in_process`). A call driven `by Nobody` is left to
/// `bynk.test.nobody_needs_secured_route`, which already refuses it there.
fn check_unsigned_route_at_system(
    case: &Case,
    callees: &HashMap<ExprId, checker::Callee>,
    services: &HashMap<String, checker::TestServiceSig>,
    actors: &HashMap<String, bynk_syntax::ast::ActorDecl>,
    errors: &mut Vec<CompileError>,
) {
    use crate::actors::Scheme;
    use bynk_syntax::ast::HandlerKind;
    // The call-site principal of each `let x <- <address> by <Actor>`.
    let mut principals: HashMap<ExprId, &str> = HashMap::new();
    for b in blocks_deep(&case.body) {
        for st in &b.statements {
            if let Statement::EffectLet(l) = st
                && let Some(p) = &l.principal
            {
                principals.insert(l.value.id, p.actor.name.as_str());
            }
        }
    }
    // The schemes the system driver has no credential for, as the label
    // that picks the diagnostic below.
    let unsigned_scheme = |name: &str| -> Option<&'static str> {
        let a = actors.get(name)?;
        let base = a
            .refinement
            .as_ref()
            .and_then(|r| actors.get(&r.base.name))
            .unwrap_or(a);
        match Scheme::from_name(base.auth.as_ref()?.name.as_str())? {
            Scheme::Signature => Some("Signature"),
            Scheme::Oidc => Some("Oidc"),
            _ => None,
        }
    };
    let mut found: Vec<(String, String, String, &'static str)> = Vec::new();
    for (id, callee) in callees {
        let checker::Callee::TestService { service, address } = callee else {
            continue;
        };
        let Some((verb, path)) = address.split_once(' ') else {
            continue;
        };
        let principal = principals.get(id).copied();
        if principal == Some("Nobody") {
            continue;
        }
        let Some(handler) = services.get(service).and_then(|sig| {
            sig.handlers.iter().find(|h| {
                matches!(&h.kind, HandlerKind::Http { method, path: p } if method.as_str() == verb && p == path)
            })
        }) else {
            continue;
        };
        let Some(by) = handler.by_clause.as_ref() else {
            continue;
        };
        // A sum runs as the member the call-site principal names. With no
        // principal the system driver falls back to the primary member, as
        // `bearer_seam_for` does, so a Signature or Oidc primary still goes
        // out without a credential. A principal naming no member is
        // `bynk.test.principal_identity_mismatch`'s, not this check's.
        let actor = if by.is_sum() {
            match principal {
                Some(p) if by.actors.iter().any(|a| a.name == p) => p,
                Some(_) => continue,
                None => by.primary().name.as_str(),
            }
        } else {
            by.primary().name.as_str()
        };
        if let Some(scheme) = unsigned_scheme(actor) {
            found.push((
                service.clone(),
                format!("{verb}(\"{path}\")"),
                actor.to_string(),
                scheme,
            ));
        }
    }
    found.sort_unstable();
    found.dedup();
    for (service, route, actor, scheme) in found {
        let (code, missing) = if scheme == "Oidc" {
            (
                "bynk.test.oidc_needs_in_process",
                "no `Oidc` token issuer yet, so the request reaches the seam with no token",
            )
        } else {
            (
                "bynk.test.signature_needs_in_process",
                "no `Signature` signer yet, so the request reaches the seam unsigned",
            )
        };
        errors.push(
            CompileError::new(
                code,
                case.name_span,
                format!(
                    "case `\"{}\"` calls `{service}.{route}`, whose actor `{actor}` is `{scheme}`-secured, but is a `system`-tier case",
                    case.name
                ),
            )
            .with_note(format!(
                "the `system` driver has {missing} and is refused with a `401`; call the route from a suite `as integration` (or `unit`), where the handler runs in process"
            )),
        );
    }
}

/// #1706: report a `system`-tier case that claims a fault — see
/// [`block_uses_faults`].
///
/// #1812: a fault claim driven `by Nobody` is reported too, at any tier.
/// `by Nobody` exercises the auth seam that exists only at `system`, where
/// the claim cannot hold, so it falls under the same code rather than
/// `bynk.test.credential_needs_system`, whose advice (promote the case to
/// `system`) would only lead here.
fn check_faults_tier(
    case: &Case,
    tier: bynk_syntax::ast::TestTier,
    errors: &mut Vec<CompileError>,
) {
    if tier != bynk_syntax::ast::TestTier::System {
        for p in faults_claims(&case.body)
            .into_iter()
            .filter_map(|f| f.principal.as_ref())
            .filter(|p| p.actor.name == "Nobody")
        {
            errors.push(
                CompileError::new(
                    "bynk.test.faults_needs_in_process",
                    p.span,
                    "a fault claim cannot be driven `by Nobody`: the missing credential is rejected only by the `system`-tier auth seam, where a fault is never a throw",
                )
                .with_note(
                    "assert the rejection instead, in a `system` case: `let r <- <call> by Nobody` then `expect r is Rejected(Unauthorized)`",
                ),
            );
        }
        return;
    }
    if block_uses_faults(&case.body) {
        errors.push(
            CompileError::new(
                "bynk.test.faults_needs_in_process",
                case.name_span,
                format!(
                    "case `\"{}\"` claims a fault with `faults`, but is a `system`-tier case",
                    case.name
                ),
            )
            .with_note(
                "at `system` a handler's fault reaches the case as an error response from the deployed Worker, not a throw; claim the fault at `unit` or `integration`, or assert the response at `system`",
            ),
        );
    }
}

/// #706: whether a `case` body drives an effect-let `by Nobody` — the "no
/// credential" principal. It is only meaningful at `system` (there is no auth
/// seam to reject a missing credential at `unit`), so a non-`system` case using
/// it is `bynk.test.credential_needs_system`. Every block of the body counts,
/// not just its top level (#1849): a `by Nobody` inside an `if` or a match arm
/// is the same credential-less call.
fn block_uses_nobody(block: &Block) -> bool {
    blocks_deep(block).into_iter().any(|b| {
        b.statements.iter().any(|s| {
            matches!(s, Statement::EffectLet(l)
                if l.principal.as_ref().is_some_and(|p| p.actor.name == "Nobody"))
        })
    })
}

/// Register a synthetic call-record type per capability operation of the target
/// context (v0.117, testing track slice 5), so `trace(Cap.op)` — typed
/// `List[<CallRecord>]` — supports field access on its records. The record's
/// fields are the operation's parameters.
pub fn register_call_record_types(
    resolved: &mut ResolvedCommons,
    target_name: &str,
    unit_tables: &HashMap<String, UnitTable>,
) {
    let Some(table) = unit_tables.get(target_name) else {
        return;
    };
    // #291: a consumed adapter's capabilities are seams too (its `trace` and
    // `with` records); the target's own wins a name clash.
    let mut caps: Vec<(&String, &CapabilityDecl)> = table.capabilities.iter().collect();
    for (n, d) in flattened_platform_capabilities(unit_tables, resolved) {
        if !table.capabilities.contains_key(n) {
            caps.push((n, d));
        }
    }
    for (cap_name, decl) in caps {
        for op in &decl.ops {
            let fields: Vec<RecordField> = op
                .params
                .iter()
                .map(|p| RecordField {
                    documentation: None,
                    trivia: Default::default(),
                    name: p.name.clone(),
                    type_ref: p.type_ref.clone(),
                    refinement: None,
                    init: None,
                    span: p.span,
                })
                .collect();
            let name = checker::call_record_type_name(cap_name, &op.name.name);
            resolved.types.insert(
                name.clone(),
                Arc::new(TypeDecl {
                    type_params: Vec::new(),
                    name: Ident {
                        name,
                        span: op.name.span,
                    },
                    body: TypeBody::Record(RecordBody {
                        trailing_comments: Default::default(),
                        fields,
                        span: op.name.span,
                    }),
                    documentation: None,
                    span: op.name.span,
                    trivia: Trivia::default(),
                }),
            );
        }
    }
}

fn target_test_actors(table: Option<&UnitTable>) -> HashMap<String, bynk_syntax::ast::ActorDecl> {
    table.map(|t| t.actors.clone()).unwrap_or_default()
}

/// #2052 review: the context that publishes `event` to `target`'s
/// subscribers, resolved as the target's `from Events(E)` header resolves
/// `E`: the target's own declaration, else the one context among those it
/// consumes (`consumed`) that declares it. `Err` lists the candidates when
/// there is no unique one (none, or several). A same-named event in a context
/// the target does not consume is never a candidate.
pub fn event_publisher(
    target: &str,
    event: &str,
    unit_tables: &HashMap<String, UnitTable>,
    consumed: &[String],
) -> Result<String, Vec<String>> {
    if unit_tables
        .get(target)
        .is_some_and(|t| t.events.contains_key(event))
    {
        return Ok(target.to_string());
    }
    let mut owners: Vec<String> = consumed
        .iter()
        .filter(|c| c.as_str() != target)
        .filter(|c| {
            unit_tables
                .get(*c)
                .is_some_and(|t| t.events.contains_key(event))
        })
        .cloned()
        .collect();
    owners.sort();
    owners.dedup();
    match owners.as_slice() {
        [one] => Ok(one.clone()),
        _ => Err(owners),
    }
}

fn target_test_services(
    target: Option<&str>,
    unit_tables: &HashMap<String, UnitTable>,
    consumed: &[String],
) -> HashMap<String, checker::TestServiceSig> {
    use bynk_syntax::ast::ServiceProtocol;
    let Some((target, t)) = target.and_then(|n| unit_tables.get(n).map(|t| (n, t))) else {
        return HashMap::new();
    };
    // #2052: the events in the target's scope, its own and its consumed
    // contexts', for an events service's `bynk.test.event_not_subscribed`.
    let scope_events: std::collections::BTreeSet<String> = std::iter::once(target)
        .chain(consumed.iter().map(String::as_str))
        .filter_map(|u| unit_tables.get(u))
        .flat_map(|u| u.events.keys().cloned())
        .collect();
    t.services
        .iter()
        .map(|(name, decl)| {
            let protocol = match &decl.protocol {
                ServiceProtocol::Call => None,
                ServiceProtocol::Http => Some("http".to_string()),
                ServiceProtocol::Cron => Some("cron".to_string()),
                ServiceProtocol::Queue { .. } => Some("queue".to_string()),
                ServiceProtocol::WebSocket { .. } => Some("websocket".to_string()),
                ServiceProtocol::Events { .. } => Some("events".to_string()),
            };
            let handlers = decl
                .handlers
                .iter()
                .map(|h| checker::TestHandler {
                    kind: h.kind.clone(),
                    params: h.params.clone(),
                    by_clause: h.by_clause.clone(),
                    return_type: h.return_type.clone(),
                    span: h.span,
                })
                .collect();
            let (known_events, event_publisher) = match &decl.protocol {
                ServiceProtocol::Events {
                    event_type: TypeRef::Named(e),
                    ..
                } => (
                    scope_events.clone(),
                    Some(event_publisher(target, &e.name, unit_tables, consumed)),
                ),
                _ => (Default::default(), None),
            };
            (
                name.clone(),
                checker::TestServiceSig {
                    protocol,
                    handlers,
                    known_events,
                    event_publisher,
                },
            )
        })
        .collect()
}

/// #291: the platform capabilities the target flattens in from a consumed
/// adapter (`consumes bynk { Logger }`) — seams of the unit under test, like
/// the capabilities it declares — with their declarations, sorted by name.
fn flattened_platform_capabilities<'a>(
    unit_tables: &'a HashMap<String, UnitTable>,
    resolved: &ResolvedCommons,
) -> Vec<(&'a String, &'a CapabilityDecl)> {
    let mut out: Vec<_> = resolved
        .cross_context
        .flattened_caps
        .iter()
        .filter_map(|(cap, owner)| {
            let t = unit_tables.get(owner)?;
            if t.kind != Some(UnitKind::Adapter) {
                return None;
            }
            t.capabilities.get_key_value(cap)
        })
        .collect();
    out.sort_by_key(|(cap, _)| cap.as_str());
    out
}

/// #291: a platform capability the target flattens in from a consumed
/// **adapter** (`bynk`'s `Logger`, by `consumes bynk { Logger }`) is a seam of
/// the unit under test too, so a test body may observe it (`expect Logger.info
/// called once`), as it may already `stub` it. A capability the target declares
/// itself wins on a name clash.
fn add_consumed_adapter_capabilities(
    map: &mut HashMap<String, checker::CapabilityInfo>,
    unit_tables: &HashMap<String, UnitTable>,
    resolved: &ResolvedCommons,
    tys: &Arc<Types>,
) {
    for (name, decl) in flattened_platform_capabilities(unit_tables, resolved) {
        if map.contains_key(name) {
            continue;
        }
        let ops = decl
            .ops
            .iter()
            .map(|op| build_capability_op_info(op, &resolved.types, tys))
            .collect();
        map.insert(
            name.clone(),
            checker::CapabilityInfo {
                name: name.clone(),
                ops,
                derived_from: None,
            },
        );
    }
}

/// Type-check a test `case`/`property` body against the target unit's privileges,
/// returning the inferred `expr_types` map and the `Callee` classification
/// recorded alongside it. The **check** path feeds real diagnostic/ref sinks;
/// the **emit** path reuses it with throwaway sinks to give the case-body
/// lowering full type information (so collection kernels — notably
/// `trace(Cap.op)`'s `List[…]` methods — dispatch on the receiver's checked
/// type) *and* full `Callee` information (P6.21 review: the emit path's own
/// `callees` accumulator was previously built here and silently discarded —
/// `bynk-emit`'s `synthetic_typed_commons_for_target` never received it, so
/// `Callee::Intrinsic`/`Store`/etc. were never recorded for anything inside a
/// `.test.bynk` body, even though this function computed them correctly all
/// along).
#[allow(clippy::too_many_arguments)]
pub fn typecheck_case_body(
    target_name: &str,
    body: &Block,
    unit_span: Span,
    unit_tables: &HashMap<String, UnitTable>,
    resolved: &ResolvedCommons,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    // v0.119: bindings already in scope for the body — empty for a `case`, the
    // `run: List[Step]` binding for a history property.
    initial_scope: HashMap<String, checker::TyId>,
    tys: &Arc<Types>,
) -> (
    HashMap<ExprId, checker::TypedExpr>,
    HashMap<ExprId, checker::Callee>,
) {
    let mut expr_types: HashMap<ExprId, checker::TypedExpr> = HashMap::new();
    let mut callees: HashMap<ExprId, checker::Callee> = HashMap::new();
    // Synthesise an Effect[Result[(), ValidationError]] return type as a
    // stand-in for Effect[Result[(), ExpectationError]]. v0.7 doesn't model an
    // explicit ExpectationError type — the runtime catches it instead.
    let synthetic_return = TypeRef::Effect(
        Box::new(TypeRef::Result(
            Box::new(TypeRef::Unit(unit_span)),
            Box::new(TypeRef::ValidationError(unit_span)),
            unit_span,
        )),
        unit_span,
    );

    // Capabilities of the target context, if any (so the test body can
    // call capabilities directly when targeting a context).
    let mut capability_info_map: HashMap<String, checker::CapabilityInfo> = HashMap::new();
    if let Some(table) = unit_tables.get(target_name) {
        for (name, decl) in &table.capabilities {
            let ops = decl
                .ops
                .iter()
                .map(|op| build_capability_op_info(op, &resolved.types, tys))
                .collect();
            capability_info_map.insert(
                name.clone(),
                checker::CapabilityInfo {
                    name: name.clone(),
                    ops,
                    derived_from: None,
                },
            );
        }
    }
    add_consumed_adapter_capabilities(&mut capability_info_map, unit_tables, resolved, tys);

    // All declared capabilities are implicitly "given" inside a test body;
    // the test runner wires them via the mocked deps. We feed the same map
    // to both `capabilities` (in-scope) and `declared_capabilities`.
    let given_declared: Vec<String> = capability_info_map.keys().cloned().collect();

    let return_ty = checker::resolve_type_ref(&synthetic_return, &resolved.types, tys).unwrap();
    let return_ty_span = unit_span;
    // Test bodies record no hints (out of v0.27 scope) — a throwaway sink.
    let mut no_hints = HintSink::new();
    let mut no_locals = LocalsSink::new();
    // Test bodies record no capability requirements either — muted sink.
    let mut no_requirements = RequirementSink::new();
    let _ = checker::check_body(
        resolved,
        body,
        return_ty,
        return_ty_span,
        initial_scope,
        checker::CapabilityCtx {
            capabilities: capability_info_map.clone(),
            declared_capabilities: capability_info_map,
            given_remaining: given_declared.iter().cloned().collect(),
            given_used: HashSet::new(),
            given_entries: Vec::new(),
            given_anchor: None,
        },
        target_test_services(
            Some(target_name),
            unit_tables,
            &resolved.cross_context.consumed_contexts,
        ),
        target_test_actors(unit_tables.get(target_name)),
        None,
        checker::CheckSinks {
            tys,
            expr_types: &mut expr_types,
            errors,
            refs,
            hints: &mut no_hints,
            locals: &mut no_locals,
            requirements: &mut no_requirements,
            callees: &mut callees,
        },
    );
    (expr_types, callees)
}

#[allow(clippy::too_many_arguments)]
fn check_test_case_body(
    target_name: &str,
    target_kind: UnitKind,
    case: &Case,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) {
    let Some((mut resolved, _)) = build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) else {
        return;
    };
    register_call_record_types(&mut resolved, target_name, unit_tables);
    let _ = target_kind;
    let _ = typecheck_case_body(
        target_name,
        &case.body,
        case.span,
        unit_tables,
        &resolved,
        errors,
        refs,
        HashMap::new(),
        tys,
    );
    // Don't enforce return-type equality; the test runner discards the
    // tail expression and recovers success/failure from expectation outcome.
    // Don't enforce "every given used" — capabilities are implicitly
    // available in a test body.

    // v0.115: flag a `case` that merely restates a contract already declared at
    // the source (`bynk.contract.restated_by_test`) — an `expect` that is
    // α-equivalent to an `ensures` clause over the same bound arguments. The dev
    // guard and the runner attack already check it. Conservative: under-flagging
    // is acceptable, over-flagging is not.
    check_restated_contract(&case.body, &resolved, errors);
}

/// v0.115: within a test body, flag an `expect` that re-states a contract's
/// `ensures`. Fires only on the clearest restatement: a binding `let r = f(args)`
/// (or `r <- f(args)`) of a contracted free function's result, followed by an
/// `expect E` that is α-equivalent to one of `f`'s `ensures` predicates under the
/// substitution `result → r`, `params → args`. Syntactic — never semantic — so a
/// merely-equivalent (but differently written) test is not flagged.
fn check_restated_contract(
    body: &Block,
    resolved: &ResolvedCommons,
    errors: &mut Vec<CompileError>,
) {
    // Map each locally-bound name to the contracted free function + call args it
    // was bound from (`let r = f(a, b)`).
    let mut bound: HashMap<String, (&FnDecl, &[Expr])> = HashMap::new();
    for stmt in &body.statements {
        let (name, value) = match stmt {
            Statement::Let(l) | Statement::EffectLet(l) => (&l.name.name, &l.value),
            _ => continue,
        };
        if let ExprKind::Call {
            name: callee, args, ..
        } = &value.kind
            && let Some(f) = resolved.fns.get(&callee.name)
            && matches!(&f.name, FnName::Free(_))
            && !f.ensures.is_empty()
            && f.params.len() == args.len()
        {
            bound.insert(name.clone(), (f, args.as_slice()));
        }
    }
    if bound.is_empty() {
        return;
    }
    for stmt in &body.statements {
        let Statement::Expect(e) = stmt else { continue };
        for (result_name, (f, args)) in &bound {
            // subst: result → r, each param → its call argument.
            let result_ident = Expr {
                id: ExprId::SYNTHETIC,
                kind: ExprKind::Ident(Ident {
                    name: result_name.clone(),
                    span: e.span,
                }),
                span: e.span,
            };
            let mut subst: HashMap<&str, &Expr> = HashMap::new();
            subst.insert("result", &result_ident);
            for (p, a) in f.params.iter().zip(args.iter()) {
                subst.insert(p.name.name.as_str(), a);
            }
            for c in &f.ensures {
                if expr_alpha_eq_subst(&c.predicate, &e.value, &subst) {
                    let FnName::Free(fname) = &f.name else {
                        continue;
                    };
                    errors.push(
                        CompileError::new(
                            "bynk.contract.restated_by_test",
                            e.span,
                            format!(
                                "this `expect` restates the `ensures {}` contract of `{}`, which is already checked at every call and by the runner",
                                c.name.name, fname.name
                            ),
                        )
                        .with_note(
                            "a contract is checked everywhere for free — delete the restating test, or keep a `case` only for a specific witnessed value",
                        ),
                    );
                    break;
                }
            }
        }
    }
}

/// Structural (α-)equality of two predicate expressions, ignoring spans, where a
/// bare identifier in `pattern` that appears in `subst` must match the
/// corresponding substituted expression in `actual` (the rest compares by shape).
/// Deliberately conservative — only the operators/leaves a contract predicate can
/// contain are compared; anything unrecognised is unequal.
fn expr_alpha_eq_subst(pattern: &Expr, actual: &Expr, subst: &HashMap<&str, &Expr>) -> bool {
    if let ExprKind::Ident(id) = &pattern.kind
        && let Some(replacement) = subst.get(id.name.as_str())
    {
        return expr_struct_eq(replacement, actual);
    }
    match (&pattern.kind, &actual.kind) {
        (ExprKind::Ident(a), ExprKind::Ident(b)) => a.name == b.name,
        (ExprKind::IntLit { value: a, .. }, ExprKind::IntLit { value: b, .. }) => a == b,
        (ExprKind::BoolLit(a), ExprKind::BoolLit(b)) => a == b,
        (ExprKind::StrLit(a), ExprKind::StrLit(b)) => a == b,
        (ExprKind::Paren(a), _) => expr_alpha_eq_subst(a, actual, subst),
        (_, ExprKind::Paren(b)) => expr_alpha_eq_subst(pattern, b, subst),
        (ExprKind::BinOp(oa, la, ra), ExprKind::BinOp(ob, lb, rb)) => {
            oa == ob && expr_alpha_eq_subst(la, lb, subst) && expr_alpha_eq_subst(ra, rb, subst)
        }
        (ExprKind::UnaryOp(oa, a), ExprKind::UnaryOp(ob, b)) => {
            oa == ob && expr_alpha_eq_subst(a, b, subst)
        }
        (
            ExprKind::MethodCall {
                receiver: ra,
                method: ma,
                args: aa,
                ..
            },
            ExprKind::MethodCall {
                receiver: rb,
                method: mb,
                args: ab,
                ..
            },
        ) => {
            ma.name == mb.name
                && aa.len() == ab.len()
                && expr_alpha_eq_subst(ra, rb, subst)
                && aa
                    .iter()
                    .zip(ab.iter())
                    .all(|(x, y)| expr_alpha_eq_subst(x, y, subst))
        }
        _ => false,
    }
}

/// Plain structural equality of two expressions ignoring spans — used to compare
/// a substituted argument against its use in the test predicate.
fn expr_struct_eq(a: &Expr, b: &Expr) -> bool {
    match (&a.kind, &b.kind) {
        (ExprKind::Ident(x), ExprKind::Ident(y)) => x.name == y.name,
        (ExprKind::IntLit { value: x, .. }, ExprKind::IntLit { value: y, .. }) => x == y,
        (ExprKind::BoolLit(x), ExprKind::BoolLit(y)) => x == y,
        (ExprKind::StrLit(x), ExprKind::StrLit(y)) => x == y,
        (ExprKind::Paren(x), _) => expr_struct_eq(x, b),
        (_, ExprKind::Paren(y)) => expr_struct_eq(a, y),
        (ExprKind::BinOp(oa, la, ra), ExprKind::BinOp(ob, lb, rb)) => {
            oa == ob && expr_struct_eq(la, lb) && expr_struct_eq(ra, rb)
        }
        (ExprKind::UnaryOp(oa, x), ExprKind::UnaryOp(ob, y)) => oa == ob && expr_struct_eq(x, y),
        (
            ExprKind::MethodCall {
                receiver: ra,
                method: ma,
                args: aa,
                ..
            },
            ExprKind::MethodCall {
                receiver: rb,
                method: mb,
                args: ab,
                ..
            },
        ) => {
            ma.name == mb.name
                && aa.len() == ab.len()
                && expr_struct_eq(ra, rb)
                && aa.iter().zip(ab.iter()).all(|(x, y)| expr_struct_eq(x, y))
        }
        _ => false,
    }
}

/// v0.114: the recursion cap for property-binding generability (mirrors the
/// checker's `MOCK_DEPTH` for bare `Val`).
pub const PROP_GEN_DEPTH: u32 = 12;

/// Whether a `for all x: T` binding's type is refinement-generable: refined
/// types must not carry a `Matches` predicate (no refinement-driven generator),
/// and sums/records must have every component recursively generable within the
/// depth cap. Mirrors the checker's `can_mock_bare`.
pub fn prop_binding_generable(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    depth: u32,
    tys: &Arc<Types>,
) -> bool {
    if depth == 0 {
        return false;
    }
    match &*tys.get(ty) {
        checker::Ty::Base(_) => true,
        checker::Ty::Named { name, .. } => {
            let Some(decl) = types.get(name) else {
                return false;
            };
            match &decl.body {
                TypeBody::Refined { refinement, .. } | TypeBody::Opaque { refinement, .. } => {
                    !refinement.as_ref().is_some_and(|r| {
                        r.predicates
                            .iter()
                            .any(|p| matches!(p.kind, PredKind::Matches(_)))
                    })
                }
                TypeBody::Sum(s) => s.variants.first().is_some_and(|v| {
                    v.payload.iter().all(|f| {
                        checker::resolve_type_ref(&f.type_ref, types, tys)
                            .is_some_and(|t| prop_binding_generable(t, types, depth - 1, tys))
                    })
                }),
                TypeBody::Record(r) => r.fields.iter().all(|f| {
                    checker::resolve_type_ref(&f.type_ref, types, tys)
                        .is_some_and(|t| prop_binding_generable(t, types, depth - 1, tys))
                }),
            }
        }
        _ => false,
    }
}

/// The refinement of a resolved refined/opaque named type, if any — used by the
/// conservative restates-refinement check.
fn named_refinement<'a>(
    ty: checker::TyId,
    types: &'a HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> Option<&'a Refinement> {
    let node = tys.get(ty);
    let checker::Ty::Named { name, .. } = &*node else {
        return None;
    };
    match &types.get(name)?.body {
        TypeBody::Refined { refinement, .. } | TypeBody::Opaque { refinement, .. } => {
            refinement.as_ref()
        }
        _ => None,
    }
}

/// v0.114 (DECISION P): does `pred` merely restate a refinement `bound_var`
/// already guarantees? A **conservative, syntactic** check — it fires only when
/// the predicate is exactly the refinement over the bound variable, never
/// guessing (under-flagging is acceptable; over-flagging is not). Handles the
/// `Positive` (`v > 0` / `v >= 1`) and `NonNegative` (`v >= 0`) numeric cases.
fn predicate_restates_refinement(pred: &Expr, bound_var: &str, refinement: &Refinement) -> bool {
    let ExprKind::BinOp(op, lhs, rhs) = &pred.kind else {
        return false;
    };
    // `<var> <op> <int-literal>` only.
    let ExprKind::Ident(id) = &lhs.kind else {
        return false;
    };
    if id.name != bound_var {
        return false;
    }
    let ExprKind::IntLit { value: n, .. } = &rhs.kind else {
        return false;
    };
    let n = *n;
    let positive = refinement
        .predicates
        .iter()
        .any(|p| matches!(p.kind, PredKind::Positive));
    let non_negative = refinement
        .predicates
        .iter()
        .any(|p| matches!(p.kind, PredKind::NonNegative));
    match op {
        // `v > 0` / `v >= 1` restate `Positive`.
        BinOp::Gt if n == 0 => positive,
        BinOp::GtEq if n == 1 => positive,
        // `v >= 0` restates `NonNegative`.
        BinOp::GtEq if n == 0 => non_negative,
        _ => false,
    }
}

/// v0.119 (DECISION D): which state-projection rewrite maps a history predicate
/// back into the space an `invariant` / `transition` is written in.
#[derive(Clone, Copy)]
enum HistoryRestate {
    /// An `invariant` reads bare state fields: `s.new.F` ≡ `F`.
    Invariant,
    /// A `transition` reads `old` / `new`: `s.old` ≡ `old`, `s.new` ≡ `new`.
    Transition,
}

/// `Some(field)` when `e` is `s.new.<field>` (the reached-state projection an
/// invariant-restating history predicate uses).
fn as_new_field<'a>(e: &'a Expr, s: &str) -> Option<&'a str> {
    let ExprKind::FieldAccess { receiver, field } = &e.kind else {
        return None;
    };
    let ExprKind::FieldAccess {
        receiver: inner,
        field: which,
    } = &receiver.kind
    else {
        return None;
    };
    let ExprKind::Ident(id) = &inner.kind else {
        return None;
    };
    (id.name == s && which.name == "new").then_some(field.name.as_str())
}

/// `Some("old"|"new")` when `e` is `s.old` / `s.new` (the step projections a
/// transition-restating history predicate uses).
fn as_step_root<'a>(e: &'a Expr, s: &str) -> Option<&'a str> {
    let ExprKind::FieldAccess { receiver, field } = &e.kind else {
        return None;
    };
    let ExprKind::Ident(id) = &receiver.kind else {
        return None;
    };
    (id.name == s && (field.name == "old" || field.name == "new")).then_some(field.name.as_str())
}

/// Conservative, span-insensitive structural match (DECISION D): does the history
/// predicate `body` (over the step binding `s`) restate the declared predicate
/// `decl`, modulo the `mode` state-projection rewrite? Under-flags by design — any
/// construct not modelled here compares unequal, so a valid test is never blocked.
fn history_pred_matches(body: &Expr, s: &str, decl: &Expr, mode: HistoryRestate) -> bool {
    // Leaf equivalences the rewrite establishes.
    match mode {
        HistoryRestate::Invariant => {
            if let (Some(f), ExprKind::Ident(id)) = (as_new_field(body, s), &decl.kind) {
                return f == id.name;
            }
        }
        HistoryRestate::Transition => {
            if let (Some(root), ExprKind::Ident(id)) = (as_step_root(body, s), &decl.kind) {
                return root == id.name;
            }
        }
    }
    match (&body.kind, &decl.kind) {
        (ExprKind::Paren(x), _) => history_pred_matches(x, s, decl, mode),
        (_, ExprKind::Paren(y)) => history_pred_matches(body, s, y, mode),
        (ExprKind::IntLit { value: x, .. }, ExprKind::IntLit { value: y, .. }) => x == y,
        (ExprKind::BoolLit(x), ExprKind::BoolLit(y)) => x == y,
        (ExprKind::StrLit(x), ExprKind::StrLit(y)) => x == y,
        (ExprKind::Ident(x), ExprKind::Ident(y)) => x.name == y.name,
        (ExprKind::None, ExprKind::None) => true,
        (ExprKind::Some(x), ExprKind::Some(y)) => history_pred_matches(x, s, y, mode),
        (ExprKind::UnaryOp(o1, x), ExprKind::UnaryOp(o2, y)) => {
            o1 == o2 && history_pred_matches(x, s, y, mode)
        }
        // #2018: `x is Variant` — a transition over an enum state (`old.status is
        // Closed implies new.status is Closed`) is written with `is`.
        (
            ExprKind::Is {
                value: v1,
                pattern: p1,
            },
            ExprKind::Is {
                value: v2,
                pattern: p2,
            },
        ) => history_pattern_matches(p1, p2) && history_pred_matches(v1, s, v2, mode),
        (ExprKind::BinOp(o1, l1, r1), ExprKind::BinOp(o2, l2, r2)) => {
            o1 == o2
                && history_pred_matches(l1, s, l2, mode)
                && history_pred_matches(r1, s, r2, mode)
        }
        (
            ExprKind::FieldAccess {
                receiver: r1,
                field: f1,
            },
            ExprKind::FieldAccess {
                receiver: r2,
                field: f2,
            },
        ) => f1.name == f2.name && history_pred_matches(r1, s, r2, mode),
        (
            ExprKind::MethodCall {
                receiver: r1,
                method: m1,
                args: a1,
                ..
            },
            ExprKind::MethodCall {
                receiver: r2,
                method: m2,
                args: a2,
                ..
            },
        ) => {
            m1.name == m2.name
                && a1.len() == a2.len()
                && history_pred_matches(r1, s, r2, mode)
                && a1
                    .iter()
                    .zip(a2)
                    .all(|(x, y)| history_pred_matches(x, s, y, mode))
        }
        (
            ExprKind::Call {
                name: n1, args: a1, ..
            },
            ExprKind::Call {
                name: n2, args: a2, ..
            },
        ) => {
            n1.name == n2.name
                && a1.len() == a2.len()
                && a1
                    .iter()
                    .zip(a2)
                    .all(|(x, y)| history_pred_matches(x, s, y, mode))
        }
        _ => false,
    }
}

/// #2018: span-insensitive equality of the pattern in an `is` test, for
/// [`history_pred_matches`]. Conservative like the rest of the restate check: only
/// a literal or a payload-free variant (same name, same optional `Type.`
/// qualifier) is compared; any other pattern shape compares unequal.
fn history_pattern_matches(a: &Pattern, b: &Pattern) -> bool {
    match (a, b) {
        (Pattern::Literal { value: x, .. }, Pattern::Literal { value: y, .. }) => x == y,
        (
            Pattern::Variant {
                type_name: t1,
                variant: v1,
                bindings: b1,
                ..
            },
            Pattern::Variant {
                type_name: t2,
                variant: v2,
                bindings: b2,
                ..
            },
        ) => {
            b1.is_empty()
                && b2.is_empty()
                && v1.name == v2.name
                && t1.as_ref().map(|t| &t.name) == t2.as_ref().map(|t| &t.name)
        }
        _ => false,
    }
}

/// v0.119 (DECISION D): a history property that merely restates a snapshot/step
/// invariant is redundant — the driver only commits states the invariants already
/// admit. Recognise the canonical shape `for all run: History[A] { expect
/// run.all((s) => P) }` (or `.any`) whose `P` α-matches a declared
/// `invariant` (over `s.new`) or `transition` (over `s.old`/`s.new`). Returns the
/// body span to flag. Conservative — near-duplicates slip through by design.
fn history_restates_invariant(prop: &PropertyDecl, run_var: &str, agent: &AgentDecl) -> bool {
    let [stmt] = prop.forall.body.statements.as_slice() else {
        return false;
    };
    let Statement::Expect(e) = stmt else {
        return false;
    };
    // `run.all((s) => P)` / `run.any((s) => P)`.
    let ExprKind::MethodCall {
        receiver,
        method,
        args,
        ..
    } = &e.value.kind
    else {
        return false;
    };
    if method.name != "all" && method.name != "any" {
        return false;
    }
    let ExprKind::Ident(recv) = &receiver.kind else {
        return false;
    };
    if recv.name != run_var {
        return false;
    }
    let [arg] = args.as_slice() else {
        return false;
    };
    let ExprKind::Lambda(lam) = &arg.kind else {
        return false;
    };
    let [param] = lam.params.as_slice() else {
        return false;
    };
    let s = &param.name.name;
    agent
        .invariants
        .iter()
        .any(|inv| history_pred_matches(&lam.body, s, &inv.predicate, HistoryRestate::Invariant))
        || agent
            .transitions
            .iter()
            .any(|tr| history_pred_matches(&lam.body, s, &tr.predicate, HistoryRestate::Transition))
}

/// v0.119 (ADR 0155): the synthetic type names a `History[Agent]` binding
/// registers — a call sum, a step record, and a state record — all keyed off the
/// agent name so distinct agents never collide.
fn history_call_type_name(agent: &str) -> String {
    format!("__History_{agent}_Call")
}
fn history_step_type_name(agent: &str) -> String {
    format!("__History_{agent}_Step")
}
fn history_state_type_name(agent: &str) -> String {
    format!("__History_{agent}_State")
}

/// The `.call` variant tag for a handler: the handler name with its first letter
/// upper-cased (`spend` → `Spend`, `topUp` → `TopUp`). The reader matches this
/// with `is` / `match` (`s.call is Spend`).
pub fn history_variant_name(handler: &str) -> String {
    let mut chars = handler.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => handler.to_string(),
    }
}

/// The agent's drivable `on call` handlers — the ones a history sequences. Other
/// handler kinds (`http`/`cron`/`message`/`open`/`close`) are not RPC entry points
/// and are never part of a generated call-history.
pub fn history_handlers(agent: &AgentDecl) -> Vec<&Handler> {
    agent
        .handlers
        .iter()
        .filter(|h| matches!(h.kind, HandlerKind::Call) && h.method_name.is_some())
        .collect()
}

/// v0.119 (testing track slice 7, ADR 0155): type-check a `for all run:
/// History[Agent]` binding. The subject is a *run* of the agent — a generated,
/// driven call-history — bound as an ordinary `List[Step]`. Validates the
/// DECISION-B rules (agent-only, every handler parameter generable), registers the
/// synthetic call-sum / step / state record types into `resolved.types` so the
/// predicate's `List` + value surface (`.call is …`, `.old`/`.new`, `.accepted`)
/// type-checks, and returns the bound `List[Step]` type.
pub fn check_history_binding(
    inner: &TypeRef,
    span: Span,
    resolved: &mut ResolvedCommons,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) -> Result<checker::Ty, CompileError> {
    // DECISION B: only an agent has handlers to sequence and reachable states to
    // observe. `History[Value]` / `History[List[…]]` is `not_an_agent`.
    let TypeRef::Named(agent_id) = inner else {
        return Err(CompileError::new(
            "bynk.history.not_an_agent",
            span,
            format!(
                "`for all` cannot generate `History[{}]` — only an agent has handlers to sequence",
                ts_type_ref_display(inner)
            ),
        )
        .with_note("generate a driven call-history over an agent: `for all run: History[Agent]`"));
    };
    let Some(agent) = resolved.agents.get(&agent_id.name).cloned() else {
        return Err(CompileError::new(
            "bynk.history.not_an_agent",
            span,
            format!(
                "`for all run: History[{}]` names `{}`, which is not an agent in scope",
                agent_id.name, agent_id.name
            ),
        )
        .with_note(
            "only an agent (with handlers and reachable state) can be driven as a history",
        ));
    };
    refs.record(agent_id.span, SymbolKind::Type, &agent_id.name);

    let handlers = history_handlers(&agent);
    // DECISION B: the agent must be *drivable* — every handler parameter must be
    // refinement-generable (the same rule a value `for all` binding obeys), else
    // the runner cannot synthesise a call.
    for h in &handlers {
        for p in &h.params {
            let generable = checker::resolve_type_ref(&p.type_ref, &resolved.types, tys)
                .is_some_and(|t| prop_binding_generable(t, &resolved.types, PROP_GEN_DEPTH, tys));
            if !generable {
                return Err(CompileError::new(
                    "bynk.history.not_generable",
                    span,
                    format!(
                        "`History[{}]` cannot be driven — handler `{}`'s parameter `{}: {}` is not generable (e.g. a `Matches` refinement)",
                        agent_id.name,
                        h.method_name.as_ref().map(|m| m.name.as_str()).unwrap_or(""),
                        p.name.name,
                        ts_type_ref_display(&p.type_ref),
                    ),
                )
                .with_note(
                    "every handler parameter must be refinement-generable for the run to be seeded",
                ));
            }
        }
    }

    // Register the synthetic types (mirrors `register_call_record_types`). The
    // driver returns plain objects of exactly these shapes; the checker sees them
    // as ordinary record/sum types so `is`, field access, and `implies` apply
    // unchanged (the typed-step shape resolving the track's open question).
    let state_name = history_state_type_name(&agent_id.name);
    let call_name = history_call_type_name(&agent_id.name);
    let step_name = history_step_type_name(&agent_id.name);

    // `<Agent>State` — the agent's `Cell` fields, exactly as the emitted state
    // record (so `.old.balance` / `.new.balance` read a reached state).
    let state_fields: Vec<RecordField> = agent
        .store_fields
        .iter()
        .filter(|f| f.kind.head.name == "Cell" && f.kind.args.len() == 1)
        .map(|f| RecordField {
            documentation: None,
            trivia: Default::default(),
            name: f.name.clone(),
            type_ref: f.kind.args[0].clone(),
            refinement: None,
            init: None,
            span: f.span,
        })
        .collect();
    resolved.types.insert(
        state_name.clone(),
        Arc::new(TypeDecl {
            type_params: Vec::new(),
            name: Ident {
                name: state_name.clone(),
                span,
            },
            body: TypeBody::Record(RecordBody {
                trailing_comments: Default::default(),
                fields: state_fields,
                span,
            }),
            documentation: None,
            span,
            trivia: Trivia::default(),
        }),
    );

    // `.call` — a sum over the agent's handlers, each variant carrying the
    // handler's generated arguments (`Spend { amount }`, `TopUp { amount }`).
    let variants: Vec<Variant> = handlers
        .iter()
        .map(|h| {
            let hname = h.method_name.as_ref().expect("call handler has a name");
            Variant {
                documentation: None,
                trivia: Default::default(),
                name: Ident {
                    name: history_variant_name(&hname.name),
                    span: hname.span,
                },
                payload: h
                    .params
                    .iter()
                    .map(|p| VariantField {
                        documentation: None,
                        name: p.name.clone(),
                        type_ref: p.type_ref.clone(),
                        span: p.span,
                    })
                    .collect(),
                span: hname.span,
            }
        })
        .collect();
    resolved.types.insert(
        call_name.clone(),
        Arc::new(TypeDecl {
            type_params: Vec::new(),
            name: Ident {
                name: call_name.clone(),
                span,
            },
            body: TypeBody::Sum(SumBody {
                trailing_comments: Default::default(),
                variants,
                embeds: Vec::new(),
                span,
            }),
            documentation: None,
            span,
            trivia: Trivia::default(),
        }),
    );

    // A `Step` — the driven edge: which call ran (`.call`), whether it committed
    // (`.accepted`), and the committed `old` → `new` state pair.
    let step_fields = vec![
        RecordField {
            documentation: None,
            trivia: Default::default(),
            name: Ident {
                name: "call".to_string(),
                span,
            },
            type_ref: TypeRef::Named(Ident {
                name: call_name.clone(),
                span,
            }),
            refinement: None,
            init: None,
            span,
        },
        RecordField {
            documentation: None,
            trivia: Default::default(),
            name: Ident {
                name: "accepted".to_string(),
                span,
            },
            type_ref: TypeRef::Base(BaseType::Bool, span),
            refinement: None,
            init: None,
            span,
        },
        RecordField {
            documentation: None,
            trivia: Default::default(),
            name: Ident {
                name: "old".to_string(),
                span,
            },
            type_ref: TypeRef::Named(Ident {
                name: state_name.clone(),
                span,
            }),
            refinement: None,
            init: None,
            span,
        },
        RecordField {
            documentation: None,
            trivia: Default::default(),
            name: Ident {
                name: "new".to_string(),
                span,
            },
            type_ref: TypeRef::Named(Ident {
                name: state_name.clone(),
                span,
            }),
            refinement: None,
            init: None,
            span,
        },
    ];
    resolved.types.insert(
        step_name.clone(),
        Arc::new(TypeDecl {
            type_params: Vec::new(),
            name: Ident {
                name: step_name.clone(),
                span,
            },
            body: TypeBody::Record(RecordBody {
                trailing_comments: Default::default(),
                fields: step_fields,
                span,
            }),
            documentation: None,
            span,
            trivia: Trivia::default(),
        }),
    );

    Ok(checker::Ty::List(tys.intern(checker::Ty::Named {
        name: step_name,
        kind: checker::NamedKind::Record,
        args: Vec::new(),
    })))
}

/// v0.114: type-check a generative `property` — its `for all` bindings, the
/// optional `where` filter, and the predicate body — in the target's privileged
/// view. Bindings type each `x: T`; `where`/`expect` predicates type as pure
/// `Bool`; each binding's `T` must be refinement-generable (agents are rejected;
/// a `Matches` type must pin); and the body is flagged if it merely restates a
/// refinement (DECISION P). v0.119: a `for all run: History[Agent]` binding is a
/// driven call-history (the history rung — see [`check_history_binding`]).
#[allow(clippy::too_many_arguments)]
fn check_property_body(
    target_name: &str,
    target_kind: UnitKind,
    prop: &PropertyDecl,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
    tys: &Arc<Types>,
) {
    let Some((mut resolved, _)) = build_privileged_resolved(
        target_name,
        unit_tables,
        unit_uses,
        unit_consumes,
        unit_consumes_aliases,
    ) else {
        return;
    };
    register_call_record_types(&mut resolved, target_name, unit_tables);
    let _ = target_kind;

    // Bind each `for all x: T` into the predicate scope, checking generability.
    let mut binding_scope: HashMap<String, checker::TyId> = HashMap::new();
    let mut binding_types: Vec<(String, Option<checker::TyId>)> = Vec::new();
    // v0.119: the single `History[Agent]` binding (run-var, agent), for the
    // post-body `restates_invariant` check (DECISION D).
    let mut history_binding: Option<(String, AgentDecl)> = None;
    for b in &prop.forall.bindings {
        // v0.119 (ADR 0155): `for all run: History[Agent]` — the history rung. A
        // driven call-history, bound as an ordinary `List[Step]`.
        if let TypeRef::History(inner, hspan) = &b.type_ref {
            match check_history_binding(inner, *hspan, &mut resolved, refs, tys) {
                Ok(step_ty) => {
                    if let TypeRef::Named(agent_id) = &**inner
                        && let Some(agent) = resolved.agents.get(&agent_id.name)
                    {
                        history_binding = Some((b.name.name.clone(), agent.clone()));
                    }
                    binding_scope.insert(b.name.name.clone(), tys.intern(step_ty.clone()));
                    binding_types.push((b.name.name.clone(), Some(tys.intern(step_ty))));
                }
                Err(err) => {
                    errors.push(err);
                    binding_types.push((b.name.name.clone(), None));
                    // #1708: rejected, but still in scope (error-typed), so
                    // its uses in the body do not echo as unknown names.
                    binding_scope.insert(b.name.name.clone(), tys.intern(checker::Ty::Error));
                }
            }
            continue;
        }
        // Agents are not a value type — a fabricated state that satisfies every
        // invariant need not be reachable (DECISION P); reject up front.
        if let TypeRef::Named(id) = &b.type_ref
            && resolved.agents.contains_key(&id.name)
        {
            errors.push(
                CompileError::new(
                    "bynk.val.agent_not_generable",
                    b.type_ref.span(),
                    format!(
                        "`for all {}: {}` cannot generate an agent — a fabricated agent state need not be reachable",
                        b.name.name, id.name
                    ),
                )
                .with_note(
                    "generate behaviour over an agent via handler sequences (the history rung), not fabricated states",
                ),
            );
            binding_types.push((b.name.name.clone(), None));
            // #1708: rejected, but still in scope (error-typed), so
            // its uses in the body do not echo as unknown names.
            binding_scope.insert(b.name.name.clone(), tys.intern(checker::Ty::Error));
            continue;
        }
        let ty = match checker::resolve_type_ref(&b.type_ref, &resolved.types, tys) {
            Some(t) => {
                record_type_refs_in_property(&b.type_ref, &resolved, refs);
                t
            }
            None => {
                errors.push(CompileError::new(
                    "bynk.val.unknown_type",
                    b.type_ref.span(),
                    format!(
                        "`for all {}: {}` names a type that does not resolve",
                        b.name.name,
                        ts_type_ref_display(&b.type_ref)
                    ),
                ));
                binding_types.push((b.name.name.clone(), None));
                // #1708: rejected, but still in scope (error-typed), so
                // its uses in the body do not echo as unknown names.
                binding_scope.insert(b.name.name.clone(), tys.intern(checker::Ty::Error));
                continue;
            }
        };
        if !prop_binding_generable(ty, &resolved.types, PROP_GEN_DEPTH, tys) {
            errors.push(
                CompileError::new(
                    "bynk.val.needs_pin",
                    b.type_ref.span(),
                    format!(
                        "`for all {}: {}` cannot generate a value (e.g. a `Matches` refinement); a property cannot bind it",
                        b.name.name,
                        ts_type_ref_display(&b.type_ref)
                    ),
                )
                .with_note("supply the witness in a `case` with a pinned `Val[T](...)` instead"),
            );
        }
        binding_scope.insert(b.name.name.clone(), ty);
        binding_types.push((b.name.name.clone(), Some(ty)));
    }

    // Type the `where`/body predicates in the target's privileged view with the
    // bindings in scope — mirroring the `case` body context.
    let mut expr_types: HashMap<ExprId, checker::TypedExpr> = HashMap::new();
    let mut callees: HashMap<ExprId, checker::Callee> = HashMap::new();
    let unit_span = prop.span;
    let synthetic_return = TypeRef::Effect(
        Box::new(TypeRef::Result(
            Box::new(TypeRef::Unit(unit_span)),
            Box::new(TypeRef::ValidationError(unit_span)),
            unit_span,
        )),
        unit_span,
    );
    let mut capability_info_map: HashMap<String, checker::CapabilityInfo> = HashMap::new();
    if let Some(table) = unit_tables.get(target_name) {
        for (name, decl) in &table.capabilities {
            let ops = decl
                .ops
                .iter()
                .map(|op| build_capability_op_info(op, &resolved.types, tys))
                .collect();
            capability_info_map.insert(
                name.clone(),
                checker::CapabilityInfo {
                    name: name.clone(),
                    ops,
                    derived_from: None,
                },
            );
        }
    }
    add_consumed_adapter_capabilities(&mut capability_info_map, unit_tables, &resolved, tys);
    let given_declared: Vec<String> = capability_info_map.keys().cloned().collect();
    let return_ty = checker::resolve_type_ref(&synthetic_return, &resolved.types, tys).unwrap();
    let return_ty_span = prop.span;
    let mut no_hints = HintSink::new();
    let mut no_locals = LocalsSink::new();
    let mut no_requirements = RequirementSink::new();
    // The optional `where` filter is checked first (against `Bool`), sharing
    // `check_body`'s `Ctx` with the body below; the body is the one predicate
    // surface: `expect`s self-check as `Bool`.
    let _ = checker::check_body(
        &resolved,
        &prop.forall.body,
        return_ty,
        return_ty_span,
        binding_scope,
        checker::CapabilityCtx {
            capabilities: capability_info_map.clone(),
            declared_capabilities: capability_info_map,
            given_remaining: given_declared.iter().cloned().collect(),
            given_used: HashSet::new(),
            given_entries: Vec::new(),
            given_anchor: None,
        },
        target_test_services(
            Some(target_name),
            unit_tables,
            &resolved.cross_context.consumed_contexts,
        ),
        target_test_actors(unit_tables.get(target_name)),
        prop.forall.where_pred.as_ref(),
        checker::CheckSinks {
            tys,
            expr_types: &mut expr_types,
            errors,
            refs,
            hints: &mut no_hints,
            locals: &mut no_locals,
            requirements: &mut no_requirements,
            callees: &mut callees,
        },
    );

    // Conservative restates-refinement flag: a single-binding property whose
    // body is exactly `expect <pred>` restating the bound var's refinement.
    if let [(var, Some(ty))] = binding_types.as_slice()
        && let Some(refinement) = named_refinement(*ty, &resolved.types, tys)
        && let [stmt] = prop.forall.body.statements.as_slice()
        && let Statement::Expect(e) = stmt
        && predicate_restates_refinement(&e.value, var, refinement)
    {
        errors.push(
            CompileError::new(
                "bynk.property.restates_refinement",
                prop.forall.body.span,
                format!(
                    "property `{}` merely re-checks a refinement type `{}` already guarantees",
                    prop.name,
                    ty.display(tys)
                ),
            )
            .with_note(
                "a property earns its keep by asserting behaviour over valid inputs, not by restating the type's refinement",
            ),
        );
    }

    // v0.119 (DECISION D): a history property that merely restates a declared
    // `invariant` / `transition` re-checks a guarantee every reached state already
    // has (the driver only commits admissible states). Conservative — near-
    // duplicates slip through by design.
    if let Some((run_var, agent)) = &history_binding
        && history_restates_invariant(prop, run_var, agent)
    {
        errors.push(
            CompileError::new(
                "bynk.history.restates_invariant",
                prop.forall.body.span,
                format!(
                    "history property `{}` merely re-checks a guarantee agent `{}`'s `invariant`/`transition` already enforces on every reached state",
                    prop.name, agent.name.name
                ),
            )
            .with_note(
                "a history property earns its keep by asserting a cross-step protocol, not by restating a per-state invariant",
            ),
        );
    }
}

/// Record type references named by a `for all` binding so cross-file edges and
/// go-to-definition resolve for a property's generated types.
fn record_type_refs_in_property(
    type_ref: &TypeRef,
    resolved: &ResolvedCommons,
    refs: &mut RefSink,
) {
    checker::record_type_refs(type_ref, &resolved.types, &HashSet::new(), refs);
}

/// #1814: a test suite's composed symbol tables, closed over the types its
/// imported declarations reach. `hidden` names each type the closure added,
/// with its owning commons: resolvable in the view, never nameable by the
/// suite (`resolver::check_hidden_type_names_in_suite`), and imported by the
/// test scaffold from that commons.
#[derive(Clone, Default)]
pub struct SuiteView {
    pub types: HashMap<String, Arc<TypeDecl>>,
    pub fns: HashMap<String, Arc<FnDecl>>,
    pub methods: HashMap<String, ResolverMethodTable>,
    pub hidden: BTreeMap<String, String>,
    /// Review of #1814: each reached name the view already binds to another
    /// declaration (`project_model::close_view_types`).
    pub conflicts: Vec<crate::project_model::ViewConflict>,
    /// #1883, slice 3 ([DECISION F]): the assets a case may name — those the
    /// target's Worker ships: the target's own and its `uses`d commons'
    /// (content-hashed and fixed-path), plus the content-hashed assets of the
    /// contexts it consumes. An asset name types as `Asset`;
    /// `svc.GET(asset)` fetches the file.
    pub assets: HashMap<String, bynk_syntax::ast::AssetDecl>,
}

impl SuiteView {
    /// #1883, slice 3: add the assets `unit` sees — its own and those of the
    /// commons it `uses`, one level, as for functions — first name winning.
    fn merge_assets(
        &mut self,
        unit: &str,
        unit_tables: &HashMap<String, UnitTable>,
        unit_uses: &HashMap<String, Vec<String>>,
    ) {
        self.merge_assets_where(unit, unit_tables, unit_uses, |_| true);
    }

    /// [`Self::merge_assets`], keeping only the declarations `keep` admits.
    fn merge_assets_where(
        &mut self,
        unit: &str,
        unit_tables: &HashMap<String, UnitTable>,
        unit_uses: &HashMap<String, Vec<String>>,
        keep: impl Fn(&bynk_syntax::ast::AssetDecl) -> bool,
    ) {
        let own = unit_tables.get(unit).into_iter();
        let used = unit_uses
            .get(unit)
            .into_iter()
            .flatten()
            .filter_map(|u| unit_tables.get(u));
        for table in own.chain(used) {
            for (name, decl) in table.assets.iter().filter(|(_, d)| keep(d)) {
                self.assets
                    .entry(name.clone())
                    .or_insert_with(|| decl.clone());
            }
        }
    }

    /// What the closure found, without the tables.
    pub fn reach(&self) -> crate::project_model::ViewReach {
        crate::project_model::ViewReach {
            hidden: self.hidden.clone(),
            conflicts: self.conflicts.clone(),
        }
    }

    /// Merge `t`'s types and methods (and its fns, when `with_fns`) into the
    /// view, first declaration of a name winning; `with_statics` brings static
    /// methods too.
    fn merge(&mut self, t: &UnitTable, with_fns: bool, with_statics: bool) {
        for (n, d) in &t.types {
            self.types.entry(n.clone()).or_insert_with(|| d.clone());
        }
        if with_fns {
            for (n, f) in &t.fns {
                self.fns.entry(n.clone()).or_insert_with(|| f.clone());
            }
        }
        for (n, mt) in &t.methods {
            let entry = self.methods.entry(n.clone()).or_default();
            for (m, decl) in &mt.instance {
                entry
                    .instance
                    .entry(m.clone())
                    .or_insert_with(|| decl.clone());
            }
            if with_statics {
                for (m, decl) in &mt.statics {
                    entry
                        .statics
                        .entry(m.clone())
                        .or_insert_with(|| decl.clone());
                }
            }
        }
    }

    /// #1814: close the view over what the declarations of `imported` reach
    /// (`project_model::close_view_types`), recording what it adds.
    fn close(
        &mut self,
        imported: &[String],
        unit_tables: &HashMap<String, UnitTable>,
        unit_uses: &HashMap<String, Vec<String>>,
    ) {
        let reach = crate::project_model::close_view_types(
            imported,
            unit_tables,
            unit_uses,
            &mut self.types,
            &self.fns,
            &mut self.methods,
        );
        self.hidden = reach.hidden;
        self.conflicts = reach.conflicts;
    }
}

/// `owning_unit`'s privileged view: its types, fns, methods, plus those of
/// every commons it `uses`, plus the types and instance methods of every
/// consumed context. #1814: closed over the types those imported declarations
/// reach, as the unit's own table is, so a case is checked against the same
/// types the target is.
pub fn privileged_view(
    owning_unit: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
) -> Option<SuiteView> {
    let local = unit_tables.get(owning_unit)?;
    let mut view = SuiteView {
        types: local.types.clone(),
        fns: local.fns.clone(),
        methods: local.methods.clone(),
        ..SuiteView::default()
    };
    let mut imported: Vec<String> = Vec::new();
    for t in unit_uses.get(owning_unit).into_iter().flatten() {
        if let Some(used) = unit_tables.get(t) {
            view.merge(used, true, true);
            imported.push(t.clone());
        }
    }
    // Consumed-context types come in too: every one, not only the exported
    // ones, since the view is privileged. Review of #1814: so a reached name
    // can bind here to an unexported type the target's own table never
    // binds; `check_test_bodies` reports that.
    for t in unit_consumes.get(owning_unit).into_iter().flatten() {
        if let Some(used) = unit_tables.get(t) {
            view.merge(used, false, false);
            imported.push(t.clone());
        }
    }
    view.merge_assets(owning_unit, unit_tables, unit_uses);
    // Review of #1923: the content-hashed assets of each consumed context
    // too — the target's Worker ships them (`assets::deployed_assets`), so a
    // fragment a consumed service returns resolves. Its fixed paths stay
    // with its own Worker, so they are not nameable here.
    for consumed in unit_consumes.get(owning_unit).into_iter().flatten() {
        view.merge_assets_where(consumed, unit_tables, unit_uses, |d| d.at.is_none());
    }
    // #1925: a function or asset the target hides behind its own declaration
    // of another kind is out of scope here too, as in the target's module
    // (`project_model::shadowed_imports`): a case names the target's own.
    let hidden_by_local = |n: &String| crate::symbols::local_value_kind(local, n).is_some();
    view.fns
        .retain(|n, _| local.fns.contains_key(n) || !hidden_by_local(n));
    view.assets
        .retain(|n, _| local.assets.contains_key(n) || !hidden_by_local(n));
    view.close(&imported, unit_tables, unit_uses);
    Some(view)
}

/// #1814: a `system` case's harness view: types, fns and methods from the
/// suite's `uses` commons (for constructing arguments), plus each
/// participant's types and methods (so return types rebrand and variant
/// patterns resolve), closed over the types those declarations reach.
pub fn integration_view(
    uses_targets: &[String],
    participants: &[String],
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
) -> SuiteView {
    let mut view = SuiteView::default();
    let mut imported: Vec<String> = Vec::new();
    for (units, with_fns) in [(uses_targets, true), (participants, false)] {
        for u in units {
            if let Some(t) = unit_tables.get(u) {
                view.merge(t, with_fns, true);
                imported.push(u.clone());
            }
        }
    }
    // #1883, slice 3: the assets the target's Worker ships, mirroring
    // `assets::deployed_assets` — the target's own and its `uses`d commons'
    // (hashed and fixed), plus each other participant's content-hashed ones
    // (a consumed context, whose fragment may carry its `.url`). Review of
    // #1922: a participant's fixed path, or an asset of the suite's own
    // `uses` commons, is not served by the target's Worker, so it is not
    // nameable either (an ordinary unknown name rather than a `NotFound`).
    if let Some((target, others)) = participants.split_first() {
        view.merge_assets(target, unit_tables, unit_uses);
        for other in others {
            view.merge_assets_where(other, unit_tables, unit_uses, |d| d.at.is_none());
        }
        // #1925: an asset the target hides behind its own declaration of
        // another kind is not nameable, as in `privileged_view`. Its file
        // still ships: the commons that declares it may use it.
        if let Some(local) = unit_tables.get(target) {
            view.assets.retain(|n, _| {
                local.assets.contains_key(n) || crate::symbols::local_value_kind(local, n).is_none()
            });
        }
    }
    view.close(&imported, unit_tables, unit_uses);
    view
}

/// Build a [`resolver::ResolvedCommons`] backed by `owning_unit`'s
/// [`privileged_view`]. The same shape used by the production pipeline.
/// Returns the [`ResolvedCommons`] plus what the view's closure found (#1814):
/// its hidden types and its conflicts.
pub fn build_privileged_resolved(
    owning_unit: &str,
    unit_tables: &HashMap<String, UnitTable>,
    unit_uses: &HashMap<String, Vec<String>>,
    unit_consumes: &HashMap<String, Vec<String>>,
    unit_consumes_aliases: &HashMap<String, HashMap<String, String>>,
) -> Option<(ResolvedCommons, crate::project_model::ViewReach)> {
    let local = unit_tables.get(owning_unit)?;
    let view = privileged_view(owning_unit, unit_tables, unit_uses, unit_consumes)?;
    let reach = view.reach();
    let assets = view.assets.clone();
    let SuiteView {
        types,
        fns,
        methods,
        ..
    } = view;
    let cross_context = build_cross_context_info(
        owning_unit,
        unit_consumes,
        unit_consumes_aliases,
        unit_uses,
        unit_tables,
    );
    let synthetic_commons = Commons {
        name: QualifiedName {
            parts: owning_unit
                .split('.')
                .map(|part| Ident {
                    name: part.to_string(),
                    span: Span::default(),
                })
                .collect(),
            span: Span::default(),
        },
        items: Vec::new(),
        uses: Vec::new(),
        documentation: None,
        form: CommonsForm::Brace,
        span: Span::default(),
        trivia: Trivia::default(),
        trailing_comments: Vec::new(),
    };
    let agents_for_resolved = unit_tables
        .get(owning_unit)
        .map(|t| t.agents.clone())
        .unwrap_or_default();
    let no_local_events = HashMap::new();
    let resolved = ResolvedCommons::new(
        synthetic_commons,
        types,
        &local.types,
        fns,
        methods,
        agents_for_resolved,
        // "Privileged" test/stub-body resolved — deliberately relaxed, not a
        // real context emission subject to the rebrand — so events stay
        // empty rather than reading `local`'s.
        &no_local_events,
        cross_context,
        HashMap::new(),
        false,
        HashSet::new(),
    )
    // #1883, slice 3: the target's assets, nameable and fetchable in a case.
    .with_assets(assets)
    // #1925: the imports the target hides, so a case naming one reports the
    // conflict as the target's own code would.
    .with_shadowed_imports(crate::project_model::shadowed_imports_in(
        local,
        unit_uses
            .get(owning_unit)
            .into_iter()
            .flatten()
            .filter_map(|t| unit_tables.get(t).map(|u| (t, u))),
    ));
    Some((resolved, reach))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The body of `service svc`'s single handler in a parsed context.
    fn handler_body(src: &str) -> Block {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lex");
        let unit = bynk_syntax::parser::parse_unit(&tokens, src).expect("parse");
        let SourceUnit::Context(ctx) = unit else {
            panic!("expected a context unit");
        };
        ctx.items
            .into_iter()
            .find_map(|item| match item {
                bynk_syntax::ast::CommonsItem::Service(s) => s.handlers.into_iter().next(),
                _ => None,
            })
            .expect("a service handler")
            .body
    }

    const SRC: &str = "context demo\n\
        service svc {\n\
          on call(n: Int) -> Effect[Int] {\n\
            let a <- first(n)\n\
            let b = if n > 0 { let c = second(n)\n c } else { third(n) }\n\
            fourth(a + b)\n\
          }\n\
        }\n";

    /// #1740 review: the walk is in source order (it used to run bottom-up), so
    /// "the first cross-context call" and the order errors are reported in
    /// both read top to bottom.
    #[test]
    fn exprs_in_order_is_source_order() {
        let body = handler_body(SRC);
        let calls: Vec<&str> = exprs_in_order(&body)
            .into_iter()
            .filter_map(|e| match &e.kind {
                ExprKind::Call { name, .. } => Some(name.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(calls, ["first", "second", "third", "fourth"]);
    }

    /// `blocks_deep` reaches a block nested in an `if` branch, so a `let` there
    /// (an agent binding, or a name that shadows a service) is seen.
    #[test]
    fn blocks_deep_reaches_nested_lets() {
        let body = handler_body(SRC);
        let lets: Vec<String> = blocks_deep(&body)
            .iter()
            .flat_map(|b| &b.statements)
            .filter_map(|s| match s {
                Statement::Let(l) | Statement::EffectLet(l) => Some(l.name.name.clone()),
                _ => None,
            })
            .collect();
        assert!(lets.contains(&"c".to_string()), "{lets:?}");
        assert!(lets.contains(&"a".to_string()) && lets.contains(&"b".to_string()));
    }

    /// The context of negative fixture `388_credential_needs_system`.
    const CART_API: &str = "context shop.api

type UserId = String where NonEmpty
actor User { auth = Bearer(secret = \"AUTH_SECRET\"), identity = UserId }

type Item = { sku: String }

service api from http {
  on POST(\"/cart\") (body: Item) -> Effect[HttpResult[Item]] by u: User {
    Created(body)
  }
}
";

    fn nobody_case(body: &str) -> crate::testkit::Analysed {
        crate::testkit::analyse(&[
            ("shop/api.bynk", CART_API),
            (
                "shop/tests/api.test.bynk",
                &format!(
                    "suite shop.api as system {{\n  case \"c\" as unit {{\n{body}\n  }}\n}}\n"
                ),
            ),
        ])
    }

    #[test]
    fn a_unit_case_calling_by_nobody_needs_system() {
        nobody_case(
            "    let r <- api.POST(\"/cart\", Item { sku: \"w\" }) by Nobody\n    expect r is Rejected(Unauthorized)",
        )
        .assert_reports("bynk.test.credential_needs_system");
    }

    #[test]
    fn a_by_nobody_nested_in_a_block_needs_system_too() {
        // #1849: only the top-level statements used to be searched.
        nobody_case(
            "    let ok = if true {\n      let r <- api.POST(\"/cart\", Item { sku: \"w\" }) by Nobody\n      r is Rejected(Unauthorized)\n    } else { false }\n    expect ok",
        )
        .assert_reports("bynk.test.credential_needs_system");
    }
}

/// #1934: a case's service call is typed by what its handler yields
/// (re-opening ADR 0203), and a `system`-tier call that reaches past the
/// handler's types by what the wire yields.
#[cfg(test)]
mod typed_outcome_tests {
    const API: &str = "context app.web

type Item = { sku: String }

service api from http {
  on GET(\"/x\") () -> Effect[HttpResult[()]] by Visitor {
    Raw(Bytes.fromUtf8(\"hi\"), \"text/plain\")
  }
  on GET(\"/name\") () -> Effect[HttpResult[String]] by Visitor {
    Ok(\"ada\")
  }
  on GET(\"/item\") () -> Effect[HttpResult[Item]] by Visitor {
    Ok(Item { sku: \"w\" })
  }
}

service calc {
  on call(n: Int) -> Effect[Int] {
    n
  }
}

service sweeper from cron {
  on schedule(\"0 0 * * *\") () -> Effect[Result[(), String]] {
    Ok(())
  }
}

service intake from queue(\"orders\") {
  on message(item: Item) -> Effect[QueueResult] {
    Ack
  }
}
";

    fn case(body: &str) -> crate::testkit::Analysed {
        crate::testkit::analyse(&[
            ("app/web.bynk", API),
            (
                "app/tests/web.test.bynk",
                &format!("suite app.web {{\n  case \"c\" {{\n{body}\n  }}\n}}\n"),
            ),
        ])
    }

    /// The case of #1931 and #1934: a method on a value of the handler's
    /// result type checks, and a wrong one is reported against that type.
    #[test]
    fn an_http_outcome_has_the_handlers_result_type() {
        let get = "    let r <- api.GET(\"/name\") by Visitor\n";
        case(&format!("{get}    match r {{\n      Ok(v) => expect v.length() == 3\n      _ => expect false\n    }}"))
            .assert_clean();
        case(&format!("{get}    match r {{\n      Ok(v) => expect v.nosuch() == 3\n      _ => expect false\n    }}"))
            .assert_reports("bynk.types.method_not_found");
        case(&format!(
            "{get}    expect r is Ok(v) && v.startsWith(\"a\")"
        ))
        .assert_clean();
        case(&format!("{get}    expect r == Ok(\"ada\")")).assert_clean();
        let item = "    let r <- api.GET(\"/item\") by Visitor\n";
        case(&format!(
            "{item}    expect r is Ok(i) && i.sku.length() == 1"
        ))
        .assert_clean();
        case(&format!("{item}    expect r is Ok(i) && i.nosuch == 1"))
            .assert_reports("bynk.types.unknown_field");
    }

    /// A payload of fixed type binds it, as before; the arms are checked by
    /// the ordinary rules.
    #[test]
    fn fixed_payloads_and_arms_are_checked() {
        let get = "    let r <- api.GET(\"/x\") by Visitor\n";
        case(&format!("{get}    match r {{\n      Raw(body, ct) => expect body.length() == 2 && ct.startsWith(\"text/\")\n      _ => expect false\n    }}"))
            .assert_clean();
        case(&format!("{get}    match r {{\n      Raw(_, _) => expect nope == 1\n      _ => expect false\n    }}"))
            .assert_reports("bynk.resolve.unknown_name");
        case(&format!("{get}    match r {{\n      BadRequest(m) | NotFound => expect m.length() == 9\n      _ => expect false\n    }}"))
            .assert_reports("bynk.types.or_pattern_binding_mismatch");
        case(&format!("{get}    expect r is Bogus(_)"))
            .assert_reports("bynk.types.is_unknown_variant");
    }

    /// An `on call` handler's return type is what the case binds.
    #[test]
    fn a_call_outcome_has_the_handlers_return_type() {
        case("    let n <- calc.call(2)\n    expect n.abs() == 2").assert_clean();
        case("    let n <- calc.call(2)\n    expect n.length() == 2")
            .assert_reports("bynk.types.method_not_found");
    }

    /// cron and queue handlers are typed by their return types too, so a
    /// queue verdict is matchable.
    #[test]
    fn cron_and_queue_outcomes_are_typed() {
        case("    let r <- sweeper.schedule(\"0 0 * * *\")\n    match r {\n      Ok(_) => expect true\n      Err(e) => expect e.length() > 0\n    }")
            .assert_clean();
        case("    let r <- intake.message(Item { sku: \"x\" })\n    expect r is Ack")
            .assert_clean();
        case("    let r <- intake.message(Item { sku: \"x\" })\n    match r {\n      Retry(why) => expect why.length() > 0\n      Ack => expect true\n    }")
            .assert_clean();
        case("    let r <- intake.message(Item { sku: \"x\" })\n    expect r is Ok(_)")
            .assert_reports("bynk.types.is_unknown_variant");
    }

    /// The context of `385_system_wire_rejection`, cut down.
    const CART_API: &str = "context shop.api

type UserId = String where NonEmpty
actor User { auth = Bearer(secret = \"AUTH_SECRET\"), identity = UserId }

type Item = { sku: String }

service api from http {
  on POST(\"/cart\") (body: Item) -> Effect[HttpResult[Item]] by u: User {
    Created(body)
  }
}
";

    fn system_case(body: &str) -> crate::testkit::Analysed {
        crate::testkit::analyse(&[
            ("shop/api.bynk", CART_API),
            (
                "shop/tests/api.test.bynk",
                &format!("suite shop.api as system {{\n  case \"c\" {{\n{body}\n  }}\n}}\n"),
            ),
        ])
    }

    /// A typed `system`-tier call decodes the handler's `HttpResult`.
    #[test]
    fn a_typed_system_call_is_the_handlers_result() {
        system_case("    let r <- api.POST(\"/cart\", Item { sku: \"w\" }) by User(\"a\")\n    expect r is Created(i) && i.sku == \"w\"")
            .assert_clean();
        system_case("    let r <- api.POST(\"/cart\", Item { sku: \"w\" }) by User(\"a\")\n    expect r is Rejected(_)")
            .assert_reports("bynk.types.is_unknown_variant");
    }

    /// `Wire`, `by Nobody` and a wrong method observe the boundary, so they
    /// yield an `HttpOutcome`: its kinds are checked, and `Handled` carries
    /// the handler's `HttpResult`.
    #[test]
    fn a_call_past_the_handlers_types_is_an_http_outcome() {
        let wire = "    let r <- api.POST(\"/cart\", Wire(\"{}\")) by User(\"a\")\n";
        system_case(&format!("{wire}    match r {{\n      Rejected(RefinementViolation(d)) => expect d.length() > 0\n      Rejected(_) => expect false\n      Handled(Created(i)) => expect i.sku == \"w\"\n      Handled(_) => expect false\n    }}"))
            .assert_clean();
        system_case(&format!("{wire}    match r {{\n      Rejected(Unauthorized(d)) => expect true\n      _ => expect false\n    }}"))
            .assert_reports("bynk.types.pattern_arity");
        system_case(&format!("{wire}    expect r is Rejected(Refinement(_))"))
            .assert_reports("bynk.types.unknown_variant_in_pattern");
        system_case(&format!("{wire}    expect r is Created(_)"))
            .assert_reports("bynk.types.is_unknown_variant");
        system_case("    let r <- api.POST(\"/cart\", Item { sku: \"w\" }) by Nobody\n    expect r is Rejected(Unauthorized)")
            .assert_clean();
        // Both at once (the raw no-auth driver) is still an `HttpOutcome`.
        system_case("    let r <- api.POST(\"/cart\", Wire(\"{}\")) by Nobody\n    expect r is Rejected(Unauthorized)")
            .assert_clean();
        system_case(
            "    let r <- api.POST(\"/cart\", Wire(\"{}\")) by Nobody\n    expect r is Created(_)",
        )
        .assert_reports("bynk.types.is_unknown_variant");
        system_case(
            "    let r <- api.DELETE(\"/cart\")\n    expect r is Rejected(MethodNotAllowed)",
        )
        .assert_clean();
        system_case("    let r <- api.DELETE(\"/cart\")\n    expect r is NotFound")
            .assert_reports("bynk.types.is_unknown_variant");
    }
}

#[cfg(test)]
mod participant_tests {
    use super::*;

    /// `(publisher, event)` → its subscribers, as
    /// `symbols::discover_event_subscribers` builds it.
    type Subscribers = BTreeMap<(String, String), Vec<(String, String)>>;

    /// `ops.hub` consumes `ops.store` and the `bynk` adapter, and publishes
    /// `Ping`. `ops.audit` subscribes to it and consumes `ops.archive`, a
    /// context outside `ops.hub`'s closure. `ops.store` publishes `Saved`, to
    /// which `ops.mirror` subscribes, and `ops.audit` publishes `Noted`, to
    /// which `ops.digest` subscribes.
    fn project() -> (
        HashMap<String, Vec<String>>,
        BTreeMap<String, UnitKind>,
        Subscribers,
    ) {
        let edges = [
            ("ops.hub", &["ops.store", "bynk"][..]),
            ("ops.audit", &["ops.hub", "ops.archive"][..]),
            ("ops.mirror", &["ops.store"][..]),
            ("ops.digest", &["ops.audit"][..]),
        ];
        let unit_consumes = edges
            .iter()
            .map(|(u, ds)| (u.to_string(), ds.iter().map(|d| d.to_string()).collect()))
            .collect();
        let mut kinds: BTreeMap<String, UnitKind> = [
            "ops.hub",
            "ops.store",
            "ops.audit",
            "ops.archive",
            "ops.mirror",
            "ops.digest",
        ]
        .iter()
        .map(|c| (c.to_string(), UnitKind::Context))
        .collect();
        kinds.insert("bynk".to_string(), UnitKind::Adapter);
        let sub = |owner: &str, event: &str, ctx: &str| {
            (
                (owner.to_string(), event.to_string()),
                vec![(ctx.to_string(), "On".to_string())],
            )
        };
        let subscribers = [
            sub("ops.hub", "Ping", "ops.audit"),
            sub("ops.store", "Saved", "ops.mirror"),
            sub("ops.audit", "Noted", "ops.digest"),
        ]
        .into_iter()
        .collect();
        (unit_consumes, kinds, subscribers)
    }

    /// A case reaches the target's `consumes` closure: contexts only, the
    /// target first.
    #[test]
    fn participants_are_the_consumes_closure() {
        let (consumes, kinds, _) = project();
        assert_eq!(
            infer_participants("ops.hub", &consumes, &kinds),
            ["ops.hub", "ops.store"]
        );
    }

    /// #1959: the harness also stands up the target's subscriber and its own
    /// closure (`ops.archive`), a non-target participant's subscriber
    /// (`ops.mirror`), and a subscriber's subscriber (`ops.digest`).
    #[test]
    fn the_harness_adds_subscribers_and_their_closures_transitively() {
        let (consumes, kinds, subscribers) = project();
        assert_eq!(
            infer_harness_participants("ops.hub", &consumes, &kinds, &subscribers),
            [
                "ops.hub",
                "ops.store",
                "ops.audit",
                "ops.mirror",
                "ops.archive",
                "ops.digest"
            ]
        );
    }

    /// With no subscribers the two sets agree.
    #[test]
    fn without_subscribers_the_harness_is_the_case_set() {
        let (consumes, kinds, _) = project();
        assert_eq!(
            infer_harness_participants("ops.hub", &consumes, &kinds, &BTreeMap::new()),
            infer_participants("ops.hub", &consumes, &kinds)
        );
    }
}

/// #2018: the `is`-pattern comparison the history restate check uses. It is
/// conservative: only literals and payload-free variants (same name, same
/// optional qualifier) are equal; anything else compares unequal.
#[cfg(test)]
mod history_pattern_tests {
    use super::*;

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::default(),
        }
    }

    fn variant(type_name: Option<&str>, name: &str) -> Pattern {
        Pattern::Variant {
            type_name: type_name.map(ident),
            variant: ident(name),
            bindings: Vec::new(),
            span: Span::default(),
        }
    }

    fn payload_variant(name: &str) -> Pattern {
        Pattern::Variant {
            type_name: None,
            variant: ident(name),
            bindings: vec![PatternBinding {
                kind: PatternBindingKind::Positional {
                    pattern: Pattern::Wildcard(Span::default()),
                },
                span: Span::default(),
            }],
            span: Span::default(),
        }
    }

    fn int(n: i64) -> Pattern {
        Pattern::Literal {
            value: LiteralValue::Int(n),
            span: Span::default(),
        }
    }

    #[test]
    fn same_payload_free_variant_matches() {
        assert!(history_pattern_matches(
            &variant(None, "Closed"),
            &variant(None, "Closed")
        ));
        assert!(history_pattern_matches(
            &variant(Some("Status"), "Closed"),
            &variant(Some("Status"), "Closed")
        ));
    }

    #[test]
    fn different_variant_names_do_not_match() {
        assert!(!history_pattern_matches(
            &variant(None, "Open"),
            &variant(None, "Closed")
        ));
    }

    #[test]
    fn qualifier_present_vs_absent_does_not_match() {
        assert!(!history_pattern_matches(
            &variant(Some("Status"), "Closed"),
            &variant(None, "Closed")
        ));
        assert!(!history_pattern_matches(
            &variant(None, "Closed"),
            &variant(Some("Status"), "Closed")
        ));
        assert!(!history_pattern_matches(
            &variant(Some("Status"), "Closed"),
            &variant(Some("Other"), "Closed")
        ));
    }

    #[test]
    fn literals_match_by_value() {
        assert!(history_pattern_matches(&int(3), &int(3)));
        assert!(!history_pattern_matches(&int(3), &int(4)));
        assert!(!history_pattern_matches(&int(3), &variant(None, "Closed")));
    }

    #[test]
    fn payload_variant_never_matches() {
        assert!(!history_pattern_matches(
            &payload_variant("Some"),
            &payload_variant("Some")
        ));
        assert!(!history_pattern_matches(
            &payload_variant("Some"),
            &variant(None, "Some")
        ));
    }
}

/// #2052: the `svc.event(e)` address's checks that the fixtures don't pin
/// (a note's text) or that need a project shape of their own.
#[cfg(test)]
mod event_address_tests {
    use crate::testkit::analyse;

    const ORDER: &str =
        "context shop.order\n\nexports transparent { Placed }\n\nevent Placed = { id: String }\n";

    fn ledger(consumes: &str, extra: &str) -> String {
        format!(
            "context shop.ledger\n\n{consumes}\n{extra}\nservice OnPlaced from Events(Placed) {{\n  on event(e: Placed) -> Effect[()] {{\n    ()\n  }}\n}}\n"
        )
    }

    fn suite(body: &str) -> String {
        format!("suite shop.ledger {{\n  case \"c\" {{\n{body}\n  }}\n}}\n")
    }

    #[test]
    fn an_envelope_the_suite_cannot_name_notes_consumes_bynk() {
        let a = analyse(&[
            ("shop/order.bynk", ORDER),
            ("shop/ledger.bynk", &ledger("consumes shop.order", "")),
            (
                "shop/tests/ledger.test.bynk",
                &suite("    let _ <- OnPlaced.event(Placed { id: \"o1\" }, \"evt-1\")"),
            ),
        ]);
        let e = a
            .analysis
            .errors
            .iter()
            .find(|e| e.error.category == "bynk.types.argument_mismatch")
            .expect("an argument mismatch on the envelope");
        assert!(
            e.error
                .notes
                .iter()
                .any(|n| n.contains("must `consumes bynk`")),
            "expected the `consumes bynk` note, got {:?}",
            e.error.notes
        );
    }

    #[test]
    fn a_same_named_event_outside_the_targets_scope_is_a_plain_mismatch() {
        // `shop.archive` declares an event `Note`, but the target doesn't
        // consume it; the target's own `Note` is a record, not an event.
        let a = analyse(&[
            ("shop/order.bynk", ORDER),
            (
                "shop/archive.bynk",
                "context shop.archive\n\nevent Note = { text: String }\n",
            ),
            (
                "shop/ledger.bynk",
                &ledger("consumes shop.order", "\ntype Note = { text: String }\n"),
            ),
            (
                "shop/tests/ledger.test.bynk",
                &suite("    let _ <- OnPlaced.event(Note { text: \"n\" })"),
            ),
        ]);
        a.assert_reports("bynk.types.argument_mismatch");
        assert!(
            !a.categories().contains(&"bynk.test.event_not_subscribed"),
            "a record named like an unconsumed context's event is not an event: {:?}",
            a.categories()
        );
    }

    #[test]
    fn a_same_named_event_in_an_unconsumed_context_leaves_the_publisher_unique() {
        let a = analyse(&[
            ("shop/order.bynk", ORDER),
            (
                "shop/archive.bynk",
                "context shop.archive\n\nexports transparent { Placed }\n\nevent Placed = { id: String }\n",
            ),
            ("shop/ledger.bynk", &ledger("consumes shop.order", "")),
            (
                "shop/tests/ledger.test.bynk",
                &suite("    let _ <- OnPlaced.event(Placed { id: \"o1\" })"),
            ),
        ]);
        a.assert_clean();
    }

    #[test]
    fn two_consumed_publishers_refuse_a_default_envelope() {
        let a = analyse(&[
            ("shop/order.bynk", ORDER),
            (
                "shop/archive.bynk",
                "context shop.archive\n\nexports transparent { Placed }\n\nevent Placed = { id: String }\n",
            ),
            (
                "shop/ledger.bynk",
                &ledger("consumes shop.order\nconsumes shop.archive", ""),
            ),
            (
                "shop/tests/ledger.test.bynk",
                &suite("    let _ <- OnPlaced.event(Placed { id: \"o1\" })"),
            ),
        ]);
        a.assert_reports("bynk.test.event_publisher_ambiguous");
    }
}
