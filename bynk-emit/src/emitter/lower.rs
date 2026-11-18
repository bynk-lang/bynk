//! Expression and statement lowering — the `LowerCtx`-driven engine that
//! turns Bynk expressions, statements, matches, and mocks into TypeScript
//! source. Split out of `emitter.rs` (ADR 0060); `LowerCtx` and the `ts_*`
//! type renderers stay in the parent and are reached via `use super::*`.

use std::collections::HashSet;
use std::sync::Arc;

use bynk_check::checker::{Callee, NamedKind, Ty, TyId, TypedCommons};
use bynk_check::kernel_op::{
    BytesOp, DecimalOp, DurationOp, EffectOp, EffectResultOp, FloatOp, InstantOp, IntOp, KernelOp,
    MapOp, OptionOp, QueryOp, ResultOp, SetOp, StreamOp, StringOp, TestConnectionOp,
};
use bynk_syntax::ast::{
    BaseType, BinOp, Block, Expr, ExprKind, FaultsExpr, FieldInit, Ident, InterpPart, LambdaExpr,
    LiteralValue, MatchArm, ObservationExpr, ObservationMatcher, Pattern, PatternBindingKind,
    PredKind, Refinement, TypeBody, TypeDecl, TypeRef, UnaryOp,
};

use bynk_model::is_effectful_return;
use bynk_model::{ConstVal, EventPatternIr, EventPatternValueIr, IrHttpMethod};

use super::ctx::{BodyMode, ModuleCtx, TestShared};
use super::lower_ctx::LowerCtx;
use super::lower_list::{
    join_other_elem_ts, kernel_key_comparator, key_fn_is_plain, key_is_plain, list_ok_elem_ts,
    lower_list_kernel, with_comparator,
};
use super::lower_stmt::{
    Body, Item, arms_contain_question, block_body, block_body_into, block_contains_question,
    emit_block_as_function_body_with_return, emit_statement, hoist_if_as_statement,
    hoist_value_as_statement, if_tail, match_dispatch, match_tail, render_text, ternary_shaped,
    write_line,
};
use super::ts_types::{
    ts_array_elem, ts_base, ts_ty, ts_ty_to_ts_type, ts_type_ref, ts_type_ref_to_ts_type,
    unchecked_construct, unchecked_construct_test, unchecked_construct_test_node,
};
use super::*;

/// Lower a block to a sequence of TypeScript statements suitable for use as
/// an async function body. Used by v0.7 mock-operation emission — today
/// exclusively test/property/contract scaffolding (`stub` rhs values, `where`
/// predicates, contract `requires` guards), never real production provider
/// bodies, so `test_scaffold` is set unconditionally (nothing here emits an
/// `assert`, so `assert_loc` stays `None`) purely to mark test-scaffold mode
/// for `in_test_scaffold()` — otherwise a refined-literal admitted here (e.g.
/// a `stub Cap.op() returns "lit"` rhs) brands via the production `(v as T)`
/// cast, which cannot resolve `T` in a stub class's `any`-typed destructure
/// (Locale capability track, slice 1, #844).
pub fn lower_block_to_async_body(
    block: &Block,
    return_type: &TypeRef,
    typed: &mut TypedCommons,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    runtime_use: &RuntimeUse,
) -> (String, SourceMapBuilder) {
    let mut out = String::new();
    // v0.70: a sub-builder records body checkpoints relative to this local buffer;
    // the caller merges it into the module map at the splice offset.
    let smb = RefCell::new(SourceMapBuilder::new());
    {
        let mut cx = LowerCtx::new(
            ModuleCtx::new(typed, cross_context, runtime_use, UnitFacts::detached()),
            BodyMode::PredicateScaffold {
                test: TestShared {
                    test_scaffold: true,
                    assert_loc: None,
                },
            },
        )
        .with_source_map(Some(&smb));
        let async_tail = is_effectful_return(return_type);
        emit_block_as_function_body_with_return(
            &mut out,
            block,
            &mut cx,
            0,
            async_tail,
            Some(return_type),
        );
    }
    (out, smb.into_inner())
}

/// Lower a test-case body: statements followed by a discarded tail expression
/// (the runner records success via the assertion mechanism, not via a return
/// value). Used by v0.7 test emission.
#[allow(clippy::too_many_arguments)]
pub fn lower_test_case_body(
    block: &Block,
    typed: &mut TypedCommons,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    test_services: HashSet<String>,
    test_service_handlers: HashMap<String, Vec<bynk_model::IrHandlerKind>>,
    sum_handlers: HashSet<(String, String)>,
    test_agents: HashSet<String>,
    settle_sends: bool,
    harness_bound: HashSet<String>,
    source: &str,
    rel_path: &str,
    runtime_use: &RuntimeUse,
) -> (String, SourceMapBuilder) {
    lower_test_case_body_reporting(
        block,
        typed,
        cross_context,
        test_services,
        test_service_handlers,
        sum_handlers,
        test_agents,
        settle_sends,
        harness_bound,
        source,
        rel_path,
        runtime_use,
        false,
    )
}

/// [`lower_test_case_body`], optionally reporting the tail's value (#3585,
/// `bynk eval`). With `report_value`, the tail expression is not discarded: its
/// value is encoded as the wire shape `Json.encode` gives it and written to
/// stdout as one `{"type":"eval-value","valueType":"…","value":…}` line, which
/// the test runner's NDJSON reader picks up. Nothing else about the body changes.
#[allow(clippy::too_many_arguments)]
pub fn lower_test_case_body_reporting(
    block: &Block,
    typed: &mut TypedCommons,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    test_services: HashSet<String>,
    test_service_handlers: HashMap<String, Vec<bynk_model::IrHandlerKind>>,
    sum_handlers: HashSet<(String, String)>,
    test_agents: HashSet<String>,
    settle_sends: bool,
    // #2024: the names the runner scaffold binds in the body's scope.
    harness_bound: HashSet<String>,
    source: &str,
    rel_path: &str,
    runtime_use: &RuntimeUse,
    report_value: bool,
) -> (String, SourceMapBuilder) {
    let mut out = String::new();
    let smb = RefCell::new(SourceMapBuilder::new());
    {
        let mut cx = LowerCtx::new(
            ModuleCtx::new(typed, cross_context, runtime_use, UnitFacts::detached()),
            BodyMode::TestCase {
                test: TestShared {
                    test_scaffold: true,
                    assert_loc: Some(crate::emitter::ctx::AssertLoc {
                        source: source.to_string(),
                        rel_path: rel_path.to_string(),
                    }),
                },
                // v0.117: observations and `trace` in the body read the per-case
                // recorded trace object declared by the runner scaffold.
                observation_trace: Some("__obs".to_string()),
                test_services,
                test_service_handlers,
                sum_handlers,
                settle_sends,
            },
        )
        .with_source_map(Some(&smb));
        cx.local_agents = test_agents;
        cx.harness_bound = harness_bound;
        for stmt in &block.statements {
            emit_statement(&mut out, stmt, &mut cx, 0);
        }
        // Evaluate the tail expression but discard its value; assertions inside
        // it still take effect via thrown AssertionErrors.
        cx.record_span(out.len(), block.tail.span);
        let mut pre = Pre::new();
        if report_value && let Some(line) = eval_value_line(&mut pre, &block.tail, &mut cx) {
            for s in &pre.stmts() {
                write_line(&mut out, 0, s);
            }
            write_line(&mut out, 0, &line);
            return (out, smb.into_inner());
        }
        let tail = pre.lower(&block.tail, &mut cx);
        for s in &pre.stmts() {
            write_line(&mut out, 0, s);
        }
        if !tail.is_empty() && tail != "undefined" {
            write_line(&mut out, 0, &format!("void ({tail});"));
        }
    }
    (out, smb.into_inner())
}

/// v0.16: lower an integration test case body. Like [`lower_test_case_body`],
/// but in **workers** mode and from a synthetic harness root: entry calls
/// (`ctx.service(args)`) are cross-context calls that lower to `callService(
/// deps.env.<BINDING>, …)` over the real wire. The harness root declares no
/// local services/agents, so those scoped sets stay empty; `cross_context`
/// carries every participant's service surface.
#[allow(clippy::too_many_arguments)]
pub fn lower_integration_case_body(
    block: &Block,
    typed: &mut TypedCommons,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    system_http_services: std::collections::HashSet<String>,
    system_http_routes: std::collections::HashSet<(String, String, String)>,
    system_http_route_body: HashMap<(String, String, String), (usize, bynk_syntax::ast::TypeRef)>,
    system_http_type_ns: String,
    system_http_claims: crate::emitter::ctx::SystemHttpClaims,
    // #2136: the target's cron and queue services, driven through the Worker.
    system_jobs: crate::emitter::ctx::SystemJobServices,
    // #2024: the names the case scaffold binds in the body's scope.
    harness_bound: HashSet<String>,
    source: &str,
    rel_path: &str,
    runtime_use: &RuntimeUse,
    // #2138 (track §3.6): a remote build wraps each replayable segment in a
    // `__bynkSettle` retry ([`settle_segments`]).
    settle: bool,
) -> (String, SourceMapBuilder) {
    let mut out = String::new();
    let smb = RefCell::new(SourceMapBuilder::new());
    let kinds: Vec<SettleKind> = block
        .statements
        .iter()
        .map(|s| settle_kind(s, &typed.callees))
        .collect();
    let mut bounds: Vec<(usize, usize)> = Vec::new();
    // Each statement's top-level bindings, as emitted (a re-`let` or a name
    // the harness binds is renamed), for a settle attempt to return.
    let mut binds: Vec<Vec<String>> = Vec::new();
    {
        let mut module = ModuleCtx::new(typed, cross_context, runtime_use, UnitFacts::detached());
        module.target = BuildTarget::Workers;
        let mut cx = LowerCtx::new(
            module,
            BodyMode::IntegrationCase {
                test: TestShared {
                    test_scaffold: true,
                    assert_loc: Some(crate::emitter::ctx::AssertLoc {
                        source: source.to_string(),
                        rel_path: rel_path.to_string(),
                    }),
                },
                system_http_services,
                system_http_routes,
                system_http_route_body,
                system_http_type_ns,
                system_http_claims: Box::new(system_http_claims),
                system_jobs,
            },
        )
        .with_source_map(Some(&smb));
        cx.harness_bound = harness_bound;
        for stmt in &block.statements {
            let start = out.len();
            emit_statement(&mut out, stmt, &mut cx, 0);
            bounds.push((start, out.len()));
            binds.push(match stmt {
                // #2640: a `par` block binds each of its `let` names.
                bynk_syntax::ast::Statement::Par(p) => p
                    .statements
                    .iter()
                    .filter_map(|s| match s {
                        bynk_syntax::ast::Statement::EffectLet(l) if l.name.name != "_" => Some(
                            cx.resolved_local_name(&l.name.name)
                                .unwrap_or_else(|| crate::emitter::ts_ident(&l.name.name)),
                        ),
                        _ => None,
                    })
                    .collect(),
                bynk_syntax::ast::Statement::Let(l) | bynk_syntax::ast::Statement::EffectLet(l)
                    if l.name.name != "_" =>
                {
                    vec![
                        cx.resolved_local_name(&l.name.name)
                            .unwrap_or_else(|| crate::emitter::ts_ident(&l.name.name)),
                    ]
                }
                _ => Vec::new(),
            });
        }
        cx.record_span(out.len(), block.tail.span);
        let mut pre = Pre::new();
        let tail = pre.lower(&block.tail, &mut cx);
        for s in &pre.stmts() {
            write_line(&mut out, 0, s);
        }
        if !tail.is_empty() && tail != "undefined" {
            write_line(&mut out, 0, &format!("void ({tail});"));
        }
    }
    if settle {
        // The rewrite moves the text, so the source map no longer fits it; a
        // remote build is not one an editor steps through.
        return (
            settle_segments(&out, &bounds, &kinds, &binds),
            SourceMapBuilder::new(),
        );
    }
    (out, smb.into_inner())
}

/// #2138 (track §3.6): how a top-level case statement takes part in the
/// `--settle` retry rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettleKind {
    /// An address whose method is safe per RFC 9110 (`GET`, `HEAD`,
    /// `OPTIONS`): replayed before a retried `expect`.
    Safe,
    /// Any other address (`POST`, `schedule`, `message`, …): never replayed,
    /// so it ends the segment a retry may re-run.
    Unsafe,
    Expect,
    Other,
}

/// Fails closed: only a pure `let` is `Other` and only an `expect` is
/// `Expect`, and either is `Unsafe` if it holds an unsafe address. An
/// effectful statement (`<-`, `do`, `:=`, `advance`) is `Safe` only
/// when its value *is* a safe address and holds no other; anything else ends
/// the segment, so a retry can never re-run a write. A missed retry is the
/// price; a repeated `POST` against a real deployment would not be.
fn settle_kind(
    stmt: &bynk_syntax::ast::Statement,
    callees: &HashMap<bynk_syntax::ast::ExprId, bynk_check::checker::Callee>,
) -> SettleKind {
    use bynk_check::checker::Callee;
    use bynk_syntax::ast::Statement;
    // `Some(true)` for a safe address, `Some(false)` for an unsafe one or any
    // other call that reaches beyond the case (an `expect … faults` may hold
    // one), `None` for anything else. A free function called in a pure
    // position is pure; called with `<-` it isn't a safe address, so it ends
    // the segment below.
    let safety = |e: &bynk_syntax::ast::Expr| match callees.get(&e.id) {
        Some(Callee::TestService { address, .. }) => Some(matches!(
            address.split(' ').next().unwrap_or_default(),
            "GET" | "HEAD" | "OPTIONS"
        )),
        Some(Callee::TestAssetFetch { .. }) => Some(true),
        Some(
            Callee::Agent { .. }
            | Callee::AgentInit(_)
            | Callee::Cross { .. }
            | Callee::CrossCap { .. },
        ) => Some(false),
        _ => None,
    };
    // Whether any expression in `stmt` other than `skip` is unsafe.
    let holds_unsafe = |skip: Option<bynk_syntax::ast::ExprId>| {
        let mut exprs = Vec::new();
        bynk_syntax::ast::statement_exprs(stmt, &mut exprs);
        let mut found = false;
        for e in exprs {
            bynk_model::walk_exprs(e, &mut |x: &bynk_syntax::ast::Expr| {
                if Some(x.id) != skip && safety(x) == Some(false) {
                    found = true;
                }
            });
        }
        found
    };
    match stmt {
        Statement::Let(_) | Statement::Expect(_) if holds_unsafe(None) => SettleKind::Unsafe,
        Statement::Let(_) => SettleKind::Other,
        Statement::Expect(_) => SettleKind::Expect,
        Statement::EffectLet(bynk_syntax::ast::LetStmt { value, .. })
        | Statement::Do(bynk_syntax::ast::DoStmt { value, .. })
            if safety(value) == Some(true) && !holds_unsafe(Some(value.id)) =>
        {
            SettleKind::Safe
        }
        _ => SettleKind::Unsafe,
    }
}

/// #2138 (track §3.6): rewrite a lowered case body for `--settle`. The body's
/// top-level statements split into runs at each unsafe address. In a run
/// holding an `expect` and a safe address before it, the statements from the
/// run's start through its last `expect` become one `__bynkSettle` attempt:
/// re-run until every `expect` in it holds or the deadline passes, so the
/// addresses since the last unsafe one are replayed before each retry. The
/// bindings it declares are returned out of the attempt, so the statements
/// after it see them unchanged. `bounds` are each statement's byte range in
/// `out`; text after the last one (the tail) is kept as is.
fn settle_segments(
    out: &str,
    bounds: &[(usize, usize)],
    kinds: &[SettleKind],
    binds: &[Vec<String>],
) -> String {
    let piece = |i: usize| &out[bounds[i].0..bounds[i].1];
    let mut text = String::new();
    let mut i = 0;
    while i < kinds.len() {
        if kinds[i] == SettleKind::Unsafe {
            text.push_str(piece(i));
            i += 1;
            continue;
        }
        let end = (i..kinds.len())
            .find(|&k| kinds[k] == SettleKind::Unsafe)
            .unwrap_or(kinds.len());
        let last_expect = (i..end).rev().find(|&k| kinds[k] == SettleKind::Expect);
        match last_expect {
            Some(e) if (i..e).any(|k| kinds[k] == SettleKind::Safe) => {
                let segment: String = (i..=e).map(piece).collect();
                let names: Vec<&str> = binds[i..=e].iter().flatten().map(String::as_str).collect();
                text.push_str(&settle_attempt(&segment, &names));
                for k in e + 1..end {
                    text.push_str(piece(k));
                }
            }
            _ => {
                for k in i..end {
                    text.push_str(piece(k));
                }
            }
        }
        i = end;
    }
    if let Some(&(_, last)) = bounds.last() {
        text.push_str(&out[last..]);
    } else {
        text.push_str(out);
    }
    text
}

/// One `__bynkSettle` attempt around `segment`, returning `names` (the
/// segment's top-level bindings, from the AST, as emitted) so the code after
/// it sees them. The segment's text is moved in unchanged — not re-indented,
/// which would alter a multi-line template literal.
fn settle_attempt(segment: &str, names: &[&str]) -> String {
    if names.is_empty() {
        format!("await __bynkSettle(async () => {{\n{segment}}});\n")
    } else {
        let list = names.join(", ");
        format!(
            "const {{ {list} }} = await __bynkSettle(async () => {{\n{segment}return {{ {list} }};\n}});\n"
        )
    }
}

/// v0.154 (ADR 0178): the `Err`-wrap a `?` on a `Result` operand needs, if the
/// operand's error type differs from the enclosing return's and a declared
/// embedding converts it. Returns `(sum_type, variant)` to emit
/// `Err(sum_type.variant(err))`, or `None` to propagate the `Err` unchanged.
/// Uses the checker's `embedding_for`, so it can never diverge from what the
/// checker accepted.
fn embed_conversion(
    operand_ty: Option<bynk_check::checker::TyId>,
    cx: &LowerCtx,
) -> Option<(String, String)> {
    let tys = cx.ty_render();
    use bynk_check::checker::Ty;
    let operand_node = operand_ty.map(|t| tys.get(t));
    let Some(Ty::Result(_, e)) = operand_node.as_deref() else {
        return None;
    };
    let f_err = peel_result_err(cx.return_ty?, tys.types())?;
    // An exact/compatible match propagates as-is; only a genuine mismatch that a
    // declared embedding resolves gets wrapped.
    if bynk_check::checker::compatible(*e, f_err, tys.types()) {
        return None;
    }
    bynk_check::checker::embedding_for(f_err, *e, &cx.commons().types, tys.types())
}

/// Peel `Result[_, E]` / `Effect[Result[_, E]]` to the error type `E`.
fn peel_result_err(
    ty: bynk_check::checker::TyId,
    tys: &Arc<Types>,
) -> Option<bynk_check::checker::TyId> {
    use bynk_check::checker::Ty;
    match &*tys.get(ty) {
        Ty::Result(_, e) => Some(*e),
        Ty::Effect(inner) => peel_result_err(*inner, tys),
        _ => None,
    }
}

/// Slice 1A.8: the value-position `if`'s hoist, with the node form. Statement
/// nodes only when the condition, both tails and every statement the branches
/// hoisted are nodes; otherwise today's text form ([`hoist_if_as_statement`],
/// still the entry for the short-circuit operators and the async tail). A text
/// statement is never wrapped to fit inside a node.
fn hoist_if_value(
    pre: &mut Pre,
    cond: LoweredExpr,
    then_tail: Lowered,
    else_tail: Lowered,
    e: &Expr,
    cx: &mut LowerCtx,
) -> String {
    let all_nodes = matches!(cond, LoweredExpr::Node(_))
        && [&then_tail, &else_tail].iter().all(|t| {
            matches!(t.expr, LoweredExpr::Node(_))
                && t.pre.iter().all(|s| matches!(s, LoweredStmt::Node(_)))
        });
    if all_nodes {
        let slot = cx.fresh();
        let ty = cx
            .commons()
            .expr_ty(e.id)
            .map(|_| ts_ty_to_ts_type(cx.commons().expr_types[&e.id].ty, cx.ty_render()));
        hoisted_if_nodes(pre, &slot, ty, cond, then_tail, else_tail);
        slot
    } else {
        let slot_ty = checked_ty_ts(e, cx);
        hoist_if_as_statement(pre, cond.into_text(), then_tail, else_tail, slot_ty, cx)
    }
}

/// [`hoist_if_value`]'s node form. The slot's type is taken from the
/// checker's record of the `if` expression (see the TS7034/TS7005 note on the
/// text form), as a type node. The branches are `Block`s, so the printer nests
/// them one level per `if`, as `assign_branch`'s padding did.
fn hoisted_if_nodes(
    pre: &mut Pre,
    slot: &str,
    ty: Option<bynk_ts::TsType>,
    cond: LoweredExpr,
    then_tail: Lowered,
    else_tail: Lowered,
) {
    pre.push_node(bynk_ts::TsStmt::let_stmt(
        bynk_ts::TsBindingName::Ident(slot.to_string()),
        ty,
        None,
        None,
    ));
    let LoweredExpr::Node(cond) = cond else {
        unreachable!("hoisted_if_nodes is given a node condition")
    };
    let branch = |tail: Lowered| {
        let mut stmts: Vec<bynk_ts::TsStmt> = tail
            .pre
            .into_iter()
            .filter_map(|s| match s {
                LoweredStmt::Node(n) => Some(*n),
                LoweredStmt::Text(_) => None,
            })
            .collect();
        if let LoweredExpr::Node(value) = tail.expr {
            stmts.push(bynk_ts::TsStmt::assign(ident_node(slot), value, None));
        }
        bynk_ts::TsStmt::block(stmts, None)
    };
    let then_branch = branch(then_tail);
    let else_branch = branch(else_tail);
    pre.push_node(bynk_ts::TsStmt::if_else_same_line_stmt(
        cond,
        then_branch,
        else_branch,
        None,
    ));
}

/// The TypeScript rendering of the type the checker recorded for `e`, if any.
/// Used to annotate the slot [`hoist_if_as_statement`] declares.
pub(super) fn checked_ty_ts(e: &Expr, cx: &LowerCtx) -> Option<String> {
    cx.commons()
        .expr_ty(e.id)
        .map(|_| ts_ty(cx.commons().expr_types[&e.id].ty, cx.ty_render()))
}

/// How a `?` leaves the function when its operand is the failing side. One
/// value drives both the node guard and today's text guard, so they cannot
/// disagree about which case applies.
enum QuestionExit {
    /// #2857: a test case's `?` fails the case, reporting the `Err` at the
    /// `?`'s location. Strings are raw (the printer, or `escape_ts_string` on
    /// the text side, escapes them).
    Case {
        location: String,
        start: usize,
        end: usize,
        src: String,
    },
    /// v0.153 (ADR 0177): `None` early-returns `HttpResult.NotFound`.
    NotFound,
    /// v0.154 (ADR 0178): `Err(T.V(__rN.error))`.
    Embed { ty_name: String, variant: String },
    /// `Err` propagates unchanged.
    Plain,
}

/// The `?` guard as a statement node: `if (__rN.tag === "Err") return …;`. The
/// return is not a `Block`, so the printer keeps it inline and the output is
/// byte-identical to the text form. The case form is an `InlineBlock`, which
/// prints `{ throw …; }` on the one line today's text has.
fn question_guard_node(tmp: &str, exit: &QuestionExit) -> bynk_ts::TsStmt {
    let tag = if matches!(exit, QuestionExit::NotFound) {
        "None"
    } else {
        "Err"
    };
    let cond = bynk_ts::TsExpr::Binary {
        op: bynk_ts::TsBinaryOp::StrictEq,
        left: Box::new(member_node(tmp, "tag")),
        right: Box::new(str_node(tag)),
    };
    let call = |callee: bynk_ts::TsExpr, args: Vec<bynk_ts::TsExpr>| bynk_ts::TsExpr::Call {
        callee: Box::new(callee),
        args,
    };
    let then = match exit {
        QuestionExit::Case {
            location,
            start,
            end,
            src,
        } => {
            let detail = bynk_ts::TsExpr::Binary {
                op: bynk_ts::TsBinaryOp::Add,
                left: Box::new(str_node(format!("{src}\n  the operand was Err: "))),
                right: Box::new(call(
                    ident_node("__bynkShow"),
                    vec![member_node(tmp, "error")],
                )),
            };
            let failure = call(
                ident_node("__bynkExpectFailure"),
                vec![
                    str_node(location.clone()),
                    num_lit(start.to_string()),
                    num_lit(end.to_string()),
                    detail,
                ],
            );
            bynk_ts::TsStmt::inline_block(vec![bynk_ts::TsStmt::throw_stmt(failure, None)], None)
        }
        QuestionExit::NotFound => {
            bynk_ts::TsStmt::return_stmt(Some(member_node("HttpResult", "NotFound")), None)
        }
        QuestionExit::Embed { ty_name, variant } => bynk_ts::TsStmt::return_stmt(
            Some(call(
                ident_node("Err"),
                vec![call(
                    member_node(ty_name, variant),
                    vec![member_node(tmp, "error")],
                )],
            )),
            None,
        ),
        QuestionExit::Plain => bynk_ts::TsStmt::return_stmt(Some(ident_node(tmp)), None),
    };
    bynk_ts::TsStmt::if_stmt(cond, then, None)
}

/// [`question_guard_node`]'s text twin, kept for a text operand.
fn question_guard_text(tmp: &str, exit: &QuestionExit) -> String {
    match exit {
        QuestionExit::Case {
            location,
            start,
            end,
            src,
        } => {
            let location = crate::emitter::escape_ts_string(location);
            let src = crate::emitter::escape_ts_string(src);
            let detail = format!("\"{src}\\n  the operand was Err: \" + __bynkShow({tmp}.error)");
            format!(
                "if ({tmp}.tag === \"Err\") {{ throw __bynkExpectFailure(\"{location}\", {start}, {end}, {detail}); }}"
            )
        }
        QuestionExit::NotFound => {
            format!("if ({tmp}.tag === \"None\") return HttpResult.NotFound;")
        }
        QuestionExit::Embed { ty_name, variant } => {
            format!("if ({tmp}.tag === \"Err\") return Err({ty_name}.{variant}({tmp}.error));")
        }
        QuestionExit::Plain => format!("if ({tmp}.tag === \"Err\") return {tmp};"),
    }
}

/// `inner?` (slice 1A.8): hoists `const __rN = <inner>;` and the early-return
/// guard, and yields `__rN.value`. The two statements are nodes when the
/// operand is a node, and today's text lines otherwise (see
/// [`Pre::push_nodes_or_text`]); the yielded `__rN.value` is a genuine member
/// node either way.
fn lower_question(e: &Expr, inner: &Expr, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    // v0.153 (ADR 0177): an `Option[T]?` operand lifts into an
    // HttpResult handler — `None` early-returns `NotFound` (404),
    // `Some(v)` yields `v`. Any other operand is a `Result`: `Err`
    // propagates unchanged. Branch on the operand's checked type.
    // The checker rejects any other `?` operand, so a typed operand is
    // always present — assert it, so a future gap surfaces loudly in
    // tests rather than silently emitting the `Result` branch (which on
    // an untyped `Option` would leak `None` → `undefined`).
    let operand_ty = cx.commons().expr_types.get(&inner.id).map(|te| te.ty);
    let operand_node = operand_ty.map(|t| tys.get(t));
    debug_assert!(
        matches!(
            operand_node.as_deref(),
            Some(Ty::Option(_) | Ty::Result(_, _))
        ),
        "`?` operand has no `Option`/`Result` checked type at {:?}: {operand_node:?}",
        inner.span,
    );
    let is_option = matches!(operand_node.as_deref(), Some(Ty::Option(_)));
    // v0.154 (ADR 0178): a `Result` operand whose error type differs from
    // the enclosing return's is converted by a declared embedding
    // (`embeds E as V`) — the same rule the checker accepted it under.
    let embed = if is_option || cx.question_fails_case() {
        None
    } else {
        embed_conversion(operand_ty, cx)
    };
    let mut pre = Pre::new();
    let operand = pre.lower_node(inner, cx);
    let tmp = cx.fresh();
    let exit = if cx.question_fails_case() {
        // #2857: a case returns a `CaseResult`, not the operand's
        // `Result` — so the `?` fails the case, reporting the `Err`
        // value at the `?`'s location. (The checker rejects an `Option`
        // operand outside an `HttpResult` handler.)
        QuestionExit::Case {
            location: expect_location_raw(cx, e.span.start),
            start: e.span.start,
            end: e.span.end,
            src: expect_source_text_raw(cx, e.span),
        }
    } else if is_option {
        QuestionExit::NotFound
    } else if let Some((ty_name, variant)) = embed {
        QuestionExit::Embed { ty_name, variant }
    } else {
        QuestionExit::Plain
    };
    pre.push_nodes_or_text(
        vec![operand],
        |c| {
            vec![
                bynk_ts::TsStmt::const_stmt(
                    bynk_ts::TsBindingName::Ident(tmp.clone()),
                    None,
                    one(c),
                    None,
                ),
                question_guard_node(&tmp, &exit),
            ]
        },
        |c| {
            vec![
                format!("const {tmp} = {};", one(c)),
                question_guard_text(&tmp, &exit),
            ]
        },
    );
    // T2.3 (R6.3): every branch above just pushed a `return` that must
    // exit the enclosing function, not whatever scope this `Lowered`
    // eventually lands in — see `LowerCtx::emitted_early_return`.
    cx.emitted_early_return = true;
    pre.finish(LoweredExpr::Node(member_node(&tmp, "value")))
}

/// v0.59: the `location` string an `expect` failure carries. With a test-body
/// [`AssertLoc`](crate::emitter::ctx::AssertLoc) in scope it is a real, escaped
/// `path:line:col` (so `--format json` consumers can link to the source);
/// otherwise it falls back to the bare byte offset (`expect`s only appear in test
/// bodies, so the fallback is defensive).
pub(super) fn expect_location(cx: &LowerCtx, offset: usize) -> String {
    crate::emitter::escape_ts_string(&expect_location_raw(cx, offset))
}

/// [`expect_location`] before escaping, for a `Str` node (the printer escapes
/// it with the same one function, `escape_ts_literal`).
pub(super) fn expect_location_raw(cx: &LowerCtx, offset: usize) -> String {
    match cx.assert_loc() {
        Some(loc) => {
            let (line, col) = bynk_syntax::span::line_col(&loc.source, offset);
            // Normalise to forward slashes so the location is identical on
            // Windows (where `PathBuf` joins with `\`) — matching the
            // diagnostic path rendering and the committed goldens.
            let path = loc.rel_path.replace('\\', "/");
            format!("{path}:{line}:{col}")
        }
        None => format!("offset {offset}"),
    }
}

/// v0.112: the trimmed source text of an `expect` predicate, embedded in the
/// structural failure report (`expect <src>`), unescaped: it goes into a `Str`
/// node (the printer escapes it) or through `escape_ts_string` for the text form.
/// Empty when no test-body source is in scope (defensive — `expect`s only appear
/// in test bodies).
pub(super) fn expect_source_text_raw(cx: &LowerCtx, span: bynk_syntax::span::Span) -> String {
    match cx.assert_loc() {
        Some(loc) => loc
            .source
            .get(span.start..span.end)
            .unwrap_or("")
            .trim()
            .to_string(),
        None => String::new(),
    }
}

/// A compile-time literal that v0.9.4 may admit as a refined type (int, float,
/// decimal or string), as a node (1A.17, Decision D). `None` for anything else.
fn lower_const_literal_node(e: &Expr) -> Option<bynk_ts::TsExpr> {
    match &e.kind {
        ExprKind::IntLit { value: n, .. } => Some(num_lit(n.to_string())),
        // v0.21: the stored lexeme verbatim.
        ExprKind::FloatLit { lexeme, .. } => Some(num_lit(lexeme.clone())),
        ExprKind::DecimalLit { coef, scale, .. } => Some(decimal_lit(coef, *scale)),
        ExprKind::StrLit(s) => Some(str_node(s.as_str())),
        // A negated numeric literal — admissible at compile time (the
        // checker folds the sign in `const_literal`), so the lowering must
        // route it through `unsafe` like any other admitted literal.
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => match &inner.kind {
            ExprKind::IntLit { value: n, .. } => Some(num_lit(format!("-{n}"))),
            ExprKind::FloatLit { lexeme, .. } => Some(num_lit(format!("-{lexeme}"))),
            ExprKind::DecimalLit { coef, scale, .. } => {
                Some(decimal_lit(&format!("-{coef}"), *scale))
            }
            _ => None,
        },
        _ => None,
    }
}

/// [`lower_const_literal_node`] printed, for `lower_expr`'s refined-literal
/// branch (still text: it also builds the production `(lit as T)` form).
fn lower_const_literal_raw(e: &Expr) -> Option<String> {
    lower_const_literal_node(e).map(|n| bynk_ts::print_expr(&n))
}

/// The text of a unary operator applied to its operand's lowered `inner`.
///
/// A negated negation (`- -x`, `- -5`) would otherwise splice to `--x`, which
/// TypeScript reads as a pre-decrement (or, on a literal, a syntax error), so a
/// `-` operand whose text begins with `-` is bracketed, as the `bynk_ts`
/// printer does for `TsUnaryOp::Neg`: `-(-x)`.
fn unary_text(op: UnaryOp, inner: &str) -> String {
    let sym = match op {
        UnaryOp::Neg => "-",
        UnaryOp::Not => "!",
    };
    let (open, close) = if op == UnaryOp::Neg && inner.starts_with('-') {
        ("(", ")")
    } else {
        ("", "")
    };
    format!("{sym}{open}{inner}{close}")
}

/// A literal's data, destructured from its `ExprKind` by `lower_expr`'s own
/// arms, so [`Lit::lower`] matches an exhaustive type with no catch-all.
enum Lit<'a> {
    /// The integer's digits.
    Int(String),
    /// The stored lexeme, verbatim.
    Float(&'a str),
    Decimal(&'a str, u32),
    /// The duration's resolved milliseconds (v0.86, ADR 0112): the value `Duration` erases to.
    Duration(String),
    Str(&'a str),
    Bool(bool),
    Unit,
    None,
}

/// Slice 1A.2 (#3694): the literal family lowers to typed nodes ([`Lit::lower`]). The string's
/// escaping is `TsLit::Str`'s (the one shared escaper); numbers print their
/// digits verbatim; `None` and the unit value are the runtime's `None` /
/// `undefined`.
impl Lit<'_> {
    fn lower(self) -> Lowered {
        use bynk_ts::{TsExpr, TsLit};
        Lowered::node(match self {
            Lit::Int(digits) | Lit::Duration(digits) => num_lit(digits),
            // v0.21: the stored lexeme verbatim — `1e10` must not normalise.
            Lit::Float(lexeme) => num_lit(lexeme.to_string()),
            // #2622: a `Decimal` literal is an object literal (no runtime import).
            Lit::Decimal(coef, scale) => decimal_lit(coef, scale),
            Lit::Str(s) => TsExpr::Lit(TsLit::Str(s.to_string())),
            Lit::Bool(b) => TsExpr::Lit(TsLit::Bool(b)),
            Lit::None => TsExpr::Ident("None".to_string()),
            Lit::Unit => TsExpr::Ident("undefined".to_string()),
        })
    }
}

/// A numeric literal node: the digits are printed verbatim (`TsLit::Num`), so a
/// big integer, a float lexeme (`1e10`) or a duration's milliseconds render
/// exactly as the old hand-built text did.
pub(super) fn num_lit(digits: String) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(digits))
}

/// #2622: a `Decimal` literal as an object-literal node — the node twin of
/// [`decimal_ts_literal`](crate::emitter::ts_types::decimal_ts_literal)
/// (`{ coefficient: <signed>n, scale: <scale> }`, no runtime import).
pub(super) fn decimal_lit(coef: &str, scale: u32) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::object(vec![
        (
            "coefficient".to_string(),
            num_lit(format!("{}n", decimal_signed_coef(coef))),
        ),
        ("scale".to_string(), num_lit(scale.to_string())),
    ])
}

/// Lower an interpolated string (v0.43, ADR 0075) to a TS template literal.
/// Chunks become escaped literal text; each hole becomes `${String(<expr>)}`.
/// `String(…)` is identity for a `String` hole and the display form for
/// `Int`/`Float`/`Bool` — and the checker guarantees only base scalars reach
/// here, so no `[object Object]` can be emitted.
fn lower_interp_str(parts: &[InterpPart], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // `chunks` are the already-escaped literal parts (`escape_ts_template`, which
    // `TemplateLit` prints verbatim): always one more than there are holes, with
    // an empty chunk between adjacent holes.
    let mut chunks: Vec<String> = vec![String::new()];
    let mut holes: Vec<LoweredExpr> = Vec::new();
    let mut decimal_holes: Vec<bool> = Vec::new();
    for part in parts {
        match part {
            InterpPart::Chunk(text) => {
                if let Some(last) = chunks.last_mut() {
                    last.push_str(&escape_ts_template(text));
                }
            }
            InterpPart::Hole(hole) => {
                holes.push(pre.lower_node(hole, cx));
                // #2622: a `Decimal` prints as its canonical text, not as the
                // host's `[object Object]`.
                let decimal = cx
                    .commons()
                    .expr_types
                    .get(&hole.id)
                    .and_then(|te| te.ty.base(cx.commons().tys()))
                    == Some(BaseType::Decimal);
                if decimal {
                    cx.note_decimal("__bynkDecimalToString");
                }
                decimal_holes.push(decimal);
                chunks.push(String::new());
            }
        }
    }
    let wrapper = |decimal: bool| {
        if decimal {
            "__bynkDecimalToString"
        } else {
            "String"
        }
    };
    let built = node_or_text(
        holes,
        |nodes| {
            let exprs = nodes
                .into_iter()
                .zip(&decimal_holes)
                .map(|(hole, d)| bynk_ts::TsExpr::Call {
                    callee: Box::new(ident_node(wrapper(*d))),
                    args: vec![hole],
                })
                .collect();
            bynk_ts::TsExpr::template_lit(chunks.clone(), exprs)
        },
        |texts| {
            let mut out = String::from("`");
            for (i, chunk) in chunks.iter().enumerate() {
                out.push_str(chunk);
                if let Some(text) = texts.get(i) {
                    out.push_str(&format!("${{{}({text})}}", wrapper(decimal_holes[i])));
                }
            }
            out.push('`');
            out
        },
    );
    pre.finish_built(built)
}

/// Escape a literal chunk for a TS template-literal context: backslash,
/// backtick, and `$` (to neutralise `${`), plus the control-char escapes
/// [`escape_ts_string`] applies. (v0.43.)
fn escape_ts_template(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '`' => out.push_str("\\`"),
            '$' => out.push_str("\\$"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            // #2619: `\0`, other control characters and the JS line terminators
            // U+2028/U+2029 print as `\u{…}` escapes (valid in strings and templates).
            c if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') => {
                out.push_str(&format!("\\u{{{:X}}}", c as u32));
            }
            c => out.push(c),
        }
    }
    out
}

/// #1 review: an expression's lowered text, paired with the statements that
/// must run before it — the typed replacement for the `stmts: &mut
/// Vec<String>` sink `lower_expr_into` used to take. A caller that receives a
/// `Lowered` cannot accidentally drop `pre` (unlike the old sink, dropping it
/// took no code at all) or splice it into a string where a statement is
/// required (`Lowered` is not a `String`) — the two failure modes the sink
/// pattern actually produced (`lower_if`'s/`lower_tail_expr`'s ternary paths,
/// `lower_and_with_is`'s splice).
///
/// T2.1 (R6.2): this is now the *only* way a lowering function hands
/// statements to its caller. There is no sink parameter left in `bynk-emit`
/// to hand them to instead.
pub(crate) struct Lowered {
    pub pre: Vec<LoweredStmt>,
    pub expr: LoweredExpr,
}

/// Slice 1A.2 (#3694): the expression side of the lowering bridge. A lowering
/// function returns either a typed [`TsExpr`] (a *converted* family) or text
/// (an *unconverted* one). Families convert one at a time, leaf first; a
/// converted child reaches an unconverted parent as text printed by the
/// `bynk_ts` printer, through [`LoweredExpr::into_text`] — the one place a node
/// becomes text inside lowering. The `Text` side is counted by the
/// `lowering_text_bridge` growth probe and is deleted when 1A exits.
#[derive(Clone)]
pub(crate) enum LoweredExpr {
    Node(bynk_ts::TsExpr),
    Text(String),
}

impl LoweredExpr {
    /// The only node-to-text conversion inside lowering: a `Node` is printed
    /// by the `bynk_ts` expression printer, `Text` passes through.
    pub(crate) fn into_text(self) -> String {
        match self {
            LoweredExpr::Node(n) => bynk_ts::print_expr(&n),
            LoweredExpr::Text(t) => t,
        }
    }
}

/// Slice 1A.3 (#3696), Decision A: the one mixed-children rule, and the 1A
/// convention. A converted parent builds a node only when *every* child is a
/// node (`node` receives them in order); if any child is still text, every
/// child is rendered through [`LoweredExpr::into_text`] and `text` builds
/// today's string. A text child is never wrapped in a `VerbatimExpr` or an
/// `Ident` carrying expression text.
///
/// It returns `Ok(node)` or `Err(text)`: the text side leaves through
/// `lower_expr`'s one shared text exit (`finish_text`) rather than opening a
/// second text construction site (see `built_or_text!`).
pub(super) type Built = Result<bynk_ts::TsExpr, String>;

impl From<Built> for LoweredExpr {
    /// A built value as a bridge value: the node, else its text.
    fn from(built: Built) -> Self {
        match built {
            Ok(n) => LoweredExpr::Node(n),
            Err(t) => LoweredExpr::Text(t),
        }
    }
}

pub(super) fn node_or_text(
    children: Vec<LoweredExpr>,
    node: impl FnOnce(Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr,
    text: impl FnOnce(Vec<String>) -> String,
) -> Built {
    if children.iter().all(|c| matches!(c, LoweredExpr::Node(_))) {
        let nodes = children
            .into_iter()
            .filter_map(|c| match c {
                LoweredExpr::Node(n) => Some(n),
                LoweredExpr::Text(_) => None,
            })
            .collect();
        Ok(node(nodes))
    } else {
        Err(text(
            children.into_iter().map(LoweredExpr::into_text).collect(),
        ))
    }
}

/// `-x` / `!x`: a `Unary` node (the printer brackets the operand), or today's
/// `{sym}{operand}` text.
fn unary_op(op: UnaryOp, child: LoweredExpr) -> Built {
    let ts_op = match op {
        UnaryOp::Neg => bynk_ts::TsUnaryOp::Neg,
        UnaryOp::Not => bynk_ts::TsUnaryOp::Not,
    };
    node_or_text(
        vec![child],
        |c| bynk_ts::TsExpr::Unary {
            op: ts_op,
            expr: Box::new(one(c)),
        },
        // The text side shares `unary_text` (#3698), which brackets a negated
        // negation so `- -x` never prints as the decrement `--x`.
        |c| unary_text(op, &one(c)),
    )
}

/// `[a, b, …]`: an `Array` node, or today's `[a, b]` text.
fn array(children: Vec<LoweredExpr>) -> Built {
    node_or_text(
        children,
        |items| bynk_ts::TsExpr::Array {
            items,
            multiline: false,
        },
        |items| format!("[{}]", items.join(", ")),
    )
}

/// An explicit source `( … )`: `bynk_ts` has `TsExpr::Paren`, so the source's
/// own parentheses are kept as a node.
fn paren(child: LoweredExpr) -> Built {
    node_or_text(
        vec![child],
        |c| bynk_ts::TsExpr::Paren(Box::new(one(c))),
        |c| format!("({})", one(c)),
    )
}

/// A genuine identifier node. Never built from expression text.
pub(super) fn ident_node(name: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Ident(name.to_string())
}

/// `(test ? consequent : alternate)`: a `Paren(Conditional)` node, or today's
/// text. Shared by the value-position ternary `if` and the statement emitter's
/// ternary tail (1A.11), so both keep today's outer brackets.
pub(super) fn ternary_built(
    test: LoweredExpr,
    consequent: LoweredExpr,
    alternate: LoweredExpr,
) -> Built {
    node_or_text(
        vec![test, consequent, alternate],
        |c| {
            let [test, consequent, alternate]: [bynk_ts::TsExpr; 3] = c
                .try_into()
                .unwrap_or_else(|_| unreachable!("a ternary has three children"));
            bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Conditional {
                test: Box::new(test),
                consequent: Box::new(consequent),
                alternate: Box::new(alternate),
            }))
        },
        |c| format!("({} ? {} : {})", c[0], c[1], c[2]),
    )
}

/// `object.property` over a genuine identifier `object`.
pub(super) fn member_node(object: &str, property: &str) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Member {
        object: Box::new(ident_node(object)),
        property: property.to_string(),
    }
}

/// Finish `lower_expr`'s arm from a [`Built`]: a node returns at once; text
/// falls out of the match as the arm's `String`, to the shared `finish_text`.
macro_rules! built_or_text {
    ($pre:ident, $built:expr) => {
        match $built {
            Ok(node) => return $pre.finish(LoweredExpr::Node(node)),
            Err(text) => text,
        }
    };
}

/// As [`built_or_text!`], for an already-built [`Lowered`] (its hoists join
/// `$pre`): a node returns at once, text falls out to the shared exit.
macro_rules! lowered_or_text {
    ($pre:ident, $lowered:expr) => {
        match $pre.absorb_node($lowered) {
            LoweredExpr::Node(node) => return $pre.finish(LoweredExpr::Node(node)),
            LoweredExpr::Text(text) => text,
        }
    };
}

/// A runtime-constructor call with one argument — `Ok(x)`, `Some(x)`,
/// `__bynkDecimalNeg(x)`, or `HttpResult.Ok(x)` when `object` is given — as a
/// call node over genuine identifiers, or today's text if the argument is text.
fn runtime_ctor_call(object: Option<&'static str>, name: &'static str, arg: LoweredExpr) -> Built {
    node_or_text(
        vec![arg],
        |c| {
            let callee = match object {
                Some(o) => bynk_ts::TsExpr::Member {
                    object: Box::new(bynk_ts::TsExpr::Ident(o.to_string())),
                    property: name.to_string(),
                },
                None => bynk_ts::TsExpr::Ident(name.to_string()),
            };
            bynk_ts::TsExpr::Call {
                callee: Box::new(callee),
                args: vec![one(c)],
            }
        },
        |c| match object {
            Some(o) => format!("{o}.{name}({})", one(c)),
            None => format!("{name}({})", one(c)),
        },
    )
}

/// `callee(a, b, …)` over a genuine callee node: a `Call` node, or today's
/// `callee(a, b)` text (the callee prints exactly as it is spelled).
fn call_built(callee: bynk_ts::TsExpr, args: Vec<LoweredExpr>) -> Built {
    let callee_text = bynk_ts::print_expr(&callee);
    node_or_text(
        args,
        |args| bynk_ts::TsExpr::Call {
            callee: Box::new(callee),
            args,
        },
        |args| format!("{callee_text}({})", args.join(", ")),
    )
}

/// Witness nodes printed for a text caller (1A: the capability, closure and
/// kernel families are still text).
fn print_witnesses(nodes: Vec<bynk_ts::TsExpr>) -> impl Iterator<Item = String> {
    nodes.into_iter().map(|n| bynk_ts::print_expr(&n))
}

/// A string-literal node: the printer escapes it exactly as
/// `escape_ts_string` does (they are one function, `escape_ts_literal`).
pub(super) fn str_node(s: impl Into<String>) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(s.into()))
}

/// The single element of a one-child builder's argument.
fn one<T>(children: Vec<T>) -> T {
    let mut it = children.into_iter();
    match (it.next(), it.next()) {
        (Some(only), None) => only,
        _ => unreachable!("a one-child builder is given exactly one child"),
    }
}

/// The statement side of the bridge: a hoisted statement is either a typed
/// [`TsStmt`] or text. Where `pre` statements leave lowering into a `TsStmt`
/// body ([`Pre::ts_stmts`]) a `Node` stays a node and a `Text` becomes
/// `TsStmt::raw`, exactly as before.
pub(crate) enum LoweredStmt {
    // Boxed: a `TsStmt` is ~350 bytes and almost every hoisted statement is text.
    Node(Box<bynk_ts::TsStmt>),
    Text(String),
}

impl LoweredStmt {
    /// The statement as a statement-body item (see [`Lowered::pre_items`]).
    pub(super) fn item(&self) -> Item {
        match self {
            LoweredStmt::Node(n) => Item::Node(n.clone()),
            LoweredStmt::Text(t) => Item::Line(t.clone()),
        }
    }

    /// The statement as lines of text, for a caller assembling statements as
    /// text (the statement-side counterpart of [`LoweredExpr::into_text`]; no
    /// family builds `Node` statements yet). A `Text` is one entry, as it
    /// always was. A `Node` prints at depth 0 and yields one entry per line,
    /// the printer's own nesting kept as leading spaces, so a caller that
    /// indents each entry (`write_line`) indents every line of a multi-line
    /// statement, not only the first.
    fn lines(&self) -> Vec<String> {
        match self {
            LoweredStmt::Node(n) => bynk_ts::print_stmt(n, 0)
                .lines()
                .map(str::to_string)
                .collect(),
            LoweredStmt::Text(t) => vec![t.clone()],
        }
    }
}

impl Lowered {
    /// An expression that hoists nothing, built as text — the common leaf
    /// case. A text construction site: counted by `lowering_text_bridge`.
    pub(crate) fn bare(expr: impl Into<String>) -> Self {
        Lowered {
            pre: Vec::new(),
            expr: LoweredExpr::Text(expr.into()),
        }
    }

    /// An expression that hoists nothing, built as a typed node.
    pub(crate) fn node(expr: bynk_ts::TsExpr) -> Self {
        Lowered {
            pre: Vec::new(),
            expr: LoweredExpr::Node(expr),
        }
    }

    /// The hoisted statements as text, for a caller that assembles them into
    /// text.
    pub(crate) fn pre_texts(&self) -> Vec<String> {
        self.pre.iter().flat_map(LoweredStmt::lines).collect()
    }

    /// The hoisted statements as statement-body items, for the statement
    /// emitter: a `Text` becomes a `Line`, a `Node` stays a `Node`.
    pub(super) fn pre_items(&self) -> Vec<Item> {
        self.pre.iter().map(LoweredStmt::item).collect()
    }

    /// Whether every hoisted statement is a node (1A.11: a statement built
    /// over these is a node only if it absorbs nothing but nodes).
    pub(super) fn pre_all_nodes(&self) -> bool {
        self.pre.iter().all(|s| matches!(s, LoweredStmt::Node(_)))
    }
}

/// T2.1 (R6.2): the statements an expression under construction has hoisted so
/// far. This is the *owned* counterpart to the deleted sink: a `Pre` belongs to
/// the one function assembling an expression, and leaves it only by being
/// [finished](Pre::finish) into that expression's [`Lowered`]. It is
/// deliberately never a parameter — that is the whole point. A lowering
/// function cannot be handed somewhere to write statements, so it cannot write
/// them anywhere but its own return value, and the caller decides where they
/// land (a real statement position, or a hoist-safe wrapper).
pub(crate) struct Pre(Vec<LoweredStmt>);

/// The text-exit counters, readable from outside the crate.
///
/// `TEXT_EXITS` and `TEXT_STMTS` (and `lower_stmt`'s `LINES_RENDERED`) count into
/// thread-locals that exist only when this crate is built for its own tests
/// (`cfg(test)`) or with the `text-exit-counters` feature. A test that lowers
/// through `bynk_driver` runs the driver's own, non-test copy of this crate, so
/// the `cfg(test)` counters read 0 there and a `== 0` assertion proves nothing.
/// Such a test must live in a crate that depends on this one with the feature on
/// (`bynk-tests` does) and read these functions; a test inside this crate reads
/// the counters directly and must lower through this crate's own
/// [`compile`](crate::compile), never through `testkit::emit_project`.
#[cfg(feature = "text-exit-counters")]
pub mod text_counters {
    /// Zero both counters (the current thread's).
    pub fn reset() {
        super::TEXT_EXITS.with(|c| c.set(0));
        super::TEXT_STMTS.with(|c| c.set(0));
    }

    /// Expressions that have left lowering as text since [`reset`].
    pub fn exits() -> usize {
        super::TEXT_EXITS.with(|c| c.get())
    }

    /// Statements hoisted as text since [`reset`].
    pub fn stmts() -> usize {
        super::TEXT_STMTS.with(|c| c.get())
    }
}

#[cfg(any(test, feature = "text-exit-counters"))]
thread_local! {
    /// Test-only: how many expressions left lowering as text (1A.4's
    /// "now lowers to a node" tests read it). Compiled only for bynk-emit's own
    /// test build and under the `text-exit-counters` feature (see `lower::text_counters`).
    static TEXT_EXITS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Test-only: how many statements were hoisted into a [`Pre`] as text
    /// (1A.8's "now hoists nodes" tests read it).
    static TEXT_STMTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Pre {
    pub(crate) fn new() -> Self {
        Pre(Vec::new())
    }

    /// Lower `e`, absorbing whatever it hoists, and hand back its text. The
    /// replacement for `lower_expr_into(e, stmts, cx)`: identical ordering —
    /// the callee's statements land here, in order, at the point of the call —
    /// but the callee never saw the buffer.
    pub(crate) fn lower(&mut self, e: &Expr, cx: &mut LowerCtx) -> String {
        self.lower_node(e, cx).into_text()
    }

    /// As [`Pre::lower`], for a converted caller: hands back the bridge value
    /// without printing it.
    pub(crate) fn lower_node(&mut self, e: &Expr, cx: &mut LowerCtx) -> LoweredExpr {
        let lowered = lower_expr(e, cx);
        self.absorb_node(lowered)
    }

    /// Absorb an already-built [`Lowered`] and hand back its text.
    pub(crate) fn absorb(&mut self, lowered: Lowered) -> String {
        self.absorb_node(lowered).into_text()
    }

    /// Absorb an already-built [`Lowered`] and hand back its expression as
    /// the bridge value.
    pub(crate) fn absorb_node(&mut self, lowered: Lowered) -> LoweredExpr {
        self.0.extend(lowered.pre);
        lowered.expr
    }

    /// Absorb an optional [`Lowered`] — a kernel lowerer that may decline the
    /// shape. Declining hoists nothing, so `None` leaves `self` untouched.
    pub(crate) fn absorb_opt(&mut self, lowered: Option<Lowered>) -> Option<String> {
        lowered.map(|l| self.absorb(l))
    }

    /// Hoist a statement this function generated itself (a `?` temp, an
    /// `is`-receiver binding, a match IIFE's scaffolding), built as text. A
    /// text construction site: counted by `lowering_text_bridge`.
    pub(crate) fn push_text(&mut self, stmt: String) {
        #[cfg(any(test, feature = "text-exit-counters"))]
        TEXT_STMTS.with(|c| c.set(c.get() + 1));
        self.0.push(LoweredStmt::Text(stmt));
    }

    /// Hoist a statement this function generated itself, built as a typed
    /// node. The statement-side counterpart of [`Pre::finish`] with a node:
    /// never built from statement text.
    pub(crate) fn push_node(&mut self, stmt: bynk_ts::TsStmt) {
        self.0.push(LoweredStmt::Node(Box::new(stmt)));
    }

    /// Slice 1A.8, Decision A: [`node_or_text`]'s rule for hoisted statements.
    /// Statement nodes are hoisted only when *every* child expression is a node
    /// (`node` receives them in order); if any is text, every child is rendered
    /// through [`LoweredExpr::into_text`] and `text` builds today's lines. A
    /// text statement is never wrapped in `TsStmt::raw` to fit inside a node.
    pub(crate) fn push_nodes_or_text(
        &mut self,
        children: Vec<LoweredExpr>,
        node: impl FnOnce(Vec<bynk_ts::TsExpr>) -> Vec<bynk_ts::TsStmt>,
        text: impl FnOnce(Vec<String>) -> Vec<String>,
    ) {
        if children.iter().all(|c| matches!(c, LoweredExpr::Node(_))) {
            let nodes = children
                .into_iter()
                .filter_map(|c| match c {
                    LoweredExpr::Node(n) => Some(n),
                    LoweredExpr::Text(_) => None,
                })
                .collect();
            for stmt in node(nodes) {
                self.push_node(stmt);
            }
        } else {
            let texts = children.into_iter().map(LoweredExpr::into_text).collect();
            self.extend_text(text(texts));
        }
    }

    /// Hoist several text statements; counted like [`Pre::push_text`].
    pub(crate) fn extend_text(&mut self, stmts: impl IntoIterator<Item = String>) {
        let stmts: Vec<String> = stmts.into_iter().collect();
        #[cfg(any(test, feature = "text-exit-counters"))]
        TEXT_STMTS.with(|c| c.set(c.get() + stmts.len()));
        self.0.extend(stmts.into_iter().map(LoweredStmt::Text));
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The accumulated statements as text, for a caller that is writing them
    /// into a text statement position rather than hoisting them further.
    pub(crate) fn stmts(&self) -> Vec<String> {
        self.0.iter().flat_map(LoweredStmt::lines).collect()
    }

    /// The accumulated statements as statement-body items (see
    /// [`Lowered::pre_items`]).
    pub(super) fn items(&self) -> Vec<Item> {
        self.0.iter().map(LoweredStmt::item).collect()
    }

    /// Whether every statement hoisted so far is a node (see
    /// [`Lowered::pre_all_nodes`]).
    pub(super) fn all_nodes(&self) -> bool {
        self.0.iter().all(|s| matches!(s, LoweredStmt::Node(_)))
    }

    /// The accumulated statements as `TsStmt`s for a typed body: a `Node`
    /// stays a node, a `Text` becomes `TsStmt::raw` indented by `indent`.
    /// `indent` is the indentation the printer will give the body's own
    /// statements (verbatim raw text carries its own); a `Node` needs none
    /// because the printer nests it at that same depth.
    pub(crate) fn ts_stmts(&self, indent: &str) -> Vec<bynk_ts::TsStmt> {
        self.0
            .iter()
            .map(|s| match s {
                LoweredStmt::Node(n) => (**n).clone(),
                LoweredStmt::Text(t) => bynk_ts::TsStmt::raw(format!("{indent}{t}\n"), None),
            })
            .collect()
    }

    /// Pair the accumulated statements with the expression they precede.
    pub(crate) fn finish(self, expr: LoweredExpr) -> Lowered {
        #[cfg(any(test, feature = "text-exit-counters"))]
        if matches!(expr, LoweredExpr::Text(_)) {
            TEXT_EXITS.with(|c| c.set(c.get() + 1));
        }
        Lowered { pre: self.0, expr }
    }

    /// [`Pre::finish`] with an expression built as text. A text construction
    /// site: counted by `lowering_text_bridge`.
    pub(crate) fn finish_text(self, expr: impl Into<String>) -> Lowered {
        self.finish(LoweredExpr::Text(expr.into()))
    }

    /// Finish from a [`Built`]: a node exits as a node, text through
    /// [`Pre::finish_text`] (the one shared text exit for the call family).
    pub(crate) fn finish_built(self, built: Built) -> Lowered {
        match built {
            Ok(node) => self.finish(LoweredExpr::Node(node)),
            Err(text) => self.finish_text(text),
        }
    }
}

/// The canonical entry point: lower `e`, returning its text and whatever
/// statements must run first.
pub(crate) fn lower_expr(e: &Expr, cx: &mut LowerCtx) -> Lowered {
    // v0.9.4: a literal the checker admitted as a refined type (expected-type-
    // directed construction) is branded directly — the refinement was already
    // verified at compile time, so there is no runtime check and no `Result`.
    // ADR 0182: a refined/alias type has no public `.unsafe`, so this brands with
    // an inline `as` cast (`unchecked_construct` with `is_opaque = false`) rather
    // than calling one; opaque is never admitted here (`NamedKind::Refined` only).
    if let Some(Ty::Named {
        name,
        kind: NamedKind::Refined(_),
        ..
    }) = cx.commons().expr_ty(e.id).as_deref()
        && let Some(raw) = lower_const_literal_raw(e)
    {
        // ADR 0182: in GENERATED TEST scaffolding a branded type is in scope only
        // as an `any`-typed value binding (`const { T } = ns as any`), never as a
        // type, so a `(raw as T)` cast fails to resolve `T` ("refers to a value,
        // but is being used as a type"). Brand via `unchecked_construct_test`
        // (refined → `(raw as any)`) there, exactly as the `Val[T](lit)` mock path
        // does. Production emission keeps the real `(raw as T)` brand.
        if cx.in_test_scaffold() {
            return Lowered::bare(unchecked_construct_test(name, &raw, false));
        }
        // #2058: a consumed context's refined type is named under its
        // namespace; the module imports no bare name for it.
        return Lowered::bare(unchecked_construct(
            &cx.consumed_type_name(name),
            &raw,
            false,
        ));
    }
    let mut pre = Pre::new();
    let expr = match &e.kind {
        // Literals are typed nodes (slice 1A.2): see `Lit::lower`.
        ExprKind::IntLit { value, .. } => return Lit::Int(value.to_string()).lower(),
        ExprKind::FloatLit { lexeme, .. } => return Lit::Float(lexeme).lower(),
        ExprKind::DecimalLit { coef, scale, .. } => return Lit::Decimal(coef, *scale).lower(),
        ExprKind::DurationLit { millis, .. } => return Lit::Duration(millis.to_string()).lower(),
        ExprKind::StrLit(s) => return Lit::Str(s).lower(),
        ExprKind::BoolLit(b) => return Lit::Bool(*b).lower(),
        ExprKind::None => return Lit::None.lower(),
        ExprKind::UnitLit => return Lit::Unit.lower(),
        // Slice C: `Wire(<String>)` in a generic position lowers to its raw inner
        // string. The system-http driver site intercepts `Wire` args before this
        // to route them raw (no serialisation) and switch to the outcome decoder.
        ExprKind::Wire(inner) => {
            let child = pre.lower_node(inner, cx);
            return pre.finish(child);
        }
        // v0.43 (ADR 0075): an interpolated string lowers to a TS template
        // literal — chunks as escaped literal text, holes as `${String(…)}`.
        ExprKind::InterpStr(parts) => lowered_or_text!(pre, lower_interp_str(parts, cx)),
        // v0.20b: a list literal lowers to a TS array literal; `readonly` is
        // a type-level property and the checker owns the element typing.
        ExprKind::ListLit(elems, ..) => {
            let children: Vec<LoweredExpr> =
                elems.iter().map(|el| pre.lower_node(el, cx)).collect();
            built_or_text!(pre, array(children))
        }
        ExprKind::Ident(id) => built_or_text!(pre, lower_ident(e, id, cx)),
        ExprKind::Call { name, args, .. } => lowered_or_text!(pre, lower_call(e, name, args, cx)),
        ExprKind::UnaryOp(UnaryOp::Neg, inner)
            if cx
                .commons()
                .expr_types
                .get(&inner.id)
                .and_then(|te| te.ty.base(cx.commons().tys()))
                == Some(BaseType::Decimal) =>
        {
            // #2622: negating a `Decimal` is exact, not a JS unary minus.
            let child = pre.lower_node(inner, cx);
            cx.note_decimal("__bynkDecimalNeg");
            built_or_text!(pre, runtime_ctor_call(None, "__bynkDecimalNeg", child))
        }
        ExprKind::UnaryOp(op, inner) => {
            built_or_text!(pre, unary_op(*op, pre.lower_node(inner, cx)))
        }
        ExprKind::BinOp(op, lhs, rhs) => lowered_or_text!(pre, lower_bin_op(*op, lhs, rhs, cx)),
        ExprKind::Paren(inner) => {
            built_or_text!(pre, paren(pre.lower_node(inner, cx)))
        }
        ExprKind::Ok(inner) => {
            let child = pre.lower_node(inner, cx);
            // v0.9: `Ok` is overloaded — use the checker's recorded type to
            // decide between `Result.Ok` and `HttpResult.Ok`.
            let http = matches!(
                cx.commons().expr_ty(e.id).as_deref(),
                Some(Ty::HttpResult(_))
            );
            if http {
                cx.note_http_result();
            }
            built_or_text!(
                pre,
                runtime_ctor_call(http.then_some("HttpResult"), "Ok", child)
            )
        }
        ExprKind::Err(inner) => {
            let arg = pre.lower_node(inner, cx);
            built_or_text!(pre, runtime_ctor_call(None, "Err", arg))
        }
        ExprKind::Some(inner) => {
            let child = pre.lower_node(inner, cx);
            built_or_text!(pre, runtime_ctor_call(None, "Some", child))
        }
        ExprKind::Question(inner) => return lower_question(e, inner, cx),
        ExprKind::RecordConstruction {
            type_name, fields, ..
        } => lowered_or_text!(pre, lower_record_construction(type_name, fields, cx)),
        ExprKind::FieldAccess { .. }
            if matches!(
                cx.commons().callees.get(&e.id),
                Some(bynk_check::checker::Callee::Intrinsic { ns, .. })
                    if *ns == bynk_check::checker::RUN_ID_NS
            ) =>
        {
            // #2138: `run.id` — the module's `__bynkRunId`.
            return pre.finish(LoweredExpr::Node(ident_node("__bynkRunId")));
        }
        ExprKind::FieldAccess { receiver, field } => {
            lowered_or_text!(pre, lower_field_access(e, receiver, field, cx))
        }
        ExprKind::MethodCall { .. } => lowered_or_text!(pre, lower_method_call(e, cx)),
        // #1750: a value-position `if`/`match`/block that holds a `?` becomes a
        // real statement, so the `?`'s `return` exits the function rather than
        // an arrow. A ternary-shaped `if` is left to `lower_if`, whose own
        // hoist already handles a branch that hoists.
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } if !ternary_shaped(cond, then_block, else_block, cx)
            && (block_contains_question(then_block) || block_contains_question(else_block)) =>
        {
            pre.absorb(hoist_value_as_statement(e, cx, |body, cx| {
                if_tail(body, cond, then_block, else_block, cx, false)
            }))
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => lowered_or_text!(pre, lower_if(e, cond, then_block, else_block, cx)),
        // v0.20a: a lambda lowers to a TS arrow; `async` iff its checked type
        // is an effectful function. A lambda is deliberately its own return
        // scope (ADR 0178) — its body is lowered with no access to the outer
        // `pre`, so anything it hoists (match-as-IIFE etc.) stays local to
        // the arrow, correctly, because a `?`'s `return` there is supposed to
        // exit the lambda.
        ExprKind::Lambda(lambda) => built_or_text!(pre, lower_lambda(e, lambda, cx)),
        // Defensive: the parser builds `ExprKind::Block` only for a lambda body,
        // which `lower_lambda` emits itself, so neither `Block` arm is reached
        // today. Routed like `if`/`match` so a future value-position block
        // with a `?` would not reintroduce the arrow miscompile.
        ExprKind::Block(b) if block_contains_question(b) => {
            pre.absorb(hoist_value_as_statement(e, cx, |body, cx| {
                block_body_into(body, b, cx, false)
            }))
        }
        ExprKind::Block(b) => built_or_text!(pre, lower_block_as_expr(b, cx)),
        ExprKind::Match { discriminant, arms } if arms_contain_question(arms) => {
            pre.absorb(hoist_value_as_statement(e, cx, |body, cx| {
                match_tail(body, discriminant, arms, cx, false)
            }))
        }
        ExprKind::Match { discriminant, arms } => {
            lowered_or_text!(pre, lower_match_as_iife(discriminant, arms, cx))
        }
        ExprKind::Is { value, pattern } => lowered_or_text!(pre, lower_is(value, pattern, cx)),
        ExprKind::EffectPure(inner) => {
            let inner_expr = pre.lower(inner, cx);
            format!("globalThis.Promise.resolve({inner_expr})")
        }
        ExprKind::RecordSpread {
            type_name: _,
            base,
            overrides,
            ..
        } => lowered_or_text!(pre, lower_record_spread(base, overrides, cx)),
        ExprKind::Expect(inner) => {
            let built = expect_built(inner, &mut pre, cx);
            built_or_text!(pre, built)
        }
        // 1A.17: the test-domain constructs build nodes.
        ExprKind::Val { type_ref, args, .. } => {
            lowered_or_text!(pre, lower_val(type_ref, args, cx))
        }
        ExprKind::Observation(o) => {
            let built = lower_observation(o, cx);
            built_or_text!(pre, built)
        }
        ExprKind::Faults(f) => {
            let built = lower_faults(f, cx);
            built_or_text!(pre, built)
        }
        // `trace(Cap.op)` → the recorded calls mapped to per-call records
        // whose fields are the operation's parameters (positionally).
        ExprKind::Trace { cap, op } => {
            return pre.finish(LoweredExpr::Node(trace_node(cx, &cap.name, &op.name)));
        }
    };
    pre.finish_text(expr)
}

/// v0.9.1: expect as an expression (v0.112, renamed from `assert`).
/// Emit a runtime helper call that returns void (i.e., evaluates to
/// `undefined` at runtime and is treated as the unit value `()` in Bynk
/// terms). The expression form reports the predicate source only — the
/// statement form carries the structural expected-vs-actual report.
fn expect_built(inner: &Expr, pre: &mut Pre, cx: &mut LowerCtx) -> Built {
    let value = pre.lower_node(inner, cx);
    let span_start = inner.span.start;
    let span_end = inner.span.end;
    let location = expect_location_raw(cx, span_start);
    let src = format!("expect {}", expect_source_text_raw(cx, inner.span));
    // The `Paren` keeps today's `((value))` bytes; the `Str` nodes
    // escape exactly as `escape_ts_string` does.
    node_or_text(
        vec![value],
        |c| bynk_ts::TsExpr::Call {
            callee: Box::new(ident_node("__bynkExpect")),
            args: vec![
                bynk_ts::TsExpr::Paren(Box::new(one(c))),
                str_node(location.clone()),
                num_lit(span_start.to_string()),
                num_lit(span_end.to_string()),
                str_node(src.clone()),
            ],
        },
        |c| {
            format!(
                "__bynkExpect(({}), \"{}\", {span_start}, {span_end}, \"{}\")",
                one(c),
                crate::emitter::escape_ts_string(&location),
                crate::emitter::escape_ts_string(&src),
            )
        },
    )
}

/// The parameter names of a capability operation, looked up from the capability
/// declarations in scope (v0.117). Used to destructure a recorded call's
/// arguments for a `with` predicate and to build `trace(Cap.op)` records.
/// P6.29 (design/tracks/the-ir.md §6a): the declaration lookup itself now reads
/// `bynk_model::capability_op_sig_from_commons` — still a by-name walk
/// under the hood (nothing IR-native replaces "find the op named `op` on the
/// capability named `cap`"), but relocated to `ir/lower.rs` (excluded from
/// `ast_importers` — the `Ast → Ir` lowering pass's own job), so this file no
/// longer spells `bynk_syntax::ast::CommonsItem` itself.
fn cap_op_param_names(cx: &LowerCtx, cap: &str, op: &str) -> Vec<String> {
    bynk_model::capability_op_sig_from_commons(cx.commons(), cap, op)
        .map(|sig| sig.params.into_iter().map(|(name, _)| name).collect())
        .unwrap_or_default()
}

/// Lower a fault claim, `<call> faults` (#1706), to a `Bool` JavaScript
/// expression: an async IIFE that awaits the call and yields `true` when it
/// threw. An `ExpectationError` is re-thrown, not counted — a failed `expect`
/// inside the call's own evaluation is the case failing, not the call
/// faulting. Every other throw counts: an injected `stub … fails`, a real
/// provider's failure, an invariant violation. A fault is untyped, so the
/// claim does not discriminate between them.
///
/// The call is lowered as an `<-` subject is, under the claim's own call-site
/// principal (#1812) set exactly as the effect-let arm sets it — the
/// identity the addressed handler reads, or `by Nobody`'s no-credential
/// driver — and never an enclosing one. Whatever the call (or the identity)
/// hoists is placed inside the `try`, not in the enclosing statement's
/// prelude: a fault raised while evaluating the call's arguments is the call
/// faulting too, and must not escape the claim.
/// #2135, #2864: the driver name a reserved bad-credential principal drives:
/// `by Forged` (a wrong secret), `by Stale` (a timestamp past the tolerance)
/// or `by Expired` (a Bearer token whose `exp` has passed).
pub(super) fn bad_signature_driver(actor: &str) -> Option<&'static str> {
    match actor {
        "Forged" => Some("forged"),
        "Stale" => Some("stale"),
        "Expired" => Some("expired"),
        // #3052: an `Oidc` route's token for another issuer or audience.
        "WrongIssuer" => Some("wrongissuer"),
        "WrongAudience" => Some("wrongaudience"),
        // #3225: a token under a refused algorithm, or with no usable `sub`.
        "WrongAlgorithm" => Some("wrongalgorithm"),
        "NoSubject" => Some("nosubject"),
        _ => None,
    }
}

fn lower_faults(faults: &FaultsExpr, cx: &mut LowerCtx) -> Built {
    let FaultsExpr { call, principal } = faults;
    let saved_identity = cx.call_site_identity.take();
    let saved_actor = cx.call_site_actor.take();
    let saved_no_credential = std::mem::replace(&mut cx.call_site_no_credential, false);
    let saved_bad_signature = cx.call_site_bad_signature.take();
    let mut inner = Pre::new();
    if let Some(principal) = principal {
        // `by Nobody … faults` (and #2135 `by Forged`/`by Stale`) is
        // rejected before emit (`bynk.test.faults_needs_in_process`);
        // mirrored here so the claim lowers as the effect-let arm does
        // regardless.
        if principal.actor.name == "Nobody" {
            cx.call_site_no_credential = true;
        } else if let Some(variant) = bad_signature_driver(&principal.actor.name) {
            cx.call_site_bad_signature = Some(variant);
        } else {
            cx.call_site_identity = principal.identity.as_ref().map(|id| inner.lower(id, cx));
            cx.call_site_actor = Some(principal.actor.name.clone());
        }
    }
    let value = inner.lower_node(call, cx);
    cx.call_site_identity = saved_identity;
    cx.call_site_actor = saved_actor;
    cx.call_site_no_credential = saved_no_credential;
    cx.call_site_bad_signature = saved_bad_signature;
    // The literal `await` below is a real source of effectfulness for any
    // synchronous-looking IIFE further out — see `emitted_await`.
    cx.emitted_await = true;
    // 1A.17, Decision F: an async arrow IIFE over a `try`/`catch`, a node only
    // when the call and everything it hoisted are nodes.
    let value = if inner.all_nodes() {
        value
    } else {
        LoweredExpr::Text(value.into_text())
    };
    node_or_text(
        vec![value],
        |c| {
            use bynk_ts::{TsBinaryOp, TsExpr, TsLit, TsStmt};
            let mut try_body = inner.ts_stmts("");
            try_body.push(TsStmt::expr_stmt(
                TsExpr::Await(Box::new(TsExpr::Paren(Box::new(one(c))))),
                None,
            ));
            try_body.push(TsStmt::return_stmt(
                Some(TsExpr::Lit(TsLit::Bool(false))),
                None,
            ));
            let rethrow = TsStmt::if_stmt(
                TsExpr::Binary {
                    op: TsBinaryOp::InstanceOf,
                    left: Box::new(ident_node("__e")),
                    right: Box::new(ident_node("ExpectationError")),
                },
                TsStmt::block(vec![TsStmt::throw_stmt(ident_node("__e"), None)], None),
                None,
            );
            let catch_body = vec![
                rethrow,
                TsStmt::return_stmt(Some(TsExpr::Lit(TsLit::Bool(true))), None),
            ];
            let try_catch = TsStmt::try_catch(
                TsStmt::block(try_body, None),
                Some("__e"),
                TsStmt::block(catch_body, None),
                None,
            );
            TsExpr::Paren(Box::new(arrow_call(
                Vec::new(),
                Vec::new(),
                true,
                vec![try_catch],
            )))
        },
        |c| {
            let hoisted: String = inner.stmts().iter().map(|s| format!("{s} ")).collect();
            let value = one(c);
            format!(
                "(await (async () => {{ try {{ {hoisted}await ({value}); return false; }} catch (__e) {{ if (__e instanceof ExpectationError) {{ throw __e; }} return true; }} }})())"
            )
        },
    )
}

/// `(<obs>.log["Cap.op"] ?? [])`: the calls recorded for one capability
/// operation (v0.117). `obs` is the per-case trace object, always `__obs`.
fn recorded_calls(cx: &LowerCtx, cap: &str, op: &str) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let obs = cx.observation_trace().unwrap_or("__obs");
    TsExpr::Paren(Box::new(TsExpr::Binary {
        op: TsBinaryOp::NullishCoalescing,
        left: Box::new(TsExpr::Index {
            object: Box::new(member_node(obs, "log")),
            index: Box::new(str_node(format!("{cap}.{op}"))),
        }),
        right: Box::new(TsExpr::array(Vec::new())),
    }))
}

/// `{ args: unknown[] }`, the recorded call's shape as far as `.args` is read
/// positionally (P7.2).
fn call_args_type() -> bynk_ts::TsType {
    use bynk_ts::{TsType, TsTypeMember};
    TsType::Object(vec![TsTypeMember::prop(
        "args",
        TsType::array(TsType::named("unknown")),
    )])
}

/// `{ n: __c.args[0], … }`: the operation's parameters read positionally off a
/// recorded call.
fn call_fields(names: &[String]) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::object(
        names
            .iter()
            .enumerate()
            .map(|(i, n)| {
                (
                    n.clone(),
                    bynk_ts::TsExpr::Index {
                        object: Box::new(member_node("__c", "args")),
                        index: Box::new(num_lit(i.to_string())),
                    },
                )
            })
            .collect(),
    )
}

/// `trace(Cap.op)` (v0.117): the recorded calls mapped to per-call records.
fn trace_node(cx: &LowerCtx, cap: &str, op: &str) -> bynk_ts::TsExpr {
    use bynk_ts::{TsArrowBody, TsExpr, TsParam, TsType};
    let names = cap_op_param_names(cx, cap, op);
    // P7.2: only `.args` is read below (positionally), so `unknown[]` is
    // what's actually known about a recorded call's arguments here — but
    // the mapped-out `{ msg: unknown, … }` shape then flows into whatever
    // this `trace()` result feeds (a test's own `.filter`/projection,
    // typed against the checker's own per-op record type), which needs
    // the real field types, not `unknown`. Asserted into
    // `observation_call_record_types`' own generated alias (same name
    // `checker::call_record_type_name` derives, same test module, so no
    // qualification needed) — legal under `tsc --strict` because every
    // field of that alias is structurally assignable to `unknown`.
    let record_ty = bynk_check::checker::call_record_type_name(cap, op);
    let map = TsExpr::Call {
        callee: Box::new(TsExpr::Member {
            object: Box::new(recorded_calls(cx, cap, op)),
            property: "map".to_string(),
        }),
        args: vec![TsExpr::Arrow {
            params: vec![TsParam {
                name: "__c".to_string(),
                ty: Some(call_args_type()),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(call_fields(&names)))),
        }],
    };
    TsExpr::Paren(Box::new(TsExpr::As {
        expr: Box::new(TsExpr::Paren(Box::new(map))),
        ty: TsType::array(TsType::named(record_ty)),
    }))
}

/// Lower an observation (v0.117) to a `Bool` JavaScript expression over the
/// recorded trace object. `total`/`matching` counts drive the sugar; `before`
/// compares call order.
fn lower_observation(o: &ObservationExpr, cx: &mut LowerCtx) -> Built {
    use bynk_ts::{TsBinaryOp, TsExpr};
    let calls = recorded_calls(cx, &o.cap.name, &o.op.name);
    let length = |calls: &TsExpr| TsExpr::Member {
        object: Box::new(calls.clone()),
        property: "length".to_string(),
    };
    let binary = |op: TsBinaryOp, left: TsExpr, right: TsExpr| TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    };
    let paren = |e: TsExpr| TsExpr::Paren(Box::new(e));
    // The text twin of `recorded_calls`, for the fallbacks below.
    let calls_text = bynk_ts::print_expr(&calls);
    match &o.matcher {
        ObservationMatcher::NeverCalled => Ok(paren(binary(
            TsBinaryOp::StrictEq,
            length(&calls),
            num_lit("0".to_string()),
        ))),
        ObservationMatcher::Before { cap, op } => {
            let calls2 = recorded_calls(cx, &cap.name, &op.name);
            let order = |calls: &TsExpr| TsExpr::Member {
                object: Box::new(TsExpr::Index {
                    object: Box::new(calls.clone()),
                    index: Box::new(num_lit("0".to_string())),
                }),
                property: "order".to_string(),
            };
            let zero = || num_lit("0".to_string());
            let both = binary(
                TsBinaryOp::And,
                binary(TsBinaryOp::GreaterThan, length(&calls), zero()),
                binary(TsBinaryOp::GreaterThan, length(&calls2), zero()),
            );
            Ok(paren(binary(
                TsBinaryOp::And,
                both,
                binary(TsBinaryOp::LessThan, order(&calls), order(&calls2)),
            )))
        }
        ObservationMatcher::Called { count, with_pred } => match with_pred {
            None => match count {
                None => Ok(paren(binary(
                    TsBinaryOp::GreaterThanEq,
                    length(&calls),
                    num_lit("1".to_string()),
                ))),
                Some(c) => {
                    let n = lower_expr(c, cx).expr;
                    node_or_text(
                        vec![n],
                        |c| paren(binary(TsBinaryOp::StrictEq, length(&calls), paren(one(c)))),
                        |c| format!("({calls_text}.length === ({}))", one(c)),
                    )
                }
            },
            Some(p) => {
                let names = cap_op_param_names(cx, &o.cap.name, &o.op.name);
                let record_ty = bynk_check::checker::call_record_type_name(&o.cap.name, &o.op.name);
                let pred_lowered = lower_expr(p, cx);
                // The predicate's hoisted statements go in the arrow body: they
                // are nodes only if every one is.
                let pre_nodes = pred_lowered.pre_all_nodes();
                let pre_stmts: Vec<bynk_ts::TsStmt> = pred_lowered
                    .pre
                    .iter()
                    .filter_map(|s| match s {
                        LoweredStmt::Node(n) => Some((**n).clone()),
                        LoweredStmt::Text(_) => None,
                    })
                    .collect();
                let pre_src = pred_lowered.pre_texts().join(" ");
                let pred = if pre_nodes {
                    pred_lowered.expr
                } else {
                    LoweredExpr::Text(pred_lowered.expr.into_text())
                };
                let mut children = vec![pred];
                if let Some(c) = count {
                    children.push(lower_expr(c, cx).expr);
                }
                let has_count = count.is_some();
                node_or_text(
                    children,
                    |mut c| {
                        let n = has_count.then(|| c.pop()).flatten();
                        let pred = one(c);
                        let mut stmts = Vec::new();
                        // #291: typed through the operation's call-record alias (as
                        // `trace` is), so a predicate may read a record parameter's
                        // fields (`with req.url == …`) — `__c.args` is `unknown[]`.
                        if !names.is_empty() {
                            stmts.push(bynk_ts::TsStmt::const_stmt(
                                bynk_ts::TsBindingName::ObjectPattern(names.clone()),
                                None,
                                TsExpr::As {
                                    expr: Box::new(call_fields(&names)),
                                    ty: bynk_ts::TsType::named(record_ty.clone()),
                                },
                                None,
                            ));
                        }
                        stmts.extend(pre_stmts);
                        stmts.push(bynk_ts::TsStmt::return_stmt(Some(paren(pred)), None));
                        // P7.2: same shape as `trace` — only `.args` is destructured,
                        // so `unknown[]` is what's actually known.
                        let matching = TsExpr::Member {
                            object: Box::new(TsExpr::Call {
                                callee: Box::new(TsExpr::Member {
                                    object: Box::new(calls.clone()),
                                    property: "filter".to_string(),
                                }),
                                args: vec![TsExpr::Arrow {
                                    params: vec![bynk_ts::TsParam {
                                        name: "__c".to_string(),
                                        ty: Some(call_args_type()),
                                        optional: false,
                                    }],
                                    is_async: false,
                                    generics: Vec::new(),
                                    return_type: None,
                                    body: Box::new(bynk_ts::TsArrowBody::Multiline(stmts)),
                                }],
                            }),
                            property: "length".to_string(),
                        };
                        match n {
                            None => paren(binary(
                                TsBinaryOp::GreaterThanEq,
                                paren(matching),
                                num_lit("1".to_string()),
                            )),
                            Some(n) => {
                                paren(binary(TsBinaryOp::StrictEq, paren(matching), paren(n)))
                            }
                        }
                    },
                    |mut c| {
                        let n = has_count.then(|| c.pop()).flatten();
                        let pred = one(c);
                        let destructure = if names.is_empty() {
                            String::new()
                        } else {
                            let fields = names
                                .iter()
                                .enumerate()
                                .map(|(i, n)| format!("{n}: __c.args[{i}]"))
                                .collect::<Vec<_>>()
                                .join(", ");
                            format!(
                                "const {{ {} }} = {{ {fields} }} as {record_ty}; ",
                                names.join(", ")
                            )
                        };
                        let matching = format!(
                            "{calls_text}.filter((__c: {{ args: unknown[] }}) => {{ {destructure}{pre_src}return ({pred}); }}).length"
                        );
                        match n {
                            None => format!("(({matching}) >= 1)"),
                            Some(n) => format!("(({matching}) === ({n}))"),
                        }
                    },
                )
            }
        },
    }
}

/// A default base-type literal (as TypeScript source) that satisfies a refined
/// type's predicates, for bare `Val[T]`. `None` when no default can be derived
/// (a `Matches` refinement — the checker rejects bare `Val` for those).
fn refined_default(decl: &TypeDecl) -> Option<bynk_ts::TsExpr> {
    match &decl.body {
        TypeBody::Refined {
            base, refinement, ..
        } => refinement_default(*base, refinement.as_ref()),
        _ => None,
    }
}

/// [`refined_default`] over a base type and its refinement, wherever it is
/// written: a refined type's declaration, or a record field's inline `where`
/// (#2658: `qty: Int where Positive` must fabricate `1`, not `0`).
fn refinement_default(base: BaseType, refinement: Option<&Refinement>) -> Option<bynk_ts::TsExpr> {
    // #2686: an `@example` is the value for a refinement `Val` cannot generate
    // (`Matches`, `IsoDate`); the checker proved it satisfies the refinement.
    if let Some(lit) = refinement
        .and_then(bynk_check::checker::refinement_example)
        .and_then(lower_const_literal_node)
    {
        return Some(lit);
    }
    match base {
        BaseType::Int => {
            let mut lo: i64 = 0;
            let mut hi = i64::MAX;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1),
                        PredKind::NonNegative => lo = lo.max(0),
                        PredKind::InRange(a, b) => {
                            lo = lo.max(a.value);
                            hi = hi.min(b.value);
                        }
                        _ => {}
                    }
                }
            }
            // The floor of 0 can overshoot a wholly negative range
            // (`InRange(-5, -1)`); fall back to the upper bound. An empty
            // range never reaches here: the checker rejects it.
            if lo > hi {
                lo = hi;
            }
            Some(num_lit(lo.to_string()))
        }
        BaseType::String => {
            let mut len: i64 = 0;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match p.kind {
                        PredKind::NonEmpty => len = len.max(1),
                        PredKind::MinLength(k) | PredKind::Length(k) => len = len.max(k),
                        PredKind::Matches(_) | PredKind::IsoDate => return None,
                        _ => {}
                    }
                }
            }
            if len < 1 {
                len = 1;
            }
            Some(str_node("x".repeat(len as usize)))
        }
        BaseType::Bool => Some(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true))),
        BaseType::Float => {
            let mut lo: f64 = 0.0;
            let mut hi = f64::INFINITY;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1.0),
                        PredKind::NonNegative => lo = lo.max(0.0),
                        PredKind::InRangeF(a, b) => {
                            lo = lo.max(a.value);
                            hi = hi.min(b.value);
                        }
                        _ => {}
                    }
                }
            }
            // The `Positive` floor of 1.0 can overshoot a tight fractional
            // range (`InRange(0.0, 0.5)`); fall back to the upper bound.
            if lo > hi {
                lo = hi;
            }
            Some(num_lit(lo.to_string()))
        }
        // v0.86: `Duration` carries no refinement; `0` millis is its default.
        BaseType::Duration | BaseType::Instant => Some(num_lit("0".to_string())),
        // v0.110: `Bytes` carries no refinement; the empty octet sequence is
        // its default.
        BaseType::Bytes => Some(empty_bytes_node()),
        // #2622: the smallest value the refinement admits, at scale 2.
        BaseType::Decimal => {
            let mut lo: i128 = 0;
            let mut hi: Option<i128> = None;
            let mut scale = 2u32;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    if let PredKind::InRangeD(a, b) = &p.kind {
                        scale = scale.max(a.scale).max(b.scale);
                    }
                }
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1),
                        PredKind::InRangeD(a, b) => {
                            match scaled_coef(&a.coef, a.scale, scale) {
                                Some(v) => lo = lo.max(v),
                                // The bound does not fit once lifted to the
                                // common scale: the bound itself, at its own
                                // scale, is the smallest value in range.
                                None => {
                                    return Some(decimal_lit(&a.coef, a.scale));
                                }
                            }
                            if let Some(v) = scaled_coef(&b.coef, b.scale, scale) {
                                hi = Some(hi.map_or(v, |h| h.min(v)));
                            }
                        }
                        _ => {}
                    }
                }
            }
            // As for `Float`: a floor above the ceiling falls back to the ceiling.
            if let Some(h) = hi
                && lo > h
            {
                lo = h;
            }
            Some(decimal_lit(&lo.to_string(), scale))
        }
    }
}

/// `coef` (a signed digit string at `from` scale) raised to scale `to`, as an
/// `i128`, or `None` when it does not fit.
fn scaled_coef(coef: &str, from: u32, to: u32) -> Option<i128> {
    let v: i128 = coef.parse().ok()?;
    v.checked_mul(10i128.checked_pow(to.checked_sub(from)?)?)
}

/// v0.9.4 Part B (slice 1): lower a refined-type `Val[T]` / `Val[T](lit)` to
/// the branded `unsafe` constructor. The checker has already validated this is a
/// refined type in a test body, and recorded the refined type at `span`.
/// v0.9.4 slice 2 recursion cap for bare `Val` generation (mirrors the
/// checker's `MOCK_DEPTH`).
const MOCK_DEPTH: u32 = 12;

/// `new globalThis.Uint8Array()`: the empty octet sequence, `Bytes`'s default.
fn empty_bytes_node() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::New {
        callee: Box::new(member_node("globalThis", "Uint8Array")),
        args: Vec::new(),
    }
}

/// A base-literal default for an opaque type's underlying base, as a node. Not
/// distinct per call in this increment — per-call distinctness via a runtime
/// counter is a follow-up.
fn base_default_ts(base: BaseType) -> bynk_ts::TsExpr {
    match base {
        BaseType::Int => num_lit("0".to_string()),
        BaseType::String => str_node("mock"),
        BaseType::Bool => bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
        BaseType::Float => num_lit("0".to_string()),
        BaseType::Duration | BaseType::Instant => num_lit("0".to_string()),
        BaseType::Bytes => empty_bytes_node(),
        BaseType::Decimal => decimal_lit("0", 0),
    }
}

/// Generate a TypeScript expression for a bare `Val` of `ty` (v0.9.4 Part B,
/// slice 2), as a node (1A.17, Decision D). Recurses through sum payloads and
/// record fields; refined types use `refined_default`, opaque types wrap a base
/// default, bare bases use 0/""/true.
fn mock_value(ty: TyId, cx: &LowerCtx, depth: u32) -> bynk_ts::TsExpr {
    let undefined = || ident_node("undefined");
    let tys = cx.ty_render();
    if depth == 0 {
        return undefined();
    }
    match &*tys.get(ty) {
        Ty::Base(BaseType::Int) => num_lit("0".to_string()),
        Ty::Base(BaseType::String) => str_node(""),
        Ty::Base(BaseType::Bool) => bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
        Ty::Base(BaseType::Float) => num_lit("0".to_string()),
        Ty::Base(BaseType::Decimal) => decimal_lit("0", 0),
        Ty::Named { name, .. } => {
            let Some(decl) = cx.commons().types.get(name.as_str()) else {
                return undefined();
            };
            match &decl.body {
                TypeBody::Refined { .. } => {
                    let d = refined_default(decl).unwrap_or_else(|| num_lit("0".to_string()));
                    unchecked_construct_test_node(name, d, false)
                }
                TypeBody::Opaque { base, .. } => {
                    unchecked_construct_test_node(name, base_default_ts(*base), true)
                }
                TypeBody::Sum(s) => match s.variants.first() {
                    None => undefined(),
                    Some(v) if v.payload.is_empty() => member_node(name, &v.name.name),
                    Some(v) => {
                        let args = v
                            .payload
                            .iter()
                            .map(|f| {
                                bynk_check::checker::resolve_type_ref(
                                    &f.type_ref,
                                    &cx.commons().types,
                                    tys.types(),
                                )
                                .map(|t| mock_value(t, cx, depth - 1))
                                .unwrap_or_else(undefined)
                            })
                            .collect();
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(member_node(name, &v.name.name)),
                            args,
                        }
                    }
                },
                TypeBody::Record(r) => bynk_ts::TsExpr::object(
                    r.fields
                        .iter()
                        .map(|f| {
                            // An inline `where` on a base-typed field narrows
                            // the base's default, as a declared refined type's
                            // does; the field stays a plain base value.
                            let inline = match (&f.type_ref, &f.refinement) {
                                (TypeRef::Base(base, _), Some(r)) => {
                                    refinement_default(*base, Some(r))
                                }
                                _ => None,
                            };
                            let fv = inline.unwrap_or_else(|| {
                                bynk_check::checker::resolve_type_ref(
                                    &f.type_ref,
                                    &cx.commons().types,
                                    tys.types(),
                                )
                                .map(|t| mock_value(t, cx, depth - 1))
                                .unwrap_or_else(undefined)
                            });
                            (f.name.name.clone(), fv)
                        })
                        .collect(),
                ),
            }
        }
        _ => undefined(),
    }
}

/// Lower an `ExprKind::MethodCall`. This is a dispatcher: a sequence of
/// independent guard-and-`return` branches, tried in order (the order is
/// load-bearing — earlier guards take precedence), falling through to the
/// UFCS instance-call tail. The collection/numeric/string/option/result
/// kernels and the typed JSON codec delegate to dedicated helpers that
/// return `Option<String>`.
/// #926: render a capability call's explicit type argument(s) as a TS
/// generic argument list (`<T>`), or `""` when absent. Unlike
/// `Json.decode[T]` (`lower_json_codec_call`, below), which specialises a
/// runtime codec per call and needs no TS-level generic at all, a capability
/// operation's `T` is a pure type-level parameter on a real generic TS
/// interface method (Decision C) — with no argument of type `T`, TypeScript
/// cannot infer a return-position-only parameter, so the call site must
/// name it explicitly.
fn capability_call_type_args_ts(
    type_args: &[TypeRef],
    wire: Option<&bynk_check::checker::codable::CapEncoding>,
) -> String {
    // Constrained generics slice 3: a type parameter that only occurs in encoded
    // (`unknown`) arguments is not in the emitted interface, so a call omits it.
    let kept: Vec<&TypeRef> = match wire {
        Some(w) => type_args
            .iter()
            .zip(&w.keep_type_args)
            .filter_map(|(t, keep)| keep.then_some(t))
            .collect(),
        None => type_args.iter().collect(),
    };
    if kept.is_empty() {
        return String::new();
    }
    format!(
        "<{}>",
        kept.into_iter()
            .map(ts_type_ref)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Constrained generics slice 3: how the capability call `e` encodes its
/// `Codable` arguments, when it has any.
fn capability_wire(
    e: &Expr,
    cx: &LowerCtx,
) -> Option<std::sync::Arc<bynk_check::checker::codable::CapEncoding>> {
    match cx.commons().callee(e.id) {
        Some(Callee::Capability { wire, .. } | Callee::CrossCap { wire, .. }) => wire.clone(),
        _ => None,
    }
}

/// Constrained generics slice 3 (DECISION D): encode each argument whose
/// parameter mentions a `Codable` type parameter, whole, with the codec of its
/// instantiated type. The provider receives plain JSON and no witness.
fn encode_capability_args(
    wire: Option<&bynk_check::checker::codable::CapEncoding>,
    args: &mut [String],
    cx: &LowerCtx,
) {
    let Some(wire) = wire else {
        return;
    };
    for (arg, ty) in args.iter_mut().zip(&wire.encode) {
        if let Some(ty) = ty {
            *arg = cx.encode_call(*ty, arg);
        }
    }
}

/// Typed capability reads (#3458): a call to an operation whose `Effect[P]`
/// payload mentions a `Codable` parameter gets the provider's plain-JSON reply
/// decoded as the instantiated `P`, whole, with the codec of `P`. What a reply
/// that does not decode means is the operation's declared policy (a fault, or a
/// miss for an `Option` payload); the runtime helper applies it. Other calls
/// are returned as they are.
fn decode_capability_reply(
    call: String,
    wire: Option<&bynk_check::checker::codable::CapEncoding>,
    cap: &str,
    op: &str,
    cx: &LowerCtx,
) -> String {
    use bynk_check::checker::codable::ReplyPolicy;
    let Some(reply) = wire.and_then(|w| w.reply.as_ref()) else {
        return call;
    };
    cx.runtime_use().note_cmp("__bynkCapabilityReply");
    let policy = match reply.policy {
        ReplyPolicy::Fault => "fault",
        ReplyPolicy::Miss => "miss",
    };
    format!(
        "__bynkCapabilityReply({call}, {}, {{ capability: \"{}\", operation: \"{}\", type: \"{}\" }}, \"{policy}\")",
        // 1A: witness printed for a text caller
        bynk_ts::print_expr(&cx.codec(reply.payload)),
        escape_ts_string(cap),
        escape_ts_string(op),
        escape_ts_string(&reply.payload.display(cx.commons().tys())),
    )
}

/// #934 (call-site key scoping): `Idempotency.dedup[T](key)` and
/// `Idempotency.remember[T](key, value, expiresAfter)` and
/// `Idempotency.claim(key, expiresAfter)` all take the
/// developer-supplied dedup key as their first argument (already lowered, in
/// `args_lowered[0]`). Rewrite it in place to a template literal prefixed with
/// the calling handler's own qualified name, so two unrelated handlers that
/// happen to pick the same literal key never collide.
///
/// `is_first_party` must already have confirmed this is genuinely the
/// first-party `bynk.Idempotency` capability, not merely a same-named
/// capability some other adapter or context happens to declare (both call
/// sites resolve that separately, since the two lowering paths — flattened
/// vs. cross-context-qualified — carry the provenance differently).
///
/// `cx.handler_scope` is set at every real capability-call site (an ordinary
/// service handler, an agent handler, a composed provider op body, a
/// websocket lifecycle DO method — see `emit_service`/`emit_agent`/
/// `emit_provider`/`emit_ws_do_method`). Its absence here means some new
/// capability-call site was added without threading it through; that is a
/// compiler bug, not a user error, so this fails loudly rather than silently
/// shipping an unscoped key.
fn scope_idempotency_key(
    is_first_party: bool,
    method: &str,
    args_lowered: &mut [String],
    cx: &LowerCtx,
) {
    if !is_first_party || !matches!(method, "dedup" | "remember" | "claim") {
        return;
    }
    let scope = cx
        .handler_scope()
        .unwrap_or_else(|| panic!("Idempotency.{method} lowered with no handler_scope set"));
    args_lowered[0] = format!("`{scope}::${{{}}}`", args_lowered[0]);
}

/// W30 (#2840): lower a first-party `Idempotency.claim(key, expiresAfter)` made
/// in a service handler so the claim is recorded in the handler's claim scope
/// (`__claims`, declared by [`CLAIM_SCOPE_DECLARATION`]). The scope decides how long
/// the claim is first taken for (`lease`: a short lease when the provider will
/// settle it, the full `expiresAfter` when it won't); `note` remembers the key
/// when it was `Claimed`, and the scope confirms it for the full `expiresAfter`
/// or releases it when the handler exits. The call itself still goes through
/// `<deps>.Idempotency.claim`, so a `stub` of it applies.
fn tracked_claim_call(cap_expr: &str, args: &[String], cx: &mut LowerCtx) -> String {
    cx.claim_scope_used = true;
    // The bare `await` below makes any enclosing thunk or IIFE effectful —
    // see `emitted_await` (a `par` part that is a claim call must be `async`).
    cx.emitted_await = true;
    format!(
        "(await (async (__k: string, __t: number) => __claims.note(__k, await {cap_expr}.claim(__k, __claims.lease(__t)), __t))({}, {}))",
        args[0], args[1]
    )
}

/// W30 (#2840): the statement that opens a service handler's claim scope, which
/// wraps the already-lowered function body (`lower_stmt::claim_scoped`) so the
/// claims it made are settled when it exits. `__claims` is the provider's own
/// scope (`Idempotency.__track()`, hidden: not in the capability), or a no-op
/// scope for a provider without one, which takes the claim for its full
/// window and never settles it. A thrown fault releases the claims; a
/// returned `Err` or a 5xx `HttpResult` does too; any other outcome confirms
/// them.
pub(crate) const CLAIM_SCOPE_DECLARATION: &str = "const __claims = (deps.Idempotency as unknown as { __track?: () => { note<C>(k: string, c: C, t: number): C; lease(t: number): number; settle(r: unknown, threw: boolean): globalThis.Promise<void> } }).__track?.() ?? { note: <C>(_k: string, c: C, _t: number) => c, lease: (t: number) => t, settle: async (_r: unknown, _threw: boolean) => {} };";

fn lower_method_call(e: &Expr, cx: &mut LowerCtx) -> Lowered {
    let ExprKind::MethodCall {
        receiver,
        method,
        type_args,
        args,
        ..
    } = &e.kind
    else {
        unreachable!("lower_method_call is called on a method call")
    };
    let mut pre = Pre::new();
    // 1A: the domain-specific dispatchers below (store, scalar/collection
    // statics, runtime values, D1, cross-context, events, cookies,
    // capabilities, type statics, test and system services, agent calls) stay
    // text, each its own family with its own runtime contract. Of the kernel
    // methods only the `List` kernel builds nodes (1A.18a and 1A.18b); the plain instance
    // call that ends this function is a node.
    // Persisted-store receivers: held Map, Map, Set, Cache, Log, Cell (#3167).
    if let Some(s) = lower_store_method_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    // Intrinsic statics: scalar and collection constructors, then runtime values (#3167).
    if let Some(s) = lower_scalar_static_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_runtime_value_static_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    // Capability, cross-context, events and static-type calls (#3167).
    if let Some(s) = lower_d1_peek_call(e, cx) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_d1_select_call(e, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_cross_context_call(e, receiver, method, type_args, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_events_emit_call(e, receiver, type_args, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_signed_cookie_call(e, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_cookie_field_call(e, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_capability_call(e, receiver, method, type_args, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_type_static_call(e, receiver, method, type_args, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    // Test-case and system-tier service addresses, then agent invocations (#3167).
    if let Some(s) = lower_test_asset_fetch_call(e, receiver, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_system_schedule_call(receiver, method, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_system_http_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_test_service_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_inline_agent_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    if let Some(s) = lower_let_bound_agent_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish_text(s);
    }
    // v0.20b: built-in kernel methods, resolved by the checker (#3164).
    if let Some(built) = lower_kernel_method_call(e, receiver, method, args, cx, &mut pre) {
        return pre.finish_built(built);
    }
    // Instance call: UFCS lowering with the receiver as first arg.
    let built = lower_ufcs_method_call(e, receiver, method, type_args, args, cx, &mut pre);
    pre.finish_built(built)
}

/// #3147: lower `D1.peek(<participant>.<Cap>)` in an integration or `system`
/// case, or `None` when the call is not one. It holds the participant's own
/// capability over its test database, read-only, as `deps.<local>`: the same
/// `WorkersD1` the app's code calls, over the very database object the
/// participant's `env` holds, so a call on the handle (an ordinary capability
/// call on `deps.<local>`) sees what the app wrote.
fn lower_d1_peek_call(e: &Expr, cx: &mut LowerCtx) -> Option<String> {
    let Some(Callee::D1Peek {
        participant,
        binding,
        local,
    }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let ns = test_scaffold_ns(&participant);
    Some(format!(
        "(deps.{local} = new bynk_cloudflare__binding.WorkersD1({{ {binding}: deps.env_{ns}.{binding}.readOnly() }}, \"{binding}\"))"
    ))
}

/// Lower a derived-`D1` `select`/`firstAs` call (#2865), or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_d1_select_call(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // #2865: `Cap.select[T](tail, params)` / `Cap.firstAs[T](tail, params)` on
    // a derived `D1`. The checker wrote the statement (`SELECT` + the column
    // list from `T` + the literal tail); this runs it through `all`/`first`
    // and decodes with the `decodeSql` codec, so no runtime op is involved.
    // Keyed on the recorded callee, not the method name. #2957: the error
    // tags are `as const` — `Err<E>` infers `E` from the literal, and a bare
    // `"Sql"` widens to `string`, so a match on `Err(Sql(e))` would not narrow
    // and `e` would reach tsc as `DecodeError | SqlError`.
    if let Some(Callee::D1Select {
        cap,
        row,
        sql,
        first,
        pg,
        mysql,
    }) = cx.commons().callees.get(&e.id).cloned()
        && let Some(params) = args.get(1)
    {
        let tys = cx.ty_render();
        let target = if first {
            row
        } else {
            tys.intern(Ty::List(row))
        };
        // Hyperdrive H3 (#3320): a `Postgres` read decodes Postgres' text
        // forms and fails as a `PgReadError`, whose variants are `PgSql` and
        // `PgDecode` (`Sql` and `Decode` are `SqlReadError`'s).
        // Hyperdrive H5 (#3322): a `MySql` read decodes the same text forms for
        // `Int` (a `BIGINT` arrives as text) and fails as a `MySqlReadError`
        // (`MySqlSql`, `MySqlDecode`).
        let source = if pg || mysql {
            DecodeSource::Pg
        } else {
            DecodeSource::Sql
        };
        let (sql_tag, decode_tag) = if pg {
            ("PgSql", "PgDecode")
        } else if mysql {
            ("MySqlSql", "MySqlDecode")
        } else {
            ("Sql", "Decode")
        };
        let decoder = json_decoder_fn(target, source, cx)
            .expect("a checked D1 select row type has a JSON decoder");
        {
            let params = pre.lower(params, cx);
            let deps =
                if cx.in_test_case() || cap.starts_with(bynk_check::checker::peek::LOCAL_PREFIX) {
                    "deps".to_string()
                } else {
                    cx.cap_deps_expr().to_string()
                };
            let sql = escape_ts_string(&sql);
            let (op, decode) = if first {
                (
                    "first",
                    format!(
                        "if (__r.value.tag === \"None\") return Ok(None); \
                         const __d = __decode(__r.value.value); \
                         return __d.tag === \"Ok\" ? Ok(Some(__d.value)) : Err({{ tag: \"{decode_tag}\" as const, error: __d.error }});"
                    ),
                )
            } else {
                (
                    "all",
                    format!(
                        "const __d = __decode(__r.value); \
                         return __d.tag === \"Ok\" ? Ok(__d.value) : Err({{ tag: \"{decode_tag}\" as const, error: __d.error }});"
                    ),
                )
            };
            return Some(format!(
                "(async () => {{ const __decode = {decoder}; \
                 const __r = await {deps}.{cap}.{op}(\"{sql}\", {params}); \
                 if (__r.tag === \"Err\") return Err({{ tag: \"{sql_tag}\" as const, error: __r.error }}); \
                 {decode} }})()"
            ));
        }
    }
    None
}

/// #3386: lower `Actor.issue(identity)` and `Actor.expire()` on a local `SignedCookie` actor, or
/// `None` when the call is not one. `issue` is an effect: it signs the identity into a session
/// cookie with the actor's keys (from `deps.__session`, supplied by the Worker's compose from the
/// environment) at the `Clock`'s time; `expire` is the pure matching deletion.
fn lower_signed_cookie_call(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let Some(Callee::SignedCookie { actor, issue }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let seam = cx.commons().commons.items.iter().find_map(|i| match i {
        CommonsItem::Actor(a) if a.name.name == actor => {
            bynk_check::actors::signed_cookie_seam_of(a, None)
        }
        _ => None,
    })?;
    cx.note_session_cookie();
    let name = escape_ts_string(&seam.cookie);
    let same_site = escape_ts_string(&seam.same_site);
    if !issue {
        return Some(format!(
            "__expireSessionCookie(\"{name}\", \"{same_site}\")"
        ));
    }
    let id = pre.lower(args.first()?, cx);
    let deps = if cx.in_test_case() {
        "deps".to_string()
    } else {
        cx.cap_deps_expr().to_string()
    };
    let actor = escape_ts_string(&actor);
    Some(format!(
        "(async () => __issueSessionCookie(__sessionKeysOf({deps}, \"{actor}\"), \"{actor}\", \"{name}\", \"{same_site}\", {id}, {}, await {deps}.Clock.now()))()",
        seam.ttl_secs
    ))
}

/// #3446: lower `Record.field.cookie(value)` and `Record.field.expire()` to the `Cookie` record
/// literal an author would have written by hand: the field's wire name (its `@name`, else its own
/// name, the key the decoder reads) and the value as the decoder reads it. The leaf table is the
/// inverse of `fieldsObject`'s (an `Int` its digits, a `Bool` `true`/`false`, an enum its variant's
/// name, everything else its text), keyed on the same `query_leaf_kind` the decoder's field spec
/// uses, so the two cannot drift. `expire` is the deletion `clearCookie` sends: empty, `Max-Age=0`,
/// epoch `Expires`. Every other attribute is the `Cookie` default, applied by the router.
fn lower_cookie_field_call(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let Some(Callee::CookieField {
        record,
        field,
        expire,
    }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let types = &cx.commons().types;
    let TypeBody::Record(body) = &types.get(&record)?.body else {
        return None;
    };
    let f = body.fields.iter().find(|f| f.name.name == field)?;
    let name = escape_ts_string(&f.wire_name().unwrap_or_else(|| f.name.name.clone()));
    if expire {
        return Some(typed_cookie_literal(&name, "\"\"", "Some(0)", "Some(0)"));
    }
    let leaf = match &f.type_ref {
        TypeRef::Option(inner, _) => inner.as_ref(),
        other => other,
    };
    let kind = crate::emitter::workers_entry::query_leaf_kind(leaf, types, false);
    let mut value = pre.lower(args.first()?, cx);
    if kind == "decimal" {
        // A `Decimal` is its rendered text on the wire; the runtime helper takes it as such.
        cx.note_decimal("__bynkDecimalToString");
        value = format!("__bynkDecimalToString({value})");
    }
    // The helper rides the cookie-helper import group (`SESSION_COOKIE_RUNTIME_IMPORTS`).
    cx.note_session_cookie();
    let text = format!("__cookieValue(\"{kind}\", {value})");
    Some(typed_cookie_literal(
        &name,
        &text,
        "{ tag: \"None\" as const }",
        "{ tag: \"None\" as const }",
    ))
}

/// #3446: the `Cookie` record literal of a typed write, with every attribute the record's
/// defaults give (the router and `serialiseCookie` read them all: they apply no defaults of their
/// own), as a hand-written `Cookie { name, value }` lowers.
fn typed_cookie_literal(name: &str, value: &str, max_age: &str, expires: &str) -> String {
    format!(
        "({{ name: \"{name}\", value: {value}, maxAge: {max_age}, expires: {expires}, \
         domain: {{ tag: \"None\" as const }}, path: \"/\", secure: true, httpOnly: true, \
         sameSite: {{ tag: \"Lax\" as const }}, partitioned: false }})"
    )
}

/// Lower a cross-context capability call (`B.Cap.op(…)`) or service call, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_cross_context_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.15 cross-context capability call: `B.Cap.op(args)` /
    // `Alias.Cap.op(args)`. The provider is instantiated locally in
    // the composition root, so this lowers to an in-process
    // `<deps>.<Cap>.op(args)` exactly like a local capability call —
    // the consumed-context prefix is resolved away.
    if let Some(chain) = flatten_emit_ident_chain(receiver)
        && let Some((consumed, cap)) = cx.cross_context().resolve_cross_capability(&chain)
    {
        let mut args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        scope_idempotency_key(
            cap == "Idempotency" && consumed == "bynk",
            &method.name,
            &mut args_lowered,
            cx,
        );
        if cap == "Idempotency"
            && consumed == "bynk"
            && method.name == "claim"
            && cx.tracks_claims()
        {
            let cap_expr = format!("{}.{}", cx.cap_deps_expr(), cap);
            return Some(tracked_claim_call(&cap_expr, &args_lowered, cx));
        }
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = format!(
            "{}.{}.{}{}({})",
            cx.cap_deps_expr(),
            cap,
            method.name,
            capability_call_type_args_ts(type_args, wire.as_deref()),
            args_lowered.join(", ")
        );
        return Some(decode_capability_reply(
            call,
            wire.as_deref(),
            &cap,
            &method.name,
            cx,
        ));
    }
    // v0.6 cross-context service call: receiver is an alias or the
    // dotted name of a consumed context. #1961: unless the checker resolved
    // this call to something else — a local shadowing the alias
    // (`1203_cross_context_call_shadowed_by_local`) is a UFCS call, and
    // lowering it over the wire named a Service Binding the Worker, which
    // binds only what it calls, does not declare.
    if !matches!(cx.commons().callee(e.id), Some(c) if !matches!(c, Callee::Cross { .. }))
        && let Some(s) =
            pre.absorb_opt(lower_cross_context_service_call(receiver, method, args, cx))
    {
        return Some(s);
    }
    None
}

/// Lower `Events.emit[E](event)`, buffered into the handler's `__events` until commit, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_events_emit_call(
    e: &Expr,
    receiver: &Expr,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // Events track, slice 0 (spine #936): `Events.emit[E](event)` never
    // calls through a provider like an ordinary capability op — release-
    // at-commit (the retired events track's §3.0, `design/archive/retired-tracks.md`) buffers it into the handler's own
    // `__events` local instead, flushed only if the handler completes
    // without throwing. The local is declared by the outer body wrapper
    // whenever `block_uses_emit` is true (the same syntactic check this
    // receiver/method match mirrors), so the two stay in lockstep by
    // construction. The flush calls `deps.__eventsDispatch`, which the
    // publishing context's own compose wires to the Cloudflare fan-out DO
    // (Workers) or an in-process dispatch closure (Bundle) — see
    // `emit_events_fanout_do`/`project.rs`'s `__eventsDispatch` construction.
    // P6.21 (continued, review of #1254): reads `Callee::Capability` (P6.0)
    // instead of `id.name == "Events" && method.name == "emit"` — a real
    // instance of R6.5's name-matched-receiver defect class, the same one
    // every other branch in this module already closed. `unit_table_uses_
    // emit` (`project.rs`) already reads this exact `Callee` for the
    // identical classification, project-wide. `cx.is_first_party_events()`
    // stays: it answers a different question (is *this unit's* `Events`
    // capability actually the first-party runtime implementation, not
    // which provider/target the emitted TS should call at all) than the
    // one `Callee` settles.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Capability { cap, op, .. }) if cap == "Events" && op == "emit"
        )
        && cx.is_first_party_events()
    {
        let event_name = type_args
            .first()
            .and_then(|t| match t {
                TypeRef::Named(ident) => Some(ident.name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "unknown".to_string());
        let payload = pre.lower(&args[0], cx);
        // Events track, slice 2 (spine #936): the envelope is minted here,
        // once, before fan-out duplicates this single `__events` entry to
        // every subscriber — so one emission always carries one `eventId`/
        // `emittedAt` to all of them (the invariant the identical-eventId
        // fixture proves). `publisherId` is the emitting *context's*
        // qualified name, not the emitting agent's instance identity:
        // `Events.emit` is also legal from a plain, keyless service handler
        // with no agent to report (`negative/503_events_emit_outside_owner`),
        // so a context-scoped identifier is the only one available
        // uniformly at every legal emission site — an amendment to
        // design/bynk-design-notes.md §7's "the publisher is the emitting
        // agent" framing (events-envelope ADR). `emittedAt` uses bare
        // `Date.now()`, not `given Clock`, matching the runtime's existing
        // JWT exp/nbf precedent of bypassing the `Clock` capability for its
        // own internal timestamps — requiring `Clock` alongside `Events`
        // would be new ambient coupling and would break every existing
        // fixture that emits with `given Events` alone. `schemaVersion` is
        // the event's own declared `@schema(N)` (Events slice 3b, #978),
        // or `1` if it declares none — a real, per-type computed value is
        // still a future slice (3c, the cross-build schema registry).
        let schema_version = cx.event_schema_version(&event_name);
        let publisher_id = escape_ts_string(cx.owning_context().unwrap_or_default());
        return Some(format!(
            "(async () => {{ __events.push({{ type: \"{event_name}\", payload: {payload}, envelope: {{ eventId: globalThis.crypto.randomUUID(), publisherId: \"{publisher_id}\", emittedAt: globalThis.Date.now(), schemaVersion: {schema_version} }} }}); }})()"
        ));
    }
    None
}

/// Lower a capability call: in a test `case` body, or on a capability named in `given`, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_capability_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // #1975: a capability call in a test `case` body (`Greeter.hello("ann")`)
    // reaches the case's own `deps`, built by `__makeTestDeps`: the checker
    // puts the target's capabilities in scope there, so it records the call
    // as a capability operation, but a case body is no handler and has no
    // `given` set for the branch below to read.
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.in_test_case()
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Capability { cap, .. }) if *cap == id.name
        )
    {
        let mut args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = format!(
            "deps.{}.{}({})",
            id.name,
            method.name,
            args_lowered.join(", ")
        );
        return Some(decode_capability_reply(
            call,
            wire.as_deref(),
            &id.name,
            &method.name,
            cx,
        ));
    }
    // #3147: a read through a `D1.peek` handle, whatever the handle is called:
    // the call is recorded on the capability the case holds as `deps.<cap>`.
    if let Some(Callee::Capability { cap, .. }) = cx.commons().callee(e.id)
        && cap.starts_with(bynk_check::checker::peek::LOCAL_PREFIX)
    {
        let cap = cap.clone();
        let mut args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = format!("deps.{cap}.{}({})", method.name, args_lowered.join(", "));
        return Some(decode_capability_reply(
            call,
            wire.as_deref(),
            &cap,
            &method.name,
            cx,
        ));
    }
    // Capability call: receiver is a bare ident naming a declared
    // capability in `given`. Lower to `<deps>.Capability.op(args)`,
    // where `<deps>` is `deps` in a handler body and `this.deps` in a
    // provider body (v0.12 provider composition).
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.has_capability(&id.name)
    {
        let mut args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        // #934: a flattened `consumes bynk { Idempotency }` local name is
        // first-party only if it's actually `bynk`'s own capability — either
        // declared right here (this unit *is* `bynk`) or flattened in from it
        // (`flattened_caps`). A same-named capability from any other adapter
        // or context (e.g. the #926/#931 generic-op test fixtures' own
        // illustrative `capability Idempotency`) must not be scoped.
        let is_first_party = id.name == "Idempotency"
            && (cx.in_bynk_unit()
                || cx
                    .cross_context()
                    .flattened_caps
                    .get(&id.name)
                    .map(String::as_str)
                    == Some("bynk"));
        scope_idempotency_key(is_first_party, &method.name, &mut args_lowered, cx);
        if is_first_party && method.name == "claim" && cx.tracks_claims() {
            let cap_expr = format!("{}.{}", cx.cap_deps_expr(), id.name);
            return Some(tracked_claim_call(&cap_expr, &args_lowered, cx));
        }
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = format!(
            "{}.{}.{}{}({})",
            cx.cap_deps_expr(),
            id.name,
            method.name,
            capability_call_type_args_ts(type_args, wire.as_deref()),
            args_lowered.join(", ")
        );
        let decoded = decode_capability_reply(call, wire.as_deref(), &id.name, &method.name, cx);
        let target = format!("{}.{}", id.name, method.name);
        return Some(cx.trace_step("capability", &target, e.span.start, decoded));
    }
    None
}

/// Lower a static call on a declared type (`Box.empty[Int]()`), or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_type_static_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // Static call: receiver is a bare ident naming a declared type — unless
    // the checker resolved it as a test-service address, which a target's
    // service named like a `uses`-imported type it hides is (#1927).
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.commons().types.contains_key(&id.name)
        && !matches!(
            cx.commons().callees.get(&e.id),
            Some(
                bynk_check::checker::Callee::TestService { .. }
                    | bynk_check::checker::Callee::TestAssetFetch { .. }
            )
        )
    {
        let mut args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        // #2719: a static method in the needs-context set takes the context last.
        let mut needs_exec = false;
        if let Some(Callee::Static(decl, witnesses)) = cx.commons().callees.get(&e.id) {
            // Constrained generics slice 4: the call's witnesses follow the
            // ordinary arguments, as for an instance method.
            // 1A: witness printed for a text caller
            args_lowered.extend(print_witnesses(cx.witness_args(decl, witnesses)));
            if cx.commons().exec_plan.fn_needs(decl) {
                args_lowered.push(cx.exec_expr());
                needs_exec = true;
            }
        }
        // Slice 4 (DECISION B): explicit type arguments (`Box.empty[Int]()`) are
        // kept, so a call with no argument of type `T` still types as written.
        let call = format!(
            "{}.{}{}({})",
            id.name,
            method.name,
            capability_call_type_args_ts(type_args, None),
            args_lowered.join(", ")
        );
        return Some(if needs_exec {
            settle_exec_call(call, cx)
        } else {
            call
        });
    }
    None
}

/// Lower a test `case` asset fetch (`svc.GET(asset)`), or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_test_asset_fetch_call(
    e: &Expr,
    receiver: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.7 / v0.182: local service call inside a test case body. `svc.call(args)`
    // lowers to `svc.call(args, deps)`. An http address `svc.<VERB>("/path", …)`
    // (#664) lowers to the emitted handler key `svc.http_<VERB>_<path>(<rest>,
    // <deps>)`, where `<deps>` folds in the call-site identity when present.
    // Slice B: a `system`-tier http address drives a real `worker.fetch` through
    // a driver (`__sysdrive_<svc>_<key>(rest, sub)`), not a direct handler call.
    // #1883, slice 3 ([DECISION F]): `svc.GET(asset)` in a case — the
    // checker classified it (`Callee::TestAssetFetch`). The platform serves an asset before the Worker runs, so it
    // reaches no handler in either tier: the module's `__bynkAssetGet` answers
    // from the files the target's Worker ships (`asset_harness`).
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callees.get(&e.id),
            Some(bynk_check::checker::Callee::TestAssetFetch { .. })
        )
        && let Some(first) = args.first()
    {
        let asset = pre.lower(first, cx);
        return Some(format!("__bynkAssetGet(({asset}).url)"));
    }
    None
}

/// Lower a `system`-tier cron or queue address, driven through the participant Worker, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_system_schedule_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // #2136: a `system`-tier cron or queue address drives the participant
    // Worker's real `scheduled` / `queue` entry through a driver, which builds
    // the platform-shaped event and settles the execution context itself.
    if let ExprKind::Ident(id) = &receiver.kind {
        let driver = match (method.name.as_str(), args.first().map(|a| &a.kind)) {
            ("schedule", Some(ExprKind::StrLit(expr))) => cx.system_schedule_driver(&id.name, expr),
            ("message", _) => cx.system_message_driver(&id.name),
            _ => None,
        };
        if let Some(driver) = driver {
            let skip = usize::from(method.name == "schedule");
            let rest: Vec<String> = args[skip..].iter().map(|a| pre.lower(a, cx)).collect();
            return Some(format!("{driver}({})", rest.join(", ")));
        }
    }
    None
}

/// Lower a `system`-tier http address, driven through a real `worker.fetch`, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_system_http_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.is_system_http_service(&id.name)
        && let Some(verb) = IrHttpMethod::from_ident(&method.name)
        && let Some(first) = args.first()
        && let ExprKind::StrLit(path) = &first.kind
    {
        // #707: a `(method, path)` whose path is a declared route but whose
        // method has no handler is a **wrong-method** call — drive the generic
        // `405` driver. The checker has already rejected a path that is not
        // declared for any method, so an absent route here means wrong method.
        let route_declared =
            cx.has_system_http_route(&(id.name.clone(), verb.as_str().to_string(), path.clone()));
        if !route_declared {
            return Some(format!(
                "__sysdrive_wrongmethod_{}({:?}, {:?})",
                id.name,
                verb.as_str(),
                path
            ));
        }
        let key = crate::emitter::http_handler_method_name_ir(verb, path);
        let sub = cx
            .call_site_identity
            .clone()
            .unwrap_or_else(|| "\"\"".to_string());
        // #706: `by Nobody` drives the *no-auth* driver — no `Authorization`
        // header, so the real seam rejects it (`401` → `Rejected(Unauthorized)`);
        // it takes precedence over the body form (the seam rejects before the
        // body is read). Otherwise — Slice C: a `Wire(…)` argument drives the
        // *raw* driver (input sent unvalidated, result decodes to an
        // `HttpOutcome`); a fully typed call keeps the serialising driver.
        // #821: the two axes are independent — a `Wire(…)` argument combined
        // with `by Nobody` drives the raw *and* no-auth driver together, so
        // the missing-credential rejection and the unvalidated body both
        // apply, rather than one silently discarding the other.
        let has_wire = args[1..]
            .iter()
            .any(|a| matches!(&a.kind, ExprKind::Wire(_)));
        let driver = match (cx.call_site_no_credential, has_wire) {
            (true, true) => "__sysdrive_rawnoauth".to_string(),
            (true, false) => "__sysdrive_noauth".to_string(),
            (false, true) => "__sysdrive_raw".to_string(),
            (false, false) => "__sysdrive".to_string(),
        };
        // #2135: `by Forged` / `by Stale` drive the signing driver that sends
        // a wrong or a stale signature (`__sysdrive_forged`, `…_rawstale`).
        let driver = match (cx.call_site_bad_signature, has_wire) {
            (Some(v), true) => format!("__sysdrive_raw{v}"),
            (Some(v), false) => format!("__sysdrive_{v}"),
            (None, _) => driver,
        };
        let is_raw = driver.starts_with("__sysdrive_raw");
        // #708/#821: the raw (and raw+no-auth) driver's every slot is a
        // `string`. `lower_expr_into` already lowers a `Wire(s)` to its raw inner
        // string, but a *typed* arg mixed into the same raw call must be
        // converted to that `string` slot: the body param serialises through
        // the same wire codec the typed driver uses (so a hand-typed body
        // matches a `Wire`d one byte-for-byte); any other (path) param just
        // coerces via `String(...)`.
        let body_info = if is_raw {
            cx.system_http_route_body(&(id.name.clone(), verb.as_str().to_string(), path.clone()))
                .cloned()
        } else {
            None
        };
        let query_info = if is_raw {
            cx.system_http_route_body(&(
                id.name.clone(),
                verb.as_str().to_string(),
                format!("{path}{}", crate::emitter::QUERY_ROUTE_SUFFIX),
            ))
            .cloned()
        } else {
            None
        };
        // #3276: a `@form` route's typed body is written as a form, not JSON.
        let form_route = is_raw
            && cx
                .system_http_route_body(&(
                    id.name.clone(),
                    verb.as_str().to_string(),
                    format!("{path}{}", crate::emitter::FORM_ROUTE_SUFFIX),
                ))
                .is_some();
        // #3277: the `cookies` record's position, when this call's route takes one.
        let cookies_info = if is_raw {
            cx.system_http_route_body(&(
                id.name.clone(),
                verb.as_str().to_string(),
                format!("{path}{}", crate::emitter::COOKIES_ROUTE_SUFFIX),
            ))
            .cloned()
        } else {
            None
        };
        let rest: Vec<String> = args[1..]
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let lowered = pre.lower(a, cx);
                if !is_raw || matches!(&a.kind, ExprKind::Wire(_)) {
                    return lowered;
                }
                // A `headers: RequestHeaders` argument is no wire slot: it
                // reaches the raw driver typed, just as the typed driver takes
                // it, and becomes the request's headers, so it passes through
                // unconverted.
                if matches!(
                    cx.commons().expr_ty(a.id).as_deref(),
                    Some(Ty::RequestHeaders | Ty::RequestQuery | Ty::RequestCookies)
                ) {
                    return lowered;
                }
                // #3277: a typed `cookies` record becomes the `Cookie` header text
                // the raw driver sends.
                if let Some((cookies_idx, ty)) = &cookies_info
                    && *cookies_idx == i
                {
                    cx.note_cookie_string();
                    return format!(
                        "__cookieString({})",
                        bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                            ty,
                            &format!("({lowered} as any)"),
                            cx.system_http_type_ns(),
                            cx.runtime_use()
                        ))
                    );
                }
                // #2854: a typed `query` record becomes the query string the
                // raw driver appends to the URL.
                if let Some((query_idx, ty)) = &query_info
                    && *query_idx == i
                {
                    cx.note_query_string();
                    return format!(
                        "__queryString({})",
                        bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                            ty,
                            &format!("({lowered} as any)"),
                            cx.system_http_type_ns(),
                            cx.runtime_use()
                        ))
                    );
                }
                match &body_info {
                    // A named type's typed literal is unbranded in test-scaffold
                    // code (the checker already validated it against the
                    // handler's declared type — Slice A), the same reason
                    // `driver_param_ty` types a named driver param `any` rather
                    // than the branded type. Cast through `any` here too, so
                    // `serialise_<Name>` (which expects the branded shape)
                    // accepts the plain object literal.
                    //
                    // P7.2: deferred, not narrowed. A first attempt used
                    // `as unknown as {ty}` with a bare `ts_type_ref` and broke a
                    // real `tsc --strict` fixture (385, "Cannot find name") — the
                    // same bare-vs-qualified problem found (and fixed via
                    // `emitter/workers.rs`'s `qualified_type_ref`) for several
                    // other sites this slice touched, but that helper isn't
                    // reachable from this module, and this call site's own scope
                    // (`cx.system_http_type_ns()`) suggests the real
                    // qualification here may differ from `workers.rs`'s own
                    // `handlers` namespace besides — needs its own investigation,
                    // not a copy-paste of a fix built for a different file.
                    // #1435 (Arc E slice 1): `serialise_expr_via` now returns a
                    // real `bynk_ts::TsExpr` — this call site is `lower.rs`'s
                    // own permanent opaque exception (ADR
                    // `arc-c-lower-rs-permanent-exclusion`), so it prints at
                    // the boundary (`bynk_ts::print_expr`), the same
                    // treatment cluster 4's own `lower.rs:2552` call site
                    // already established.
                    Some((body_idx, ty)) if *body_idx == i && form_route => {
                        cx.note_query_string();
                        format!(
                            "__queryString({}).slice(1)",
                            bynk_ts::print_expr(
                                &crate::emitter::serialisation::serialise_expr_via(
                                    ty,
                                    &format!("({lowered} as any)"),
                                    cx.system_http_type_ns(),
                                    cx.runtime_use()
                                )
                            )
                        )
                    }
                    Some((body_idx, ty)) if *body_idx == i => format!(
                        "globalThis.JSON.stringify({})",
                        bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                            ty,
                            &format!("({lowered} as any)"),
                            cx.system_http_type_ns(),
                            cx.runtime_use()
                        ))
                    ),
                    _ => format!("String({lowered})"),
                }
            })
            .collect();
        let mut all = rest;
        // #2910: a route that declares no `headers:` takes the case's extra
        // request headers (the call's trailing `RequestHeaders`, when the
        // checker accepted one) in a slot ahead of `sub`.
        if cx.has_system_http_route(&(
            id.name.clone(),
            verb.as_str().to_string(),
            format!("{path}{}", crate::emitter::EXTRA_HEADERS_ROUTE_SUFFIX),
        )) {
            let sends_extra = matches!(
                cx.commons().callees.get(&e.id),
                Some(Callee::TestService {
                    extra_headers: true,
                    ..
                })
            );
            let slot = if sends_extra {
                all.pop().unwrap_or_else(|| "undefined".to_string())
            } else {
                "undefined".to_string()
            };
            all.push(slot);
        }
        all.push(sub);
        // #2019: a refinement-guarded route's signing drivers take the claims
        // the acting actor presents; the no-auth drivers sign nothing.
        if !cx.call_site_no_credential
            && cx.call_site_bad_signature.is_none()
            && let Some(claims) = cx.system_http_claims_arg(&(
                id.name.clone(),
                verb.as_str().to_string(),
                path.clone(),
            ))
        {
            all.push(claims);
        }
        // #2155: an actor-sum route's signing drivers take the actor the call
        // acts as, which picks its credential.
        if !cx.call_site_no_credential
            && cx.call_site_bad_signature.is_none()
            && let Some(actor) = cx.system_http_sum_actor_arg(&(
                id.name.clone(),
                verb.as_str().to_string(),
                path.clone(),
            ))
        {
            all.push(actor);
        }
        return Some(format!(
            "{}_{}_{}({})",
            driver,
            id.name,
            key,
            all.join(", ")
        ));
    }
    None
}

/// Lower a local service call inside a test `case` body, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_test_service_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // #2024: a case `let` may shadow a service of the target; the local wins.
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.is_test_service(&id.name)
        && !cx.is_local(&id.name)
    {
        // #1994: a target whose handlers send with `detach` settles each addressed
        // call — awaits the promises the handler handed `deps.__exec.waitUntil`
        // — so the case's expectations see the sends' effects.
        let settles = cx.test_settles_sends();
        // #2683: on virtual time every addressed call is driven (auto-advance)
        // and then settled, whether or not the handler also sends.
        let virtual_time = cx.runtime_use().virtual_time();
        let settle = |call: String| {
            if virtual_time {
                format!("__bynkSettleVirtual(deps, {call})")
            } else if settles {
                format!("__bynkSettle(deps, {call})")
            } else {
                call
            }
        };
        // P7.2: deferred, not narrowed. `identity` is the raw lowered call-site value
        // (e.g. `"bob"`), with no reachable identity type at this `cx` — resolving
        // one needs the addressed actor's own identity type threaded through from
        // `bynk-check`'s actor declarations, not plumbed here today (unlike the
        // `ty: &TypeRef` available a few lines up for the sibling brand cast). A
        // guessed type risks a `tsc --strict` mismatch; left as `any`, named here.
        // I3 (#1971), #2020: a sum handler (`on call` or http) reads the
        // principal as the resolved actor, `deps.who` (the emitted handler's
        // deps key for every sum, whatever its binder is named):
        // `by Caller("shop.front")`, `by User("ada")` or `by Visitor`.
        let sum_key = match (IrHttpMethod::from_ident(&method.name), args.first()) {
            (Some(verb), Some(first)) => match &first.kind {
                ExprKind::StrLit(path) => {
                    Some(crate::emitter::http_handler_method_name_ir(verb, path))
                }
                _ => None,
            },
            _ if method.name == "call" => Some("call".to_string()),
            // #3051: a websocket `open` reads its principal as an `on call` or
            // http handler does.
            _ if method.name == "open" => Some("open".to_string()),
            _ => None,
        };
        let is_sum = sum_key
            .as_deref()
            .is_some_and(|k| cx.is_sum_handler(&id.name, k));
        let deps_expr = match (cx.call_site_actor.clone(), cx.call_site_identity.clone()) {
            (Some(actor), identity) if is_sum => match identity {
                Some(identity) => {
                    format!("{{ ...deps, who: {{ tag: {actor:?}, identity: {identity} }} }}")
                }
                None => format!("{{ ...deps, who: {{ tag: {actor:?} }} }}"),
            },
            (_, Some(_))
                if sum_key
                    .as_deref()
                    .is_some_and(|k| cx.is_binderless_handler(&id.name, k)) =>
            {
                "deps".to_string()
            }
            (_, Some(identity)) => {
                // #2380 (L5): an `Oidc` handler also reads its token's claims
                // from `deps.claims`: the principal's, or none.
                let claims = if sum_key
                    .as_deref()
                    .is_some_and(|k| cx.is_oidc_handler(&id.name, k))
                {
                    cx.note_token_claims();
                    let claims = cx
                        .call_site_claims
                        .clone()
                        .unwrap_or_else(|| "__tokenClaims([])".to_string());
                    format!(", claims: {claims}")
                } else {
                    String::new()
                };
                format!("{{ ...deps, identity: ({identity} as any){claims} }}")
            }
            (_, None) => "deps".to_string(),
        };
        // http address: the method is an HTTP verb and the first argument is the
        // route pattern string. The key is a pure function of verb + path; the
        // remaining args are the handler's positional params.
        if let Some(verb) = IrHttpMethod::from_ident(&method.name)
            && let Some(first) = args.first()
            && let ExprKind::StrLit(path) = &first.kind
        {
            let key = crate::emitter::http_handler_method_name_ir(verb, path);
            let rest: Vec<String> = args[1..].iter().map(|a| pre.lower(a, cx)).collect();
            let mut all = rest;
            all.push(deps_expr);
            return Some(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
        }
        // cron/queue address: the emitted key is position-indexed among the
        // service's same-kind handlers, so recover the index from the handler
        // list. cron drops the leading schedule string; queue passes the
        // message. P6.37 (design/tracks/the-ir.md §6a): reads `IrHandlerKind`
        // (P6.24a's own pure, unconditional mirror) rather than the raw AST
        // `HandlerKind` this map used to store.
        if let Some(handlers) = cx.test_service_handlers(&id.name) {
            if method.name == "schedule"
                && let Some(ExprKind::StrLit(expr)) = args.first().map(|a| &a.kind)
            {
                let mut idx = 0usize;
                for h in handlers {
                    if let bynk_model::IrHandlerKind::Cron { expr: e } = h {
                        if e == expr {
                            break;
                        }
                        idx += 1;
                    }
                }
                let key = crate::emitter::cron_handler_method_name(&id.name, idx);
                let rest: Vec<String> = args[1..].iter().map(|a| pre.lower(a, cx)).collect();
                let mut all = rest;
                all.push(deps_expr);
                return Some(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
            }
            // #3051: the websocket lifecycle addresses. The `TestConnection`
            // remembers the route values and `deps` (the principal) from `open`;
            // `message`/`close` rebuild the handler's argument list from them. The
            // checker encoded the shape in the callee's address.
            if let Some(Callee::TestService { address, .. }) = cx.commons().callee(e.id)
                && let Some(shape) = address.strip_prefix("ws ")
            {
                let mut parts = shape.split(' ');
                let which = parts.next().unwrap_or_default();
                let nums: Vec<usize> = parts.filter_map(|p| p.parse().ok()).collect();
                let tc = args
                    .first()
                    .map(|a| pre.lower(a, cx))
                    .unwrap_or_else(|| "undefined".to_string());
                let svc = &id.name;
                // Typed from the handler's own signature (`Parameters<typeof …>`), so
                // `tsc` checks the arity, the frame position and the `deps` shape
                // against the emitted handler; only the route values, which the
                // `TestConnection` holds as `unknown`, are cast, as production's
                // dispatch casts `__att.args[i]`.
                let sig = |m: &str| format!("Parameters<typeof {svc}.{m}>");
                let call = match which {
                    "open" => {
                        let route: Vec<String> =
                            args[1..].iter().map(|a| pre.lower(a, cx)).collect();
                        let mut parts = vec!["__c".to_string()];
                        parts.extend(route.iter().cloned());
                        parts.push(deps_expr.clone());
                        format!(
                            "(async () => {{ const __c = {tc}; const __a: {} = [{}]; __c.remember([{}], __a[__a.length - 1]); return {svc}.open(...__a); }})()",
                            sig("open"),
                            parts.join(", "),
                            route.join(", ")
                        )
                    }
                    "message" => {
                        let (frame_idx, n) = (
                            nums.first().copied().unwrap_or(0),
                            nums.get(1).copied().unwrap_or(1),
                        );
                        let frame = args
                            .get(1)
                            .map(|a| pre.lower(a, cx))
                            .unwrap_or_else(|| "undefined".to_string());
                        let p = sig("message");
                        let mut k = 0usize;
                        let mut parts = vec!["__c".to_string()];
                        for i in 0..n {
                            if i == frame_idx {
                                parts.push(frame.clone());
                            } else {
                                parts.push(format!("__o.route[{k}] as {p}[{}]", i + 1));
                                k += 1;
                            }
                        }
                        parts.push(format!("__o.deps as {p}[{}]", n + 1));
                        format!(
                            "(async () => {{ const __c = {tc}; const __o = __c.opened(); const __a: {p} = [{}]; return {svc}.message(...__a); }})()",
                            parts.join(", ")
                        )
                    }
                    _ => {
                        let n = nums.first().copied().unwrap_or(0);
                        let p = sig("close");
                        let mut parts = vec!["__c".to_string()];
                        for i in 0..n {
                            parts.push(format!("__o.route[{i}] as {p}[{}]", i + 1));
                        }
                        parts.push(format!("__o.deps as {p}[{}]", n + 1));
                        format!(
                            "(async () => {{ const __c = {tc}; const __o = __c.opened(); const __a: {p} = [{}]; return {svc}.close(...__a); }})()",
                            parts.join(", ")
                        )
                    }
                };
                return Some(settle(call));
            }
            if method.name == "message"
                && handlers
                    .iter()
                    .any(|h| matches!(h, bynk_model::IrHandlerKind::Message))
            {
                // A `from queue` service binds exactly one queue and declares one
                // `on message` handler, so the position index is 0.
                let key = crate::emitter::queue_handler_method_name(&id.name, 0);
                let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
                // #3328: a case has no Email Routing to mint the `Sender` identity,
                // so a handler that binds it reads the envelope sender with no
                // DMARC verdict (`DmarcNone`) unless the case names a principal.
                if deps_expr == "deps"
                    && args.first().is_some_and(|a| is_inbound_email(a, cx))
                    && let Some(mail) = args_lowered.first()
                {
                    return Some(settle(format!(
                        "((__m: any) => {}.{}(__m, {{ ...deps, identity: {{ address: __m.from, dmarc: {{ tag: \"DmarcNone\" }} }} }}))({mail})",
                        id.name, key
                    )));
                }
                let mut all = args_lowered;
                all.push(deps_expr);
                return Some(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
            }
            // #2052: an event address delivers through the module's
            // `__bynkEvent_<svc>` helper (`tests_emit::event_delivery_helper`),
            // which supplies the default envelope and calls the service's
            // generated `event` method, whose prologue runs the pattern filter
            // and the `via schema(N)` guard as every real delivery does. An
            // omitted envelope is passed as `undefined`.
            if method.name == "event"
                && handlers
                    .iter()
                    .any(|h| matches!(h, bynk_model::IrHandlerKind::Event))
            {
                cx.runtime_use().note_test_event_service(&id.name);
                let payload = args
                    .first()
                    .map(|a| pre.lower(a, cx))
                    .unwrap_or_else(|| "undefined".to_string());
                let envelope = args
                    .get(1)
                    .map(|a| pre.lower(a, cx))
                    .unwrap_or_else(|| "undefined".to_string());
                return Some(settle(format!(
                    "__bynkEvent_{}({payload}, {envelope}, {deps_expr})",
                    id.name
                )));
            }
        }
        // `svc.call(args)` and other (non-http) forms: pass args through with deps.
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let mut all = args_lowered;
        all.push(deps_expr);
        return Some(settle(format!(
            "{}.{}({})",
            id.name,
            method.name,
            all.join(", ")
        )));
    }
    None
}

/// #3328: whether `e` is a first-party `InboundEmail`, the message an email
/// service's `on message` takes (a queue's message is the user's own type).
fn is_inbound_email(e: &Expr, cx: &LowerCtx) -> bool {
    cx.commons().expr_types.get(&e.id).is_some_and(|te| {
        matches!(&*cx.commons().tys().get(te.ty), bynk_check::checker::Ty::Named { name, .. } if name == "InboundEmail")
    })
}

/// Lower an inline agent invocation, `Agent(<key>).method(args)`, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_inline_agent_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.9.2: inline agent invocation. Source form is
    // `Agent(<key>).method(args)`; receiver parses as
    // `Call(Agent, [<key>])`. Lower to
    // `__makeAgent(<key>).method(args, deps)`. Works in service and
    // agent-handler bodies (deps is the handler's deps parameter) and
    // test bodies (deps is the locally-built __makeTestDeps record).
    //
    // P6.21 (partial, continued): reads `Callee::Agent` (P6.0) instead of
    // `cx.local_agents.contains(&name.name)` — the checker's own
    // `check_method_call` records this the moment a receiver's *checked
    // type* resolves to an agent, uniformly for this shape, the let-bound
    // shape below, and the self-agent WS-transfer case inside this branch —
    // closing a real instance of R6.5's name-matched-receiver defect class
    // (a free function or local sharing an agent's own name could misfire
    // the old check; the checker's own type-driven resolution cannot).
    if let ExprKind::Call {
        name,
        args: ctor_args,
        ..
    } = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Agent { .. }))
    {
        // v0.104 (real-time track slice 3b): when lowering a `from websocket`
        // `on open` body inside its hosting Durable Object, a transfer to the
        // self-agent is a direct `this.method(args, deps)` self-call — the key
        // addresses *this* DO, and the held connection never crosses an RPC
        // boundary (DECISION A). The key expression is **not** emitted: the shape
        // constraint (`bynk.ws.open_transfer_shape`, D2) restricts it to a
        // request-derivable param ident — side-effect-free and equal to this
        // instance's own key — so dropping it is sound. #2959 widened the key to a
        // context `const` or a literal (`bynk.ws.open_transfer_key`), and #2967
        // to a used commons' `const`; all are pure and the edge routed the
        // upgrade by the same value, so it still is.
        if cx.ws_self_agent() == Some(name.name.as_str()) {
            cx.record_agent_call(&name.name, &method.name);
            let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
            let mut all = args_lowered;
            all.push("deps".to_string());
            return Some(format!(
                "this.{method}({args})",
                method = method.name,
                args = all.join(", ")
            ));
        }
        cx.record_agent_call(&name.name, &method.name);
        let key_arg = ctor_args
            .first()
            .map(|a| pre.lower(a, cx))
            .unwrap_or_else(|| "\"default\"".to_string());
        let instance = cx.agent_construct(&name.name, &key_arg);
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let mut all = args_lowered;
        // #2355: `this.deps` in a provider body.
        all.push(cx.cap_deps_expr().to_string());
        let call = format!(
            "{instance}.{method}({args})",
            method = method.name,
            args = all.join(", ")
        );
        let target = format!("{}.{}", name.name, method.name);
        return Some(cx.trace_step("agent", &target, e.span.start, call));
    }
    None
}

/// Lower a method call on a let-bound agent instance, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_let_bound_agent_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // Let-bound agent invocation. `let x = Agent(key); x.method(args)`
    // — the statement emitter recorded `x` as an agent variable when
    // it lowered the let. Method calls on `x` go straight to the
    // class instance with `deps` threaded through.
    //
    // P6.21 (partial, continued): reads `Callee::Agent` instead of
    // `cx.local_agent_vars.contains_key(&id.name)` — see the inline
    // agent-invocation branch above. `cx.local_agent_vars` stays: it still
    // answers "which agent does this bind name refer to" (needed for
    // `record_agent_call`'s own bookkeeping and the #908 rename-resolution
    // below), a different question from "is this receiver really an agent
    // instance at all," which `Callee` now settles.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Agent { .. }))
    {
        if let Some(agent) = cx.local_agent_vars.get(&id.name).cloned() {
            cx.record_agent_call(&agent, &method.name);
        }
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let mut all = args_lowered;
        all.push(cx.cap_deps_expr().to_string());
        // #908: `local_agent_vars` is keyed by the bynk source name, which a
        // re-`let` may have renamed — resolve to the actual JS binding rather
        // than assuming the source name is still it.
        let recv = cx
            .resolved_local_name(&id.name)
            .unwrap_or_else(|| id.name.clone());
        let call = format!("{}.{}({})", recv, method.name, all.join(", "));
        let target = match cx.local_agent_vars.get(&id.name) {
            Some(agent) => format!("{agent}.{}", method.name),
            None => method.name.clone(),
        };
        return Some(cx.trace_step("agent", &target, e.span.start, call));
    }
    None
}

/// Lower the built-in scalar and collection statics (`HttpResult.*`, `List.empty`, `Map.empty`, `Set.empty`/`of`, the JSON codec, `Float.pi`/`e`, the numeric parses, `Duration.millis`, `Instant.from*`), or `None` when the call is none of them. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_scalar_static_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let tys = cx.ty_render();
    // v0.9: explicit `HttpResult.Variant(args)` construction. The
    // checker has already recorded the expression's type — emit it
    // directly through the runtime's HttpResult namespace.
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` (the sink
    // #1251 added) instead of `id.name == HTTP_RESULT` — closes the last
    // real instance of R6.5's name-matched-receiver defect class this
    // session's own P6.21 slices left open, since Decision C originally
    // excluded this shape for lack of a `Callee` to read.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, .. }) if *ns == HTTP_RESULT
        )
    {
        cx.note_http_result();
        // 1A: the method-call dispatchers stay text (see `lower_method_call`).
        let args_lowered: Vec<String> = lower_http_variant_args(&method.name, args, pre, cx)
            .into_iter()
            .map(LoweredExpr::into_text)
            .collect();
        return Some(format!(
            "HttpResult.{}({})",
            method.name,
            args_lowered.join(", ")
        ));
    }
    // v0.20b: built-in collection statics — `List.empty()` /
    // `Map.empty()`. The checker recorded the instantiated type;
    // emit it explicitly so the TS value doesn't infer as `never[]`
    // / `Map<unknown, unknown>` outside contextually-typed positions.
    //
    // P6.21 (partial, continued — the earlier #1247 attempt reverted this
    // exact conversion after a zero-diff bless caught it silently dropping
    // `Instant`/`Bytes` statics inside `.test.bynk` bodies; root-caused and
    // fixed separately — `typecheck_case_body` now returns its own
    // `callees` map instead of discarding it, so `Callee::Intrinsic` is
    // populated for test-case bodies too): reads `Callee::Intrinsic` (P6.0)
    // instead of `id.name == LIST || id.name == MAP` plus
    // `!cx.commons().types.contains_key` — the checker's own
    // `check_method_call` already resolves the identical shadowing question
    // (`ctx.lookup(...).is_none() && !ctx.input.types.contains_key(...)`,
    // `calls.rs`) before ever recording this `Callee`, closing a real gap
    // the emitter's own guard left open: no `cx.is_local(&id.name)` check,
    // unlike every other branch this session already converted.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if (*ns == LIST || *ns == MAP || *ns == SET) && op == "empty"
        )
        && args.is_empty()
    {
        match cx.commons().expr_ty(e.id).as_deref() {
            // K6a (#2596).
            Some(Ty::Set(t)) => {
                return Some(format!("new globalThis.Set<{}>()", ts_ty(*t, tys)));
            }
            Some(Ty::List(t)) => {
                return Some(format!(
                    "([] as readonly {}[])",
                    ts_array_elem(&ts_ty(*t, tys))
                ));
            }
            Some(Ty::Map(k, v)) => {
                return Some(format!(
                    "new globalThis.Map<{}, {}>()",
                    ts_ty(*k, tys),
                    ts_ty(*v, tys)
                ));
            }
            _ => {}
        }
    }
    // K6a (#2596): `Set.of(xs)` — a set of the list's elements; the first
    // occurrence keeps its position.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == SET && op == "of"
        )
        && args.len() == 1
        && let Some(Ty::Set(t)) = cx.commons().expr_ty(e.id).as_deref()
    {
        let t = *t;
        let xs = pre.lower(&args[0], cx);
        if !key_is_plain(t, tys.types()) {
            cx.runtime_use().note_key("__bynkSetOf");
            return Some(format!("__bynkSetOf<{}>({xs})", ts_ty(t, tys)));
        }
        return Some(format!("new globalThis.Set<{}>({xs})", ts_ty(t, tys)));
    }
    // v0.22b: the typed JSON codec (ADR 0045).
    if let Some(s) = pre.absorb_opt(lower_json_codec_call(e, receiver, method, args, cx)) {
        return Some(s);
    }
    // K6b (#2598): `Float.pi()` and `Float.e()`.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == FLOAT && (op == "pi" || op == "e")
        )
        && args.is_empty()
    {
        let constant = if method.name == "pi" { "PI" } else { "E" };
        return Some(format!("globalThis.Math.{constant}"));
    }
    // v0.22a: the numeric parse statics — `Int.parse(s)` /
    // `Float.parse(s)` (ADR 0048). #1657 (runtime-semantics track §3.4): the
    // whole string must match a strict decimal grammar before `Number(…)` sees
    // it. `Number` alone accepts surrounding whitespace, `0x`/`0b`/`0o` prefixes
    // and (for `Int`) exponents, so `" 7 "`, `"0x10"` and `"1e3"` parsed. `Int`
    // is `[+-]?digits`, then a safe integer (`+ 0` turns `-0` into `0`); `Float`
    // is a decimal with an optional exponent, then finite (the 0040 posture).
    // P6.21 (partial, continued): reads `Callee::Intrinsic` instead of
    // `id.name == INT || id.name == FLOAT` — see the `List`/`Map` branch
    // above. `Int`/`Float` are lexically reserved (the parser only admits
    // them here followed by `.`), so unlike `List`/`Map` this specific guard
    // never had a real shadowing gap to close — converted for consistency
    // with every other static-constructor branch in this function.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if (*ns == INT || *ns == FLOAT) && op == "parse"
        )
        && args.len() == 1
    {
        let s = pre.lower(&args[0], cx);
        let (grammar, value, guard) = if id.name == INT {
            (
                "/^[+-]?[0-9]+$/",
                "globalThis.Number(__s) + 0",
                "globalThis.Number.isSafeInteger(__n)",
            )
        } else {
            (
                "/^[+-]?([0-9]+(\\.[0-9]*)?|\\.[0-9]+)([eE][+-]?[0-9]+)?$/",
                "globalThis.Number(__s)",
                "globalThis.Number.isFinite(__n)",
            )
        };
        return Some(format!(
            "((__s: string) => {{ if (!{grammar}.test(__s)) return None; const __n = {value}; return {guard} ? Some(__n) : None; }})({s})"
        ));
    }
    // v0.86 (ADR 0112): `Duration.millis(n)` — the runtime `Int`→`Duration`
    // constructor. A `Duration` lowers to its milliseconds, so this is the
    // identity on the argument.
    //
    // P6.21 (partial, continued): reads `Callee::Intrinsic` instead of
    // `id.name == DURATION` — see the `List`/`Map` branch above for why this
    // closes a real shadowing gap the bare name check left open (the checker
    // itself guards `ctx.lookup(DURATION).is_none()` before ever recording
    // this `Callee`; the same applies to `Instant`/`Bytes`/`Stream` below).
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == DURATION && op == "millis"
        )
        && args.len() == 1
    {
        let inner = pre.lower(&args[0], cx);
        return Some(inner);
    }
    // K3 (#2592): `Instant.fromIso(s)`, a guarded RFC 3339 read.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == INSTANT && op == "fromIso"
        )
        && args.len() == 1
    {
        let s = pre.lower(&args[0], cx);
        cx.note_time_ops();
        return Some(format!("__bynkInstantFromIso({s})"));
    }
    // v0.90 (ADR 0114): `Instant.fromEpochMillis(n)` — an `Instant` lowers to
    // its epoch milliseconds, so this is the identity on the argument.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == INSTANT && op == "fromEpochMillis"
        )
        && args.len() == 1
    {
        let inner = pre.lower(&args[0], cx);
        return Some(inner);
    }
    None
}

/// Lower the statics that build a runtime value (`Bytes.*`, `Decimal.*`, `TokenClaims.of`, `RequestHeaders.*`, `RequestQuery.*`, `Stream.of`), or `None` when the call is none of them. Moved out of
/// `lower_method_call` (#3167) unchanged.
fn lower_runtime_value_static_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.110 (ADR 0142 D2): the `Bytes` static constructors. `fromUtf8` is the
    // UTF-8 encoding of a string (total); `fromBase64` is a guarded base64
    // decode returning `Option` (`None` on invalid base64); `empty` is the
    // zero octet sequence.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == BYTES)
    {
        match (method.name.as_str(), args.len()) {
            ("fromUtf8", 1) => {
                let s = pre.lower(&args[0], cx);
                return Some(format!("new globalThis.TextEncoder().encode({s})"));
            }
            ("fromBase64", 1) => {
                let s = pre.lower(&args[0], cx);
                cx.note_bytes();
                return Some(format!("__bynkBytesFromBase64({s})"));
            }
            // #2016: the guarded hex and base64url decodes, `None` on bad input.
            ("fromHex", 1) => {
                let s = pre.lower(&args[0], cx);
                cx.note_bytes_ops();
                return Some(format!("__bynkBytesFromHex({s})"));
            }
            ("fromBase64Url", 1) => {
                let s = pre.lower(&args[0], cx);
                cx.note_bytes_ops();
                return Some(format!("__bynkBytesFromBase64Url({s})"));
            }
            ("empty", 0) => {
                return Some("new globalThis.Uint8Array()".to_string());
            }
            _ => {}
        }
    }
    // #2622: `Decimal.parse(s)` and `Decimal.fromInt(n)`.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == DECIMAL)
    {
        match (method.name.as_str(), args.len()) {
            ("parse", 1) => {
                let s = pre.lower(&args[0], cx);
                cx.note_decimal("__bynkDecimalParse");
                return Some(format!("__bynkDecimalParse({s})"));
            }
            ("fromInt", 1) => {
                let n = pre.lower(&args[0], cx);
                cx.note_decimal("__bynkDecimalFromInt");
                return Some(format!("__bynkDecimalFromInt({n})"));
            }
            _ => {}
        }
    }
    // #2380 (L5): `TokenClaims.of(lines)` builds the runtime's claims view.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, op }) if *ns == TOKEN_CLAIMS && op == "of")
        && args.len() == 1
    {
        let xs = pre.lower(&args[0], cx);
        cx.note_token_claims();
        return Some(format!("__tokenClaims({xs})"));
    }
    // #2341 (L4): `RequestHeaders.of(lines)` / `RequestHeaders.empty()` build
    // the runtime's headers view from `"name: value"` lines.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == REQUEST_HEADERS)
    {
        match (method.name.as_str(), args.len()) {
            ("of", 1) => {
                let xs = pre.lower(&args[0], cx);
                cx.note_request_headers();
                return Some(format!("__requestHeaders({xs})"));
            }
            ("empty", 0) => {
                cx.note_request_headers();
                return Some("__requestHeaders([])".to_string());
            }
            _ => {}
        }
    }
    // #2854: `RequestQuery.of(pairs)` / `RequestQuery.empty()` build the
    // runtime's query view from `"name=value"` pairs.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == REQUEST_QUERY)
    {
        match (method.name.as_str(), args.len()) {
            ("of", 1) => {
                let xs = pre.lower(&args[0], cx);
                cx.note_request_query();
                return Some(format!("__requestQuery({xs})"));
            }
            ("empty", 0) => {
                cx.note_request_query();
                return Some("__requestQuery([])".to_string());
            }
            _ => {}
        }
    }
    // #3277: `RequestCookies.of(pairs)` / `RequestCookies.empty()` build the
    // runtime's cookies view from `"name=value"` pairs.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == REQUEST_COOKIES)
    {
        match (method.name.as_str(), args.len()) {
            ("of", 1) => {
                let xs = pre.lower(&args[0], cx);
                cx.note_request_cookies();
                return Some(format!("__requestCookies({xs})"));
            }
            ("empty", 0) => {
                cx.note_request_cookies();
                return Some("__requestCookies([])".to_string());
            }
            _ => {}
        }
    }
    // v0.100: `Stream.of(xs)` — the deterministic in-memory source. A `Stream`
    // lowers to a host async iterable; `of` wraps a list as an async generator.
    // Emitted inline (no runtime import), like the collection kernels.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == STREAM && op == "of"
        )
        && args.len() == 1
    {
        let xs = pre.lower(&args[0], cx);
        return Some(format!(
            "(async function* () {{ for (const __e of {xs}) {{ yield __e; }} }})()"
        ));
    }
    None
}

/// The persisted-store receivers of a method call: a `store` field of the
/// current agent used as `<field>.<op>(…)`. Each kind has its own function, tried
/// in order; `None` means the receiver is none of them.
fn lower_store_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    lower_store_held_map_call(e, receiver, method, args, cx, pre)
        .or_else(|| lower_store_map_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_set_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_cache_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_log_call(e, receiver, method, args, cx, pre))
        .or_else(|| lower_store_cell_call(receiver, method, args, cx, pre))
}

/// Lower a held `store Map[K, Connection]` operation (Workers: connection ids are persisted), or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_held_map_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let tys = cx.ty_render();
    // v0.104/v0.105 (real-time track slice 3b): a held-`Map` operation —
    // `<map>.<op>(…)` on a `store Map[K, Connection]` field on Workers. The live
    // socket cannot be persisted, so the durable record stores the **connection id**
    // (`__state.<map>` is a `Record<string(K), connId>`) and each `Connection` is
    // re-resolved from its connId via `resolveConnection` — so a stored connection
    // survives DO eviction (§2.9.6, slice 3b-ii). `put` records the connection's id
    // (`connIdOf`); `get` resolves it (or `None` if the socket has since closed);
    // `remove` resolves-closes-deletes (the §2.9 "removes-and-closes" contract). The
    // record mutation is staged in `__state` and flushed by the same end-of-handler
    // commit as any other persisted field.
    // P6.21 (partial, continued): the receiver test reads the checker's own
    // `Callee::Store`/`Callee::Query` (P6.0) instead of
    // `!cx.is_local(&id.name)` — a real instance of R6.5's name-matched-
    // receiver defect class. A held `store Map[K, Connection]` field's
    // method calls resolve through the same `StoreField::Map` checker
    // dispatch as any other storage map — entry ops (`put`/`remove`/
    // `contains`/`size`/`get`) as `Callee::Store`, lifted query ops (e.g.
    // `parTraverse`) as `Callee::Query` — so both variants are matched here,
    // mirroring `lower_store_map_call`'s own guard.
    // `agent_held_map_frame` stays: it answers which *kind* of store field
    // this is (held vs ordinary), not the shadowing question `Callee`
    // already settles.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && let Some(f_ts) = cx.agent_held_map_frame(&id.name).cloned()
    {
        let var = cx.agent_store_var().to_string();
        let m = format!("{var}.{}", id.name);
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        return Some(match method.name.as_str() {
            // #3261: `put` over a key that already holds a connection closes the
            // displaced one first, as `remove` does, so a held connection cannot
            // be dropped unclosed. Re-putting the same connection under its own
            // key (the same connection id) leaves it open.
            "put" => format!(
                "(async () => {{ const __k = String({0}); const __new = __connIdOf({1}); const __old = await {m}.get(__k); if (__old !== undefined && __old !== __new) {{ const __c = __resolveConnection<{f_ts}>(this.state, __old); if (__c.tag === \"Some\") {{ await __c.value.close(); }} }} await {m}.put(__k, __new); return undefined; }})()",
                a[0], a[1]
            ),
            "remove" => format!(
                "(async () => {{ const __k = String({0}); const __cid = await {m}.get(__k); if (__cid !== undefined) {{ const __c = __resolveConnection<{f_ts}>(this.state, __cid); if (__c.tag === \"Some\") {{ await __c.value.close(); }} await {m}.remove(__k); }} return undefined; }})()",
                a[0]
            ),
            "contains" => format!("{m}.has(String({}))", a[0]),
            "size" => format!("{m}.size()"),
            "get" => format!(
                "(() => {{ const __cid = {m}.get(String({0})); return __cid !== undefined ? __resolveConnection<{f_ts}>(this.state, __cid) : None; }})()",
                a[0]
            ),
            // Any non-entry op is a lazy query lifting the map into a scan over its
            // **resolved** connections (the present ones — a connId whose socket has
            // closed drops out).
            _ => {
                note_par_fan_out(cx, method);
                let other_elem_ts = args
                    .first()
                    .map(|_| join_other_elem_ts(args, cx))
                    .unwrap_or_default();
                lower_query_by_name(
                    format!(
                        "{m}.values().flatMap((__cid) => {{ const __c = __resolveConnection<{f_ts}>(this.state, __cid); return __c.tag === \"Some\" ? [__c.value] : []; }})"
                    ),
                    method,
                    &a,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &QueryShape {
                        elem_ts: &f_ts,
                        other_elem_ts: &other_elem_ts,
                        keyed: query_keyed(e, method, args, cx),
                        key_cmp: query_key_cmp(method, args, cx),
                        runtime_use: cx.runtime_use(),
                    },
                    tys,
                )
            }
            .unwrap_or_else(|| {
                format!("(/* unsupported held Map op {} */ undefined)", method.name)
            }),
        });
    }
    None
}

/// Lower a persisted `store Map[K, V]` operation, including the `@indexed` posting-list maintenance, or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_map_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let tys = cx.ty_render();
    // v0.82 (ADR 0110): a storage-`Map` operation — `<map>.<op>(…)` on a `store
    // Map[K, V]` field. Lowers to an entry op over `__state.<map>` (a
    // `Record<string, V>`): mutating the working record (`put`/`remove`/`update`/
    // `upsert`) or reading it (`get`/`contains`/`size`). The surface op is
    // `Effect`-typed and awaited with `<-`, but the working-record mutation is
    // synchronous (the durable write is the single end-of-handler flush), so an
    // awaited expression suffices. `update` on an absent key throws (a fault →
    // nothing commits).
    //
    // P6.21 (partial, continued): the receiver test reads the checker's own
    // `Callee::Store`/`Callee::Query` (P6.0) instead of `!cx.is_local(&id.name)`
    // — a real instance of R6.5's name-matched-receiver defect class, the same
    // one `body_writes_state`/#1196 already closed for write-detection.
    // `cx.is_agent_store_map` stays: it answers a different question (which
    // *kind* of store field this is, needed to pick this branch over
    // Set/Cache/Log/Cell's own), not the shadowing question `Callee` already
    // settles.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && cx.is_agent_store_map(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let m = format!("{var}.{}", id.name);
        // v0.93 (ADR 0118): the value-record fields this map is `@indexed(by:)` on.
        let idx_fields: Vec<String> = cx.agent_store_index_fields(&id.name);
        // Route an equality `filter` on an indexed field to a posting-list lookup
        // (`<map>.byIndex(f, v)`) instead of a full scan. The lookup reads the staged
        // handle, so it stays read-your-writes (the index is maintained by every put).
        if method.name == "filter"
            && let [arg] = args
            && let ExprKind::Lambda(lam) = &arg.kind
            && let Some(routed) = pre.absorb_opt(route_indexed_filter(&m, &idx_fields, lam, cx))
        {
            return Some(routed);
        }
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        // A key may be a number or a branded value: the handle is keyed by its string.
        let k = a
            .first()
            .map(|x| format!("String({x})"))
            .unwrap_or_default();
        return Some(match method.name.as_str() {
            // #3254: the map is an insertion-ordered handle, and an `@indexed` map's handle
            // keeps its own posting lists exact on every `put`/`remove` (re-indexing on
            // last-write-wins), so the mutators are the plain ones.
            //
            // #3261: on the bundle, `put` over a key that already holds a `Connection`
            // closes the displaced `TestConnection` first, as `remove` does and as the
            // Workers build does; re-putting the same connection under its own key leaves
            // it open (compared by identity).
            "put"
                if cx
                    .agent_store_map_value_ts(&id.name)
                    .is_some_and(|v| v.starts_with("Connection<")) =>
            {
                format!(
                    "(async () => {{ const __k = {0}; const __new = {1}; const __old = await {m}.get(__k); if (__old !== undefined && __old !== __new) {{ await __old.close(); }} await {m}.put(__k, __new); return undefined; }})()",
                    k, a[1]
                )
            }
            // #3189: every handle call is awaited, so a handle backed by storage serves the
            // same lowering as the in-memory one (which returns its values at once).
            "put" => format!(
                "(async () => {{ await {m}.put({}, {}); return undefined; }})()",
                k, a[1]
            ),
            // #3224: on the bundle target a held `store Map[K, Connection]` is an
            // ordinary in-memory map of `TestConnection`s (the Workers build
            // persists connection ids instead and takes `lower_store_held_map_call`), so `remove` must still honour the §2.9 contract that it
            // removes *and closes*: close the connection the entry holds, then
            // delete it, as the Workers lowering does.
            "remove"
                if cx
                    .agent_store_map_value_ts(&id.name)
                    .is_some_and(|v| v.starts_with("Connection<")) =>
            {
                format!(
                    "(async () => {{ const __k = {0}; const __o = await {m}.get(__k); if (__o !== undefined) {{ await __o.close(); await {m}.remove(__k); }} return undefined; }})()",
                    k
                )
            }
            "remove" => format!(
                "(async () => {{ await {m}.remove({}); return undefined; }})()",
                k
            ),
            "contains" => format!("{m}.has({})", k),
            "size" => format!("{m}.size()"),
            "get" => format!(
                "(async () => {{ const __k = {0}; return __bynkThen({m}.has(__k), (__h) => __h ? __bynkThen({m}.get(__k), (__v) => Some(__v!)) : None); }})()",
                k
            ),
            "update" => format!(
                "(async () => {{ await {m}.modify({0}, (__c, __p) => {{ if (!__p) {{ throw new globalThis.Error(\"Map.update: key absent\"); }} return ({1})(__c!); }}); return undefined; }})()",
                k, a[1]
            ),
            "upsert" => format!(
                "(async () => {{ await {m}.modify({0}, (__c, __p) => ({2})(__p ? __c! : ({1}))); return undefined; }})()",
                k, a[1], a[2]
            ),
            // v0.91 (ADR 0119): any non-entry op is a lazy query that lifts the
            // map into a scan over its values (`Object.values`).
            //
            // Review of #1460: `elem_ts` is this map's own value type `V`
            // (`agent_store_map_value_ts`, mirroring `f_ts` above's held-map
            // equivalent) — falls back to `unknown` only for an
            // already-diagnosed program the checker recorded no type for
            // (`join_other_elem_ts`'s own identical fallback posture), not a
            // real case on a certified one: this arm is only reached when
            // `cx.is_agent_store_map` already matched, so `map_values` always
            // has an entry for `id.name` here.
            _ => {
                let elem_ts = cx
                    .agent_store_map_value_ts(&id.name)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                note_par_fan_out(cx, method);
                let other_elem_ts = args
                    .first()
                    .map(|_| join_other_elem_ts(args, cx))
                    .unwrap_or_default();
                lower_query_by_name(
                    format!("{m}.values()"),
                    method,
                    &a,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &QueryShape {
                        elem_ts: &elem_ts,
                        other_elem_ts: &other_elem_ts,
                        keyed: query_keyed(e, method, args, cx),
                        key_cmp: query_key_cmp(method, args, cx),
                        runtime_use: cx.runtime_use(),
                    },
                    tys,
                )
                .unwrap_or_else(|| format!("(/* unsupported Map op {} */ undefined)", method.name))
            }
        });
    }
    None
}

/// Lower a persisted `store Set[T]` operation, or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_set_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.83: a storage-`Set` operation — `<set>.<op>(…)` on a `store Set[T]` field.
    // Lowers to an entry op over `__state.<set>` (a `Record<string, boolean>`):
    // `add`/`remove` mutate the working record, `contains`/`size` read it.
    //
    // P6.21 (partial, continued): reads `Callee::Store` (P6.0) instead of
    // `!cx.is_local(&id.name)` — see `lower_store_map_call` for the
    // full reasoning. `Set` never gets `Callee::Query` (checker.rs's own
    // `StoreField::Set` arm records only `Store`), unlike `Map`/`Log`.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Store { .. }))
        && cx.is_agent_store_set(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let s = format!("{var}.{}", id.name);
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        let k = a
            .first()
            .map(|x| format!("String({x})"))
            .unwrap_or_default();
        return Some(match method.name.as_str() {
            "add" => format!(
                "(async () => {{ await {s}.add({}); return undefined; }})()",
                k
            ),
            "remove" => format!(
                "(async () => {{ await {s}.remove({}); return undefined; }})()",
                k
            ),
            "contains" => format!("{s}.has({})", k),
            "size" => format!("{s}.size()"),
            other => format!("(/* unsupported Set op {other} */ undefined)"),
        });
    }
    None
}

/// Lower a persisted `store Cache[K, V]` operation (TTL expiry against the injected `Clock`), or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_cache_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.87 (ADR 0113): a storage-`Cache` operation — `<cache>.<op>(…)` on a
    // `store Cache[K, V]` field. Lowers to an entry op over `__state.<cache>` (a
    // `Record<string, { v, exp }>`) that applies TTL expiry against the injected
    // `Clock`: every op but `remove` reads `now()` (an awaited `Effect`), so the
    // op is an async IIFE. `put`/`update`/`upsert` stamp `exp = now() + ttl`;
    // `get`/`contains`/`size` treat an entry past `exp` as absent.
    //
    // P6.21 (partial, continued): reads `Callee::Store` instead of
    // `!cx.is_local(&id.name)` — see `lower_store_map_call`. `Cache`
    // never gets `Callee::Query`, same as `Set`.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Store { .. }))
        && let Some(ttl) = cx.agent_store_cache_ttl(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let c = format!("{var}.{}", id.name);
        // Cache reaping (#3189): note the clock the op read against the state the handler
        // commits, so the commit can delete the entries that clock says are expired.
        let now = format!(
            "__noteClock({var}, await {}.Clock.now())",
            cx.cap_deps_expr()
        );
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        let k = a
            .first()
            .map(|x| format!("String({x})"))
            .unwrap_or_default();
        return Some(match method.name.as_str() {
            "remove" => format!(
                "(async () => {{ await {c}.remove({}); return undefined; }})()",
                k
            ),
            "put" => format!(
                "(async () => {{ const __now = {now}; await {c}.put({0}, {{ v: {1}, exp: __now + {ttl} }}); return undefined; }})()",
                k, a[1]
            ),
            "get" => format!(
                "(async () => {{ const __now = {now}; const __e = await {c}.get({0}); return (__e !== undefined && __e.exp > __now) ? Some(__e.v) : None; }})()",
                k
            ),
            "contains" => format!(
                "(async () => {{ const __now = {now}; const __e = await {c}.get({0}); return __e !== undefined && __e.exp > __now; }})()",
                k
            ),
            "size" => {
                format!(
                    "(async () => {{ const __now = {now}; return await {c}.liveSize(__now); }})()"
                )
            }
            "update" => format!(
                "(async () => {{ const __now = {now}; const __k = {0}; await {c}.modify(__k, (__e) => {{ if (!(__e !== undefined && __e.exp > __now)) {{ throw new globalThis.Error(\"Cache.update: key absent\"); }} return {{ v: ({1})(__e.v), exp: __now + {ttl} }}; }}); return undefined; }})()",
                k, a[1]
            ),
            "upsert" => format!(
                "(async () => {{ const __now = {now}; const __k = {0}; await {c}.modify(__k, (__e) => {{ const __cur = (__e !== undefined && __e.exp > __now) ? __e.v : ({1}); return {{ v: ({2})(__cur), exp: __now + {ttl} }}; }}); return undefined; }})()",
                k, a[1], a[2]
            ),
            other => format!("(/* unsupported Cache op {other} */ undefined)"),
        });
    }
    None
}

/// Lower a persisted `store Log[T]` operation (append, time windows, the query vocabulary), or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_log_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let tys = cx.ty_render();
    // v0.95 (ADR 0121): a storage-`Log` operation — `<log>.<op>(…)` on a
    // `store Log[T]` field (an array of `{ t, v }`). `append` stamps the clock
    // and pushes (pruning past `@retain`); the time-window roots and the general
    // query vocabulary lower to a lazy pipeline over the entry values.
    //
    // P6.21 (partial, continued): reads `Callee::Store`/`Callee::Query`
    // instead of `!cx.is_local(&id.name)` — see the storage-`Map` branch
    // above. `Log` gets both variants, same as `Map` (`append` is `Store`;
    // the window-root/query vocabulary is `Query`, checker.rs's own
    // `StoreField::Log` arm).
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Store { .. } | Callee::Query { .. })
        )
        && let Some(retain) = cx.agent_store_log_retain(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let g = format!("{var}.{}", id.name);
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        if method.name == "append" {
            let now = format!("await {}.Clock.now()", cx.cap_deps_expr());
            let prune = match retain {
                Some(ms) => format!(" await {g}.prune(__now - {ms});"),
                None => String::new(),
            };
            return Some(format!(
                "(async () => {{ const __now = {now}; await {g}.append({0}, __now);{prune} return undefined; }})()",
                a[0]
            ));
        }
        // The values feeding the general query pipeline.
        let values = format!("{g}.values()");
        let thunk = |body: String| format!("(() => {body})");
        return Some(match method.name.as_str() {
            // Time-window roots → a `Query` thunk over the windowed values.
            "since" => thunk(format!("{g}.since({0})", a[0])),
            "before" => thunk(format!("{g}.before({0})", a[0])),
            "between" => thunk(format!("{g}.between({0}, {1})", a[0], a[1])),
            "recent" => thunk(format!("{g}.recent({0})", a[0])),
            "reversed" => thunk(format!("{g}.reversed()")),
            // The general query vocabulary over the entry values.
            //
            // Review of #1460: `elem_ts` is this log's own element type `T`
            // (`agent_store_log_value_ts`) — same construction and fallback
            // posture as the plain-Map scan above.
            _ => {
                let elem_ts = cx
                    .agent_store_log_value_ts(&id.name)
                    .cloned()
                    .unwrap_or_else(|| "unknown".to_string());
                note_par_fan_out(cx, method);
                let other_elem_ts = args
                    .first()
                    .map(|_| join_other_elem_ts(args, cx))
                    .unwrap_or_default();
                lower_query_by_name(
                    values,
                    method,
                    &a,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &QueryShape {
                        elem_ts: &elem_ts,
                        other_elem_ts: &other_elem_ts,
                        keyed: query_keyed(e, method, args, cx),
                        key_cmp: query_key_cmp(method, args, cx),
                        runtime_use: cx.runtime_use(),
                    },
                    tys,
                )
                .unwrap_or_else(|| format!("(/* unsupported Log op {} */ undefined)", method.name))
            }
        });
    }
    None
}

/// Lower a `store Cell[T]` update, or `None` when the receiver is not that kind of store
/// field. Moved out of `lower_method_call` (#3167) unchanged.
fn lower_store_cell_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.98 (ADR 0125): a storage-`Cell` operation — `<cell>.update(f)` on a
    // `store Cell[T]` field. Lowers to a staged read-modify-write over
    // `__state.<cell>` (`Map.update`'s lowering minus the key-absent guard — a
    // cell is always present, so there is no fault path). The mutation is
    // synchronous against the working state (read-your-writes); the single
    // end-of-handler `commitState` flush runs the invariant gate before the
    // durable write, exactly as `:=` does.
    // Note: unlike the other store kinds, a `Cell` field is deliberately bound
    // into checker scope (self_scope) as a bare local of its element type, so
    // the checker always resolves a same-named identifier to the cell — no
    // `is_local` guard belongs here without also changing that scope
    // construction, or the emitter would disagree with what the checker typed.
    if let ExprKind::Ident(id) = &receiver.kind
        && cx
            .agent_store_cells()
            .is_some_and(|(_, cells)| cells.contains(&id.name))
    {
        let var = cx.agent_store_var().to_string();
        let n = format!("{var}.{}", id.name);
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        return Some(match method.name.as_str() {
            "update" => format!("(() => {{ {n} = ({0})({n}); return undefined; }})()", a[0]),
            other => format!("(/* unsupported Cell op {other} */ undefined)"),
        });
    }
    None
}

/// `within`, `race` and `detach` on an `Effect[T]` (#2683, #2711, #3437). `None`
/// when the call's shape is not one the kernel method lowers (the caller falls
/// through to the instance-call lowering).
fn lower_effect_kernel_call(
    e: &Expr,
    receiver: &Expr,
    op: EffectOp,
    args: &[Expr],
    recv_ty: &Ty,
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let tys = cx.ty_render();
    match op {
        // #2683: `e.within(d)` races the effect against a deadline
        // and hands the abandoned effect to the handler's execution
        // context.
        EffectOp::Within if args.len() == 1 => {
            cx.note_within();
            let recv = pre.lower(receiver, cx);
            let after = pre.lower(&args[0], cx);
            return Some(format!("__bynkWithin({recv}, {after}, {})", cx.exec_expr()));
        }
        // #2711: `a.race(b)` takes the first side to succeed and
        // hands the rest to the same execution context, uncancelled.
        EffectOp::Race if args.len() == 1 => {
            cx.note_race();
            // A chain `a.race(b).race(c)` lowers to one
            // `__bynkRace([a, b, c], …)`: nesting would let `c` beat
            // `a` in a same-instant tie, as the inner race settles a
            // microtask later, and ties go to source order.
            let mut operands: Vec<&Expr> = vec![&args[0]];
            let mut head: &Expr = receiver;
            while let ExprKind::MethodCall {
                receiver: inner,
                args: a,
                ..
            } = &head.kind
                && matches!(
                    cx.commons().callee(head.id),
                    Some(Callee::Kernel {
                        op: KernelOp::Effect(EffectOp::Race),
                        ..
                    })
                )
                && a.len() == 1
                && matches!(
                    cx.commons().expr_ty(inner.id).as_deref(),
                    Some(Ty::Effect(_))
                )
            {
                operands.push(&a[0]);
                head = inner;
            }
            operands.push(head);
            operands.reverse();
            let lowered: Vec<String> = operands.into_iter().map(|o| pre.lower(o, cx)).collect();
            return Some(format!(
                "__bynkRace([{}], {})",
                lowered.join(", "),
                cx.exec_expr()
            ));
        }
        // #3437: `e.detach()` hands the already-running effect to
        // the execution context and reports a failure (a fault, or
        // an `Err` when the payload is a `Result`). The static type
        // picks the route: only a `Result` payload names its error
        // type to the helper.
        EffectOp::Detach if args.is_empty() => {
            cx.note_detach();
            let recv = pre.lower(receiver, cx);
            let site = cx
                .assert_loc()
                .map(|_| expect_location(cx, e.span.start))
                .or_else(|| cx.fault_location(e.span.start))
                .map(|l| crate::emitter::escape_ts_string(&l))
                .unwrap_or_else(|| format!("offset {}", e.span.start));
            let err_type = match &recv_ty {
                Ty::Effect(inner) => match &*tys.get(*inner) {
                    Ty::Result(_, err) => Some(crate::emitter::escape_ts_string(
                        &err.display(tys.types()).to_string(),
                    )),
                    _ => None,
                },
                _ => None,
            };
            let tail = err_type.map(|t| format!(", \"{t}\"")).unwrap_or_default();
            return Some(format!(
                "__bynkDetach({recv}, {}, \"{site}\"{tail})",
                cx.exec_expr()
            ));
        }
        EffectOp::Within | EffectOp::Race | EffectOp::Detach => {}
    }
    None
}

/// A built-in method call the checker resolved to a typed [`KernelOp`] (#3164):
/// the one place the emitter turns a kernel operation into TypeScript, matching
/// on the receiver kind and the operation. `None` when the operation has no
/// inline lowering here, so the caller falls through to the UFCS lowering; any
/// hoisted statements the attempt produced stay in `pre`. The `List` kernel
/// returns a node when its operands are nodes (1A.18a); every other family is
/// still text, wrapped here as `Err` (`// 1A:` at its arm below).
fn lower_kernel_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    let tys = cx.ty_render();
    // v0.20b: built-in kernel methods on the collection types, dispatched on the
    // receiver's checked type. Emitted inline (typed IIFEs / spreads) — no runtime
    // imports, so files that never touch collections emit byte-identically to v0.20a.
    // The checker resolved this call to a typed `KernelOp` (#3164), so each arm
    // matches on the resolved operation, not on the method's spelling, and the
    // receiver's type arguments are read off its checked type.
    if let Some(Callee::Kernel {
        op: kop,
        recv: kernel_recv,
    }) = cx.commons().callee(e.id).cloned()
    {
        let recv_ty = (*tys.get(kernel_recv)).clone();
        // The receiver's checked type disagrees with the resolved op only if
        // the checker and the emitter have drifted.
        let mismatch = |kop: KernelOp| -> ! {
            panic!("bynk internal error (#3164): kernel op {kop:?} on a mismatched receiver type")
        };
        // 1A: every arm below but `List` is text until its own family slice (the
        // track's `1A.query`, `1A.stream`, `1A.map`, `1A.set`, `1A.option`,
        // `1A.result`, `1A.string` and `1A.numeric` rows).
        match kop {
            KernelOp::List(op) => {
                let Ty::List(elem) = &recv_ty else {
                    mismatch(kop)
                };
                if let Some(built) = lower_list_kernel(e, receiver, op, args, *elem, cx, pre) {
                    return Some(built);
                }
            }
            // v0.91 (ADR 0119): a chained op on a lazy `Query` — the source is
            // the receiver thunk, invoked (`(recv)()`).
            KernelOp::Query(op) => {
                let Ty::Query(elem) = &recv_ty else {
                    mismatch(kop)
                };
                let elem_ts = ts_ty(*elem, tys);
                let recv = pre.lower(receiver, cx);
                let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
                let result_ty = cx.commons().expr_types.get(&e.id).map(|te| te.ty);
                note_par_fan_out(cx, method);
                let other_elem_ts = args
                    .first()
                    .map(|_| join_other_elem_ts(args, cx))
                    .unwrap_or_default();
                if let Some(s) = lower_query_method(
                    format!("({recv})()"),
                    op,
                    &a,
                    result_ty,
                    &QueryShape {
                        elem_ts: &elem_ts,
                        other_elem_ts: &other_elem_ts,
                        keyed: query_keyed(e, method, args, cx),
                        key_cmp: query_key_cmp(method, args, cx),
                        runtime_use: cx.runtime_use(),
                    },
                    tys,
                ) {
                    return Some(Err(s));
                }
            }
            // v0.100: a chained op on a `Stream` — the receiver already *is* an
            // async iterable, so it is the source directly. Emitted inline as
            // async-generator IIFEs (builders) / an async drain (`collect`).
            KernelOp::Stream(op) => {
                let recv = pre.lower(receiver, cx);
                let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
                if let Some(s) = lower_stream_method(recv, op, &a) {
                    return Some(Err(s));
                }
            }
            // #3051: a case's recording `TestConnection[F]`.
            KernelOp::TestConnection(op) => {
                let recv = pre.lower(receiver, cx);
                let s = match op {
                    TestConnectionOp::Sent => format!("[...({recv}).sent]"),
                    TestConnectionOp::Closed => format!("({recv}).closed"),
                    TestConnectionOp::Connection => format!("({recv}).mint()"),
                };
                return Some(Err(s));
            }
            // v0.102: the held-resource operations on a `Connection[F]` lower to
            // method calls on the runtime `Connection` object — `send(frame)` and
            // `close()`. The linearity pass has already verified ownership.
            KernelOp::Connection(op) => {
                let recv = pre.lower(receiver, cx);
                let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
                return Some(Err(format!("({recv}).{}({})", op.name(), a.join(", "))));
            }
            KernelOp::Map(op) => {
                let Ty::Map(key, val) = &recv_ty else {
                    mismatch(kop)
                };
                if let Some(s) =
                    pre.absorb_opt(lower_map_kernel(receiver, op, args, *key, *val, cx))
                {
                    return Some(Err(s));
                }
            }
            // K6a (#2596): the `Set` kernel.
            KernelOp::Set(op) => {
                let Ty::Set(elem) = &recv_ty else {
                    mismatch(kop)
                };
                if let Some(s) = pre.absorb_opt(lower_set_kernel(receiver, op, args, *elem, cx)) {
                    return Some(Err(s));
                }
            }
            // v0.21: the numeric kernel. `toFloat` is the identity
            // at runtime (the Int/Float distinction is erased);
            // the four `Float -> Int` roundings map onto `Math.*`.
            // v0.22a extends it (abs/min/max/clamp, isNaN/isFinite).
            KernelOp::Int(op) => {
                if let Some(s) = pre.absorb_opt(lower_int_kernel(receiver, op, args, cx)) {
                    return Some(Err(s));
                }
            }
            KernelOp::Float(op) => {
                if let Some(s) = pre.absorb_opt(lower_float_kernel(receiver, op, args, cx)) {
                    return Some(Err(s));
                }
            }
            // v0.86 (ADR 0112): the `Duration` kernel. `toMillis` is the identity
            // at runtime (a `Duration` already *is* its milliseconds); `toString`
            // renders the number.
            KernelOp::Duration(op) => {
                if let Some(s) = pre.absorb_opt(lower_duration_kernel(receiver, op, args, cx)) {
                    return Some(Err(s));
                }
            }
            // v0.90 (ADR 0114): the `Instant` kernel. `toEpochMillis` is the
            // identity (an `Instant` lowers to its epoch millis); `toString`
            // renders the number.
            KernelOp::Instant(op) => {
                if let Some(s) = pre.absorb_opt(lower_instant_kernel(receiver, op, args, cx)) {
                    return Some(Err(s));
                }
            }
            // #3277: a cookie rides beside an `HttpResult` as a `cookies` list the
            // router turns into one `Set-Cookie` each. `clearCookie` is the cookie
            // sent again, empty and already expired, under the same name, `Domain`
            // and `Path`, which is what removes it.
            KernelOp::HttpResult(op) => {
                cx.note_http_result_cookie();
                let recv = pre.lower(receiver, cx);
                let args: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
                let arg = args.first().cloned().unwrap_or_default();
                let expr = match op {
                    bynk_check::kernel_op::HttpResultOp::WithCookie => format!(
                        "((__r: any) => ({{ ...__r, cookies: [...(__r.cookies ?? []), ({arg})] }}))({recv})"
                    ),
                    bynk_check::kernel_op::HttpResultOp::ClearCookie => format!(
                        "((__r: any) => ({{ ...__r, cookies: [...(__r.cookies ?? []), {{ ...({arg}), value: \"\", maxAge: Some(0), expires: Some(0) }}] }}))({recv})"
                    ),
                    bynk_check::kernel_op::HttpResultOp::Cookies => {
                        format!("((({recv}) as any).cookies ?? [])")
                    }
                };
                return Some(Err(expr));
            }
            // #2341 (decision A): an HTTP handler's `headers` is a runtime
            // object with these very methods, each answering an `Option`.
            KernelOp::RequestHeaders(_)
            | KernelOp::RequestQuery(_)
            | KernelOp::RequestCookies(_)
            | KernelOp::TokenClaims(_) => {
                let recv = pre.lower(receiver, cx);
                let args: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
                return Some(Err(format!("({recv}).{}({})", kop.name(), args.join(", "))));
            }
            // v0.110 (ADR 0142): the `Bytes` kernel. `length` is the octet
            // count; `toBase64` encodes; `decodeUtf8` is a guarded UTF-8 decode
            // returning `Option`.
            KernelOp::Bytes(op) => {
                if let Some(s) = pre.absorb_opt(lower_bytes_kernel(receiver, op, args, cx)) {
                    return Some(Err(s));
                }
            }
            // #2622: the `Decimal` kernel.
            KernelOp::Decimal(op) => {
                if let Some(s) = pre.absorb_opt(lower_decimal_kernel(receiver, op, args, cx)) {
                    return Some(Err(s));
                }
            }
            // v0.22a: the string kernel (ADR 0046).
            KernelOp::String(op) => {
                if let Some(s) = pre.absorb_opt(lower_string_kernel(receiver, op, args, cx)) {
                    return Some(Err(s));
                }
            }
            // v0.22a: Option/Result combinators (ADR 0048).
            KernelOp::Option(op) => {
                let Ty::Option(inner) = &recv_ty else {
                    mismatch(kop)
                };
                if let Some(s) =
                    pre.absorb_opt(lower_option_kernel(e, receiver, op, args, *inner, cx))
                {
                    return Some(Err(s));
                }
            }
            KernelOp::Result(op) => {
                let Ty::Result(ok, err) = &recv_ty else {
                    mismatch(kop)
                };
                if let Some(s) =
                    pre.absorb_opt(lower_result_kernel(e, receiver, op, args, *ok, *err, cx))
                {
                    return Some(Err(s));
                }
            }
            // #2683 / #2711 / #3437: `within`, `race` and `detach`, on every `Effect[T]`.
            KernelOp::Effect(op) => {
                if let Some(s) = lower_effect_kernel_call(e, receiver, op, args, &recv_ty, cx, pre)
                {
                    return Some(Err(s));
                }
            }
            // §2.8.3: the `Effect[Result[T, E]]` combinators — `mapOk`/`mapErr`/
            // `flatMapOk`/`flatMapErr`. Lowered as an `async` IIFE that awaits the
            // receiver `Promise<Result<…>>` and rebuilds the transformed Result.
            KernelOp::EffectResult(op) => {
                let Ty::Effect(inner) = &recv_ty else {
                    mismatch(kop)
                };
                if let Ty::Result(ok, err) = &*tys.get(*inner)
                    && let Some(s) = pre.absorb_opt(lower_effect_result_kernel(
                        e, receiver, op, args, *ok, *err, cx,
                    ))
                {
                    return Some(Err(s));
                }
            }
        }
    }
    None
}

/// The instance-call fallback (UFCS lowering, #3167): the receiver becomes the
/// first argument of the namespace function for the receiver's checked type. The
/// result is the call expression; hoisted statements go to `pre`.
fn lower_ufcs_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Built {
    let tys = cx.ty_render();
    // Instance call: UFCS lowering with the receiver as first arg. The
    // namespace comes from the receiver's checked type. Every method call the
    // checker accepts has one: a test-service call's outcome is typed by its
    // handler (#1934), where it once reached this placeholder and failed only
    // at `tsc` (#1931). The placeholder stays reachable for a program that
    // failed checking, which is still lowered and whose output is discarded,
    // so it is not a panic.
    let known_ns = cx.receiver_namespace(receiver);
    let ns = known_ns
        .clone()
        .unwrap_or_else(|| "/* unknown */".to_string());
    let recv = pre.lower_node(receiver, cx);
    let mut all = vec![recv];
    for a in args {
        all.push(pre.lower_node(a, cx));
    }
    // Constrained generics slice 1: the call's comparator witnesses follow the
    // ordinary arguments.
    let mut needs_exec = false;
    if let Some(Callee::Method(decl, witnesses)) = cx.commons().callees.get(&e.id) {
        all.extend(
            cx.witness_args(decl, witnesses)
                .into_iter()
                .map(LoweredExpr::Node),
        );
        // #2719: a method in the needs-context set takes the context last.
        if cx.commons().exec_plan.fn_needs(decl) {
            all.push(LoweredExpr::Node(cx.exec_node()));
            needs_exec = true;
        }
    }
    // #2795: explicit type arguments on a generic instance method are kept, the
    // receiver's own parameters first (the emitted `<A, U>` order), so a call
    // with no argument of type `U` still types as written. The method's own
    // arguments are what the author wrote; a receiver argument is spelled only
    // when it is a base or built-in type, which is in scope everywhere, and is
    // `any` otherwise (a type the calling module may never import, or a type
    // variable that may not be this function's). The checker has already typed
    // the call, so `any` in a receiver slot loses nothing.
    let explicit = if type_args.is_empty() {
        String::new()
    } else {
        let recv_args: Option<Vec<TyId>> =
            cx.commons()
                .expr_types
                .get(&receiver.id)
                .and_then(|te| match &*tys.get(te.ty) {
                    Ty::Named { args, .. } => Some(args.clone()),
                    _ => None,
                });
        match recv_args {
            Some(recv_args) => {
                let parts: Vec<String> = recv_args
                    .iter()
                    .map(|a| {
                        if ts_spelling_in_scope_everywhere(*a, tys.types()) {
                            ty_to_type_ref(*a, tys.types())
                                .map(|r| ts_type_ref(&r))
                                .unwrap_or_else(|| "any".to_string())
                        } else {
                            "any".to_string()
                        }
                    })
                    .chain(type_args.iter().map(ts_type_ref))
                    .collect();
                format!("<{}>", parts.join(", "))
            }
            None => String::new(),
        }
    };
    // 1A: `bynk_ts` `Call` has no `type_args` field (#2795's explicit type
    // arguments have no node home), and the `/* unknown */` namespace is a
    // placeholder for a program that failed checking, not an identifier: both
    // stay text.
    let built = match known_ns {
        Some(ns) if explicit.is_empty() => {
            let callee = bynk_ts::TsExpr::Member {
                object: Box::new(ident_node(&ns)),
                property: method.name.clone(),
            };
            call_built(callee, all)
        }
        _ => Err(format!(
            "{ns}.{}{explicit}({})",
            method.name,
            all.into_iter()
                .map(LoweredExpr::into_text)
                .collect::<Vec<_>>()
                .join(", ")
        )),
    };
    if needs_exec {
        settle_built(built, cx)
    } else {
        built
    }
}

/// #2795: whether a type's TypeScript spelling is in scope in every emitted
/// module: base types, `Unit` and the built-in containers over them.
fn ts_spelling_in_scope_everywhere(t: TyId, tys: &Arc<Types>) -> bool {
    match &*tys.get(t) {
        Ty::Base(_) | Ty::Unit => true,
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => ts_spelling_in_scope_everywhere(*a, tys),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            ts_spelling_in_scope_everywhere(*a, tys) && ts_spelling_in_scope_everywhere(*b, tys)
        }
        _ => false,
    }
}

/// v0.22b: the typed JSON codec (ADR 0045). `encode` dispatches to the
/// module-local `serialise_*` helpers + `JSON.stringify`; `decode[T]` to
/// `JSON.parse` + `deserialise_*`, mapping a parse failure to a `Malformed`
/// DecodeError and a BoundaryError to the uniform `kind`/`path`/`message`
/// record (ADR 0047). Returns `None` when the receiver is not `Json` or the
/// shape does not match, so the dispatcher falls through.
///
/// R8.14 (Arc D, P7.15 review): reads the same `Callee::Intrinsic { ns: JSON,
/// .. }` this call's own root gets classified under in
/// `collect_json_codec_roots` (`bynk-emit/src/emitter.rs`) — not the bare
/// `id.name == JSON` receiver-name match this function used before. The two
/// must agree on what a codec call *is*: `collect_json_codec_roots` decides
/// whether a `serialise_T`/`deserialise_T` helper gets emitted at all, this
/// function decides whether a call site references one. Read using the
/// syntactic match alone, a `Json` shadowed by a local value/type would have
/// disagreed with the (already `Callee`-based) seed collector — no helper
/// emitted, but a reference to one generated anyway, a `tsc` "cannot find
/// name" strictly worse than the syntactic match's own pre-existing
/// wrong-but-well-formed output.
fn lower_json_codec_call(
    e: &Expr,
    // Unused now that the guard below reads the checker's own resolved
    // `Callee::Intrinsic` instead of matching the receiver's own shape —
    // kept in the signature for uniformity with this dispatcher's sibling
    // `lower_*_call` functions, which all share this exact parameter list.
    _receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    // #3214: `Yaml.decode[T](text)` is `Json.decode[T]` with the YAML parser in
    // place of `JSON.parse`; the typed decode and its errors are the same.
    if matches!(
        cx.commons().callees.get(&e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == YAML
    ) && args.len() == 1
        && method.name == "decode"
        && let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
        && let Some(decoder) = json_decoder_fn(t, DecodeSource::Yaml, cx)
    {
        let arg = pre.lower(&args[0], cx);
        return Some(pre.finish_text(format!("({decoder})({arg})")));
    }
    // #3276 (decision G): `Form.decode[T](text)` reads the text as the router reads
    // a `@form` body, then decodes through `T`'s codec; `Form.encode(v)` writes the
    // form text such a route reads.
    if matches!(
        cx.commons().callees.get(&e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == FORM
    ) && args.len() == 1
    {
        if method.name == "decode"
            && let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
            && let Some(decoder) = json_decoder_fn(t, DecodeSource::Form, cx)
        {
            let arg = pre.lower(&args[0], cx);
            return Some(pre.finish_text(format!("({decoder})({arg})")));
        }
        if method.name == "encode" {
            cx.runtime_use().note_form_codec();
            // The checker admits only a concrete record of form fields, so the wire
            // value is always available; a miss is a compiler fault, not a user error.
            let wire = form_encode_wire(&mut pre, &args[0], cx)
                .expect("`Form.encode` checked a concrete record of form fields");
            // #3343: a field renamed by `@name` is written under its wire key.
            let table = cx
                .commons()
                .expr_types
                .get(&args[0].id)
                .map(|te| te.ty)
                .and_then(|ty| ty_to_type_ref(ty, cx.commons().tys()))
                .and_then(|tref| {
                    super::workers_entry::renamed_specs_source(&tref, &cx.commons().types)
                })
                .map(|t| format!(", {t}"))
                .unwrap_or_default();
            return Some(pre.finish_text(format!("__formString({wire}{table})")));
        }
    }
    if matches!(
        cx.commons().callees.get(&e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == JSON
    ) && args.len() == 1
    {
        if matches!(method.name.as_str(), "encode" | "encodePlain")
            && let Some(text) =
                json_encode_text(&mut pre, &args[0], cx, method.name == "encodePlain")
        {
            return Some(pre.finish_text(text));
        }
        if matches!(method.name.as_str(), "decode" | "decodeSql")
            && let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
            && let Some(decoder) = json_decoder_fn(
                t,
                if method.name == "decodeSql" {
                    DecodeSource::Sql
                } else {
                    DecodeSource::Json
                },
                cx,
            )
        {
            let arg = pre.lower(&args[0], cx);
            return Some(pre.finish_text(format!("({decoder})({arg})")));
        }
    }
    let text: Option<String> = None;
    text.map(|expr| pre.finish_text(expr))
}

/// Which text front end feeds a typed decode.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DecodeSource {
    /// `Json.decode`: the text is JSON.
    Json,
    /// `Json.decodeSql` and the typed `D1` reads: JSON, with SQLite's `0`/`1` flags read as `Bool`.
    Sql,
    /// Hyperdrive H3 (#3320), the typed `Postgres` reads: JSON, with an `int8`'s
    /// text read at an `Int` field and a `timestamptz`'s text at an `Instant`.
    Pg,
    /// #3214, `Yaml.decode`: the text is YAML, parsed to the same JSON-shaped tree.
    Yaml,
    /// #3276, `Form.decode`: the text is a form body, read by the router's own rules.
    Form,
}

/// The arrow function that decodes text into `t`:
/// `((__s: string): Result<T, DecodeError> => …)`, for `Json.decode` /
/// `Json.decodeSql` / `Yaml.decode` (`source`) and for the typed `D1` reads
/// (#2865), which apply it to the rows `all`/`first` return. `None` when `t`
/// has no TS rendering.
fn json_decoder_fn(
    t: bynk_check::checker::TyId,
    source: DecodeSource,
    cx: &mut LowerCtx,
) -> Option<String> {
    let sql = source == DecodeSource::Sql;
    let tys = cx.ty_render();
    let rigid = super::codec::has_var(t, cx.commons());
    let tref = if rigid {
        // A target that mentions a type parameter decodes through the
        // codec witness; `tref` is then only a placeholder.
        Some(TypeRef::Unit(bynk_syntax::span::Span::default()))
    } else {
        ty_to_type_ref(t, tys.types())
    }?;
    // #917: in a test-scaffold module `T` may be foreign (owned by the
    // suite target or one of its `uses`, never declared locally), so its
    // TS type positions reach through the module's namespace qualifier —
    // empty everywhere else, where this renders identically to `ts_ty`.
    let ts = if cx.in_test_scaffold() && !rigid {
        let qual = cx.runtime_use().json_codec_qual();
        bynk_ts::print_type(&serialisation::qualified_ts_type(&tref, &qual))
    } else {
        ts_ty(t, tys)
    };
    if cx.records_codec_roots() {
        note_ground_roots(t, cx);
    }
    // #914: the wrapper below names `Result`, `JsonValue` and `DecodeError`
    // in its own signature and body, whichever arm the inner deserialiser
    // takes — including the delegating ones, which record nothing. A
    // module that curates its import list (the test-scaffold modules)
    // otherwise emits all three unimported.
    cx.runtime_use().note_json_codec();
    let des = if rigid {
        // 1A: witness printed for a text caller
        format!("{}.decode(__j, \"$\")", bynk_ts::print_expr(&cx.codec(t)))
    } else {
        bynk_ts::print_expr(&serialisation::deserialise_expr(
            &tref,
            "__j",
            "$",
            cx.runtime_use(),
        ))
    };
    let sql_bools = if sql {
        sql_bool_pass(t, cx)
    } else if source == DecodeSource::Pg {
        pg_row_pass(t, cx)
    } else {
        String::new()
    };
    // The parse step: `JSON.parse`, or (#3214) the YAML parser, whose failures
    // are already positioned `DecodeError`s.
    let parse = if source == DecodeSource::Form {
        cx.runtime_use().note_form_codec();
        let specs = form_specs_literal(t, cx)?;
        format!(
            "const __p = __formDecode(__s, {specs}); \
             if (__p.tag === \"Err\") return Err(__p.error); \
             const __j: __JsonValue = __p.value; "
        )
    } else if source == DecodeSource::Yaml {
        cx.runtime_use().note_yaml();
        "const __p = __bynkYamlParse(__s); \
         if (__p.tag === \"Err\") return Err(__p.error); \
         const __j: __JsonValue = __p.value; "
            .to_string()
    } else {
        "let __j: __JsonValue; \
         try { __j = globalThis.JSON.parse(__s) as __JsonValue; } \
         catch (__e) { return Err({ kind: \"Malformed\", path: \"$\", message: String(__e) }); } "
            .to_string()
    };
    Some(format!(
        "(__s: string): Result<{ts}, DecodeError> => {{ \
         {parse}\
         {sql_bools}const __r = {des}; \
         if (__r.tag === \"Ok\") return Ok(__r.value as {ts}); \
         const __be = __r.error; \
         return Err({{ kind: __be.kind, \
         path: (__be.kind === \"StructuralMismatch\" || __be.kind === \"RefinementViolation\") ? __be.path : \"$\", \
         message: __be.kind === \"StructuralMismatch\" ? `expected ${{__be.expected}}, got ${{String(__be.actual)}}` : __be.kind === \"RefinementViolation\" ? __be.violation.message : __be.details }}); }}"
    ))
}

/// #3276: the field table `Form.decode[T]` hands the form reader, as the TS array
/// literal of `{ name, kind, list, option }` the router's `@form` path builds. `None`
/// when `t` is not a record of the module's types.
fn form_specs_literal(t: bynk_check::checker::TyId, cx: &LowerCtx) -> Option<String> {
    use bynk_syntax::ast::TypeBody;
    let tys = cx.ty_render();
    let Ty::Named { name, .. } = &*tys.get(t) else {
        return None;
    };
    let decl = cx.commons().types.get(name.as_str())?;
    let TypeBody::Record(rec) = &decl.body else {
        return None;
    };
    let specs = super::workers_entry::record_field_specs(rec, &cx.commons().types, false);
    let items: Vec<String> = specs
        .iter()
        .map(super::workers_entry::field_spec_source)
        .collect();
    Some(format!("[{}]", items.join(", ")))
}

/// #3276: the wire value `Form.encode(arg)` writes flat: `arg` serialised exactly as
/// `Json.encode` serialises it, before the JSON text step. `None` only for a type
/// the lowering cannot render, which the checker has already refused for `Form`.
fn form_encode_wire(pre: &mut Pre, arg: &Expr, cx: &mut LowerCtx) -> Option<String> {
    let tys = cx.ty_render();
    let arg_ty = cx.commons().expr_types.get(&arg.id).map(|te| te.ty)?;
    let tref = ty_to_type_ref(arg_ty, tys.types())?;
    if cx.records_codec_roots() {
        cx.runtime_use().note_json_codec_root(arg_ty);
    }
    let v = pre.lower(arg, cx);
    Some(bynk_ts::print_expr(&serialisation::serialise_expr(
        &tref,
        &v,
        cx.runtime_use(),
    )))
}

/// #3585: the statement that reports a `bynk eval` case's tail value: its
/// `Json.encode` text and its type, as one NDJSON line. `None` when the value's
/// type is unknown to the lowering, so the case runs without reporting.
fn eval_value_line(pre: &mut Pre, tail: &Expr, cx: &mut LowerCtx) -> Option<String> {
    let ty = cx.commons().expr_types.get(&tail.id).map(|te| te.ty)?;
    let shown = bynk_check::symbols::display_type_names_in(&cx.commons().tys().display(ty));
    let shown = bynk_project::json_string(&shown);
    let head = format!(
        "globalThis.console.log('{{\"type\":\"eval-value\",\"valueType\":' + globalThis.JSON.stringify({shown})"
    );
    // A value with no wire shape (a function, say) still reports its type, and
    // no value.
    let Some(json) = json_encode_text(pre, tail, cx, false) else {
        let _ = pre.lower(tail, cx);
        return Some(format!("{head} + '}}');"));
    };
    // The encoder names `JsonValue` and `BoundaryError`, whether or not the
    // value's type needs a closure of codecs of its own.
    cx.runtime_use().note_json_codec();
    cx.runtime_use().note_boundary_codec();
    Some(format!("{head} + ',\"value\":' + {json} + '}}');"))
}

/// `Json.encode(arg)`'s text: `globalThis.JSON.stringify(<serialised arg>)`, with
/// the codec roots the module needs recorded. Shared by the `Json.encode` call
/// and the snapshot `expect` (#2842), so both encode a value one way. `None`
/// when the argument's type is unknown to the lowering.
pub(super) fn json_encode_text(
    pre: &mut Pre,
    arg: &Expr,
    cx: &mut LowerCtx,
    plain: bool,
) -> Option<String> {
    let tys = cx.ty_render();
    // Constrained generics slice 3: a value whose type mentions a type
    // parameter encodes through the codec witness of that type.
    let arg_ty = cx.commons().expr_types.get(&arg.id).map(|te| te.ty)?;
    if super::codec::has_var(arg_ty, cx.commons()) {
        if cx.records_codec_roots() {
            note_ground_roots(arg_ty, cx);
        }
        // 1A: witness printed for a text caller
        let codec = bynk_ts::print_expr(&cx.codec(arg_ty));
        let v = pre.lower(arg, cx);
        // `plain` never reaches a generic argument: the checker refused it.
        return Some(format!("globalThis.JSON.stringify({codec}.encode({v}))"));
    }
    let tref = ty_to_type_ref(arg_ty, tys.types())?;
    // #917: a test-scaffold module's target/`uses` types export no codec
    // of their own — record the root so the module that spliced this
    // body in can generate its own `serialise_*`/`deserialise_*` closure,
    // the same way a workers cross-context caller generates one for a
    // consumed context's boundary types (#661). P6.28: pushes the `TyId`
    // already in hand, not a re-derived `TypeRef` — `tref` below still
    // exists for this call site's own, separate codec-rendering use.
    if cx.records_codec_roots() {
        cx.runtime_use().note_json_codec_root(arg_ty);
    }
    let v = pre.lower(arg, cx);
    // #1435 (Arc E slice 1): boundary-print, same treatment as this
    // file's own `serialise_expr_via` call site above.
    if plain {
        // #2991: `Json.encodePlain`. The one `Option` encoder reads
        // `__bynkPlainJson`, so raise it for this one synchronous serialisation
        // and restore it in a `finally` (the encoder throws on a non-finite
        // `Float`). The checker has refused a generic argument, so only
        // monomorphic encoders (which read the flag) run here. Inlined rather
        // than a runtime helper, so no module needs a new import.
        // The argument must be evaluated *before* the flag is raised, or a tagged
        // `Json.encode` (or a function that calls one) inside it would come out
        // plain. An argument that awaits is hoisted to a temporary; any other
        // is the default of the arrow's one parameter, which JS evaluates at
        // call time, before the body raises the flag (its type is inferred).
        let (param, tmp) = if v.contains("await") {
            let tmp = format!("__pj{}", arg.id.0);
            pre.push_text(format!("const {tmp} = {v};"));
            (format!("__pv = {tmp}"), "__pv".to_string())
        } else {
            (format!("__pv = {v}"), "__pv".to_string())
        };
        let ser = bynk_ts::print_expr(&serialisation::serialise_expr(
            &tref,
            &tmp,
            cx.runtime_use(),
        ));
        return Some(format!(
            "(({param}): string => {{ const __g = globalThis as {{ __bynkPlainJson?: boolean }}; \
             const __was = __g.__bynkPlainJson; __g.__bynkPlainJson = true; \
             try {{ return globalThis.JSON.stringify({ser}); }} \
             finally {{ __g.__bynkPlainJson = __was; }} }})()"
        ));
    }
    let ser = bynk_ts::print_expr(&serialisation::serialise_expr(&tref, &v, cx.runtime_use()));
    Some(format!("globalThis.JSON.stringify({ser})"))
}

/// #917 and constrained generics slice 3: record the variable-free parts of a
/// codec target as roots, so the test-scaffold module that spliced the body in
/// generates the codecs its witnesses name.
fn note_ground_roots(t: bynk_check::checker::TyId, cx: &LowerCtx) {
    let mut parts = Vec::new();
    super::codec::ground_parts(t, cx.commons(), &mut parts);
    for p in parts {
        cx.runtime_use().note_json_codec_root(p);
    }
}

/// P4c (#2628): `Json.decodeSql[T]`'s pass over the parsed rows before the
/// ordinary codec runs. SQLite has no boolean, so a flag column arrives as
/// `0`/`1`; at each `Bool` field of the row record (the checker has confined
/// `T` to a row or a `List` of one) exactly those two numbers become
/// `false`/`true`. Anything else is left for the codec to accept or refuse,
/// so every generated codec stays the one `Json.decode` uses. Empty when the
/// row has no `Bool` field.
fn sql_bool_pass(t: bynk_check::checker::TyId, cx: &LowerCtx) -> String {
    let tys = cx.ty_render();
    let row = match &*tys.get(t) {
        Ty::List(r) => *r,
        _ => t,
    };
    let Ty::Named { name, .. } = &*tys.get(row) else {
        return String::new();
    };
    // The checker resolved this row in the same table, so a miss is a
    // compiler bug that would silently skip the pass.
    debug_assert!(
        cx.commons().types.contains_key(name.as_str()),
        "Json.decodeSql row `{name}` is not in the module's type table"
    );
    let fields = bynk_check::checker::row_bool_fields(name, &cx.commons().types);
    if fields.is_empty() {
        return String::new();
    }
    let keys = fields
        .iter()
        .map(|f| format!("\"{f}\""))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "const __flags = (__o: __JsonValue): void => {{ \
         if (__o === null || typeof __o !== \"object\" || globalThis.Array.isArray(__o)) return; \
         const __row = __o as {{ [k: string]: __JsonValue }}; \
         for (const __k of [{keys}]) {{ if (__row[__k] === 0 || __row[__k] === 1) __row[__k] = __row[__k] === 1; }} }}; \
         if (globalThis.Array.isArray(__j)) __j.forEach(__flags); else __flags(__j); "
    )
}

/// Hyperdrive H3 (#3320): a typed `Postgres` read's pass over the parsed rows
/// before the ordinary codec runs, the counterpart of [`sql_bool_pass`]. The
/// codec reads an `Instant` as epoch milliseconds and an `Int` as a number; a
/// Postgres row carries a `timestamptz` as ISO-8601 text and an `int8` as text
/// (ADR 0799 F), so the runtime's `__bynkPgRow` converts exactly the row's
/// `Instant` and `Int` fields, and leaves anything it cannot convert for the
/// codec to refuse. Empty when the row has neither.
fn pg_row_pass(t: bynk_check::checker::TyId, cx: &LowerCtx) -> String {
    let tys = cx.ty_render();
    let row = match &*tys.get(t) {
        Ty::List(r) => *r,
        _ => t,
    };
    let Ty::Named { name, .. } = &*tys.get(row) else {
        return String::new();
    };
    let types = &cx.commons().types;
    let instants = bynk_check::checker::row_base_fields(name, types, BaseType::Instant);
    let ints = bynk_check::checker::row_base_fields(name, types, BaseType::Int);
    if instants.is_empty() && ints.is_empty() {
        return String::new();
    }
    cx.runtime_use().note_pg_rows();
    let list = |names: &[String]| {
        names
            .iter()
            .map(|f| format!("\"{f}\""))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let (instants, ints) = (list(&instants), list(&ints));
    format!(
        "const __fix = (__o: __JsonValue): void => __bynkPgRow(__o, [{instants}], [{ints}]); \
         if (globalThis.Array.isArray(__j)) __j.forEach(__fix); else __fix(__j); "
    )
}

/// v0.6 cross-context service call: receiver is an alias or the dotted name
/// of a consumed context. In bundle mode, lower to
/// `deps.surface.<key>.<method>(args as <consumed_ns>.<T>)`; in workers mode
/// (v0.8), lower to
/// `callService(deps.env.<BINDING>, "<method>", serialise...(args), deserialise_<R>)`.
/// Returns `None` when the receiver is not a consumed-context prefix.
fn lower_cross_context_service_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> =
        if let Some((consumed, key)) = cross_context_lowering_prefix(receiver, cx) {
            cx.note_cross_context_used(&consumed);
            match cx.target() {
                BuildTarget::Bundle => {
                    let args_lowered: Vec<String> = args
                        .iter()
                        .enumerate()
                        .map(|(i, a)| {
                            let lowered = pre.lower(a, cx);
                            // #2042: every argument at a generic parameter
                            // that names a user type is cast, not only one
                            // holding a callee-typed part (#2039): a value the
                            // caller typed itself (`byKey: Map[String, Line]`)
                            // carries the caller's brand against the callee's.
                            param_cast(
                                &consumed,
                                cx.cross_context(),
                                method,
                                i,
                                lowered,
                                true,
                                cx.in_test_scaffold(),
                            )
                        })
                        .collect();
                    let call = format!(
                        "deps.surface.{key}.{}({})",
                        method.name,
                        args_lowered.join(", ")
                    );
                    // #2825: the result takes the caller's declaration of a
                    // same-named refined or opaque type, whose brand differs.
                    Some(return_cast(
                        &consumed,
                        cx.cross_context(),
                        &cx.commons().types,
                        cx.consumed_type_ns(),
                        method,
                        call,
                        cx.in_test_scaffold(),
                    ))
                }
                BuildTarget::Workers => {
                    let _ = key;
                    let call = pre.absorb(lower_workers_cross_context_call(
                        &consumed,
                        method,
                        args,
                        receiver.span.start,
                        cx,
                    ));
                    return Some(pre.finish_text(call));
                }
            }
        } else {
            None
        };
    text.map(|expr| pre.finish_text(expr))
}

fn lower_val(type_ref: &TypeRef, args: &[Expr], cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    // Resolve the mocked type straight from the AST node rather than the
    // checker's `expr_types` side-table — the static type table is always
    // populated, whereas a test body's per-expression types may not be visible
    // to the emitter.
    let ty = match bynk_check::checker::resolve_type_ref(type_ref, &cx.commons().types, tys.types())
    {
        Some(t) => t,
        // 1A: text, not a node: the comment is part of the output, and an
        // `Ident` carrying it is the escape the rule forbids.
        None => return pre.finish_text("undefined /* mock: unresolved type */".to_string()),
    };
    // Refined literal pin. This `Val[T](lit)` path is test-only scaffolding, where
    // the branded type is an `any` value binding, not a type (ADR 0182) — so brand
    // via `unchecked_construct_test` (refined → `(lit as any)`), not a `(lit as T)`
    // that would fail to resolve `T`. Opaque never reaches here (`NamedKind::Refined`).
    let ty_node = tys.get(ty);
    if let (
        Some(arg),
        Ty::Named {
            name,
            kind: NamedKind::Refined(_),
            ..
        },
    ) = (args.first(), &*ty_node)
    {
        let value = match lower_const_literal_node(arg) {
            Some(n) => LoweredExpr::Node(n),
            None => pre.lower_node(arg, cx),
        };
        let built = node_or_text(
            vec![value],
            |c| unchecked_construct_test_node(name, one(c), false),
            |c| unchecked_construct_test(name, &one(c), false),
        );
        return pre.finish_built(built);
    }
    // Bare mock (refined / opaque / sum / record).
    pre.finish(LoweredExpr::Node(mock_value(ty, cx, MOCK_DEPTH)))
}

/// When we encounter `lhs && rhs`, see if lhs is an `is` (possibly wrapped
/// in parens or nested `&&`) and if so collect the bindings to inject into
/// rhs. Returns `(binding_const_decls, lowered_lhs, lowered_rhs)` if
/// special handling is appropriate; otherwise returns None.
/// What [`lower_and_with_is`] produces for its caller to assemble.
struct AndWithIs {
    /// The left operand's `is` bindings, then the right operand's own hoisted
    /// statements: both run, in order, inside the right operand's IIFE.
    bindings: Vec<LoweredStmt>,
    lhs: Lowered,
    rhs: LoweredExpr,
    /// The right operand hoisted a `?`'s propagating early return.
    rhs_returns: bool,
    /// #1654: declarations of the `is` receiver temps the right operand
    /// introduced, for the caller to emit before the whole condition.
    hoisted: Vec<String>,
}

fn lower_and_with_is(lhs: &Expr, rhs: &Expr, cx: &mut LowerCtx) -> Option<AndWithIs> {
    // Probe structurally (no lowering) so a `&&` without an `is` falls through
    // to the caller's ordinary lowering untouched. This mirrors exactly the
    // shapes `gather_is_bindings_for_emit` walks (`&&` and parens), preserving
    // the original "Some iff lhs contains an `is`" behaviour.
    if !cond_contains_is(lhs) {
        return None;
    }
    // Lower lhs *before* gathering its bindings. Lowering lifts any complex
    // `is` receiver (e.g. a call) into a shared temp recorded on `cx`; the
    // binding gatherer below then references that temp via `is_receiver_text`
    // instead of re-emitting the receiver. For simple receivers nothing is
    // cached and the output is byte-identical to before.
    let lhs_lowered = lower_expr(lhs, cx);
    // #908: the gathered bindings are only in scope for `rhs` (the caller
    // wraps them together in one IIFE) — push a frame so a same-named `rhs`
    // read resolves to the binding rather than an outer `let` rename, and pop
    // once `rhs` is lowered so the registration doesn't leak past this `&&`.
    cx.shadow_scopes.push(HashMap::new());
    let mut bindings = Vec::new();
    let mut found = false;
    gather_is_bindings_for_emit(lhs, true, cx, &mut bindings, &mut found);
    let _ = found; // guaranteed true by the `cond_contains_is` guard above
    // #1/#3 review: `rhs`'s own hoisted statements used to be joined into one
    // string and spliced back in as if they were part of the expression text
    // (`bindings.is_empty()` — the common case, not an edge one — took the
    // caller's `format!("{lhs_expr} && {rhs_expr}")` path unedited, which is a
    // syntax error the moment `rhs` hoists anything: a `const` declaration
    // where JS expects an expression). `lower_expr` keeps them as real
    // statements instead, appended after the is-bindings — both must run, in
    // order, before the final `return rhs_expr` the caller wraps them in, so
    // one combined list is exactly what the caller's existing wrap already
    // expects.
    // T2.3 (R6.3): isolate the flag to just `rhs`'s own lowering, so a `?`
    // inside `lhs`'s `is`-receiver (already hoisted unconditionally into the
    // caller's own statement position, not this arrow — see `lower_bin_op`)
    // doesn't falsely report a propagating return coming from `bindings`.
    let saved_early_return = std::mem::take(&mut cx.emitted_early_return);
    // #1654: `rhs` is lowered inside an IIFE (below, in the caller), so an `is`
    // receiver temp it introduces would be scoped away from a then-branch that
    // reads it. Hoist the temps' declarations to the outermost such `&&`; a
    // nested one adds to the same list.
    let outermost = cx.is_temp_hoist.is_none();
    if outermost {
        cx.is_temp_hoist = Some(Vec::new());
    }
    let rhs_lowered = lower_expr(rhs, cx);
    let hoisted = if outermost {
        cx.is_temp_hoist.take().unwrap_or_default()
    } else {
        Vec::new()
    };
    let rhs_returns = cx.emitted_early_return;
    cx.emitted_early_return = saved_early_return || rhs_returns;
    let Lowered {
        pre: rhs_pre,
        expr: rhs_expr,
    } = rhs_lowered;
    bindings.extend(rhs_pre);
    cx.shadow_scopes.pop();
    Some(AndWithIs {
        bindings,
        lhs: lhs_lowered,
        rhs: rhs_expr,
        rhs_returns,
        hoisted,
    })
}

/// #1751/#1752: the receiver text to read variant `tag`'s payload fields from.
/// When the test was emitted out of line (`unnarrowed`), TypeScript hasn't
/// narrowed `value_text`, so the read goes through the variant the checker
/// proved.
fn variant_payload(value_text: &str, tag: &str, unnarrowed: bool) -> String {
    if unnarrowed {
        format!("({value_text} as Extract<typeof {value_text}, {{ tag: \"{tag}\" }}>)")
    } else {
        value_text.to_string()
    }
}

/// #1752: record the `is` tests in `rhs`, a short-circuit right operand
/// about to be emitted out of line (inside an arrow, or a hoisted `if`), as
/// unable to narrow their receivers anywhere else. A test is recorded under
/// either polarity, since an enclosing condition may gather its bindings
/// through `!`. See [`LowerCtx::unnarrowed_is_tests`].
fn mark_unnarrowed(rhs: &Expr, cx: &mut LowerCtx) {
    for when_true in [true, false] {
        for test in bynk_check::narrowing::matched_is_tests(rhs, when_true) {
            cx.unnarrowed_is_tests.insert(test.span);
        }
    }
}

/// Walk an expression collecting `const name = expr.field;` strings for
/// any `is`-pattern bindings on the truthy path. `found` indicates whether
/// at least one `is` was seen.
pub(super) fn gather_is_bindings_for_emit(
    e: &Expr,
    when_true: bool,
    cx: &mut LowerCtx,
    out: &mut Vec<LoweredStmt>,
    found: &mut bool,
) {
    // #1654: which `is` tests `e` proves is the shared rule the resolver and
    // the checker scope bindings by (`bynk_check::narrowing`).
    for test in bynk_check::narrowing::matched_is_tests(e, when_true) {
        emit_is_test_bindings(test, cx, out, found);
    }
}

/// `const <local> = <init>;`: a `Const` node over a node initialiser, else
/// today's text statement (1A.14: the variant-payload reads stay text, as the
/// `Extract<typeof …>` cast has no type node).
fn binding_stmt(local: &str, init: LoweredExpr) -> LoweredStmt {
    match init {
        LoweredExpr::Node(n) => LoweredStmt::Node(Box::new(bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::Ident(local.to_string()),
            None,
            n,
            None,
        ))),
        text => LoweredStmt::Text(format!("const {local} = {};", text.into_text())),
    }
}

/// The `const name = …;` declarations for one matched `is` test: nodes where
/// the receiver is a node and the read has a node form (a record field, a list
/// element or rest, the refined narrowing cast), text otherwise (a variant
/// payload read through its `Extract` cast, and an or-pattern's `let` and
/// `if`/`else` dispatch).
fn emit_is_test_bindings(
    e: &Expr,
    cx: &mut LowerCtx,
    out: &mut Vec<LoweredStmt>,
    found: &mut bool,
) {
    let tys = cx.ty_render();
    let ExprKind::Is { value, pattern } = &e.kind else {
        return;
    };
    {
        *found = true;
        let value_text = cx.is_receiver_text(value);
        let recv = cx.is_receiver_path(value);
        let disc_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
        if let Pattern::Variant {
            variant, bindings, ..
        } = pattern.as_ref()
        {
            // #1751/#1752: a test emitted out of line (inside a short-circuit
            // arrow) doesn't narrow its receiver here; read through the
            // variant the checker proved instead.
            // #2836: every tag-based `is` test is emitted un-narrowing (see
            // `lower_is`), so its payload is always read through the proven variant.
            let payload = variant_payload(&value_text, &variant.name, true);
            // v0.13: refinement narrowing re-binds the value's name to the
            // branded refined type, read from the forced receiver temp.
            if bindings.is_empty()
                && cx.is_refined_is_check(value, &variant.name)
                && let ExprKind::Ident(id) = &value.kind
            {
                let cast: Built = match recv.node() {
                    Some(r) => Ok(bynk_ts::TsExpr::As {
                        expr: Box::new(r.clone()),
                        ty: bynk_ts::TsType::named(variant.name.clone()),
                    }),
                    None => Err(format!("{value_text} as {}", variant.name)),
                };
                out.push(binding_stmt(&ts_ident(&id.name), cast.into()));
                cx.declare_binder(&id.name);
                return;
            }
            for (i, b) in bindings.iter().enumerate() {
                // `is` emits only flat, depth-1 name bindings (ADR 0169 keeps
                // nesting/guards match-only); `_` and nested patterns bind nothing.
                let Pattern::Binding(name) = b.pattern() else {
                    continue;
                };
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(disc_ty, &variant.name, i, tys.types())
                    }
                };
                // #1653: renamed when it shadows (`o is Some(o)`), so the
                // declaration can't read itself in its temporal dead zone.
                let local = cx.bind_local_name(&name.name);
                // 1A: the payload read goes through the `Extract<typeof …>` cast.
                let read: Built = Err(format!("{payload}.{field}"));
                out.push(binding_stmt(&local, read.into()));
            }
        }
        // #2641: a record pattern's depth-1 names — shorthand fields and
        // `f: name` — read straight off the receiver.
        if let Pattern::Record { fields, .. } = pattern.as_ref() {
            for (fname, sub) in fields {
                let bound = match sub {
                    None => fname,
                    Some(Pattern::Binding(n)) => n,
                    Some(_) => continue,
                };
                let local = cx.bind_local_name(&bound.name);
                out.push(binding_stmt(&local, recv.member(&fname.name).lowered()));
            }
        }
        // #2680: a list pattern's depth-1 names — bare-name elements and a
        // named rest.
        if let Pattern::List { elems, rest, .. } = pattern.as_ref() {
            for (i, sub) in elems.iter().enumerate() {
                if let Pattern::Binding(n) = sub {
                    let local = cx.bind_local_name(&n.name);
                    out.push(binding_stmt(&local, recv.index(i).lowered()));
                }
            }
            if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                let local = cx.bind_local_name(&binder.name);
                out.push(binding_stmt(&local, list_rest_value(&recv, elems.len())));
            }
        }
        // #474 §2.3.6: an or-pattern's shared names can live at different
        // structural paths per alternative (`Held`'s field 2 vs
        // `Confirmed`'s field 4), so — like the `match` if-chain's
        // `emit_pattern_bindings` — declare each name once with `let`,
        // then dispatch per alternative. Still depth-1 only (the same
        // existing `is` limitation as the `Variant` case above): an
        // alternative that isn't itself a flat `Variant` contributes no
        // bindings, only its tag test.
        if let Pattern::Or(alts, _) = pattern.as_ref() {
            let names = pattern.bound_names();
            if names.is_empty() {
                return;
            }
            let decl: Vec<String> = names
                .iter()
                .map(|id| cx.bind_local_name(&id.name))
                .collect();
            let mut lines: Vec<String> = Vec::new();
            lines.push(format!("let {};", decl.join(", ")));
            let last = alts.len() - 1;
            for (i, alt) in alts.iter().enumerate() {
                let (tag, pairs) = match alt {
                    // #2680: a list alternative is told apart by its length,
                    // so its dispatch condition is its whole structural test.
                    Pattern::List { elems, rest, .. } => {
                        let mut pairs = Vec::new();
                        for (j, sub) in elems.iter().enumerate() {
                            if let Pattern::Binding(n) = sub {
                                let local = cx
                                    .resolved_local_name(&n.name)
                                    .unwrap_or_else(|| ts_ident(&n.name));
                                pairs.push((local, format!("{value_text}[{j}]")));
                            }
                        }
                        if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                            let local = cx
                                .resolved_local_name(&binder.name)
                                .unwrap_or_else(|| ts_ident(&binder.name));
                            pairs.push((local, list_rest_expr(&value_text, elems.len())));
                        }
                        let mut tests = Vec::new();
                        pattern_tests(&recv, disc_ty, alt, cx, &mut tests);
                        let cond = if tests.is_empty() {
                            "true".to_string()
                        } else {
                            and_join(tests).into_text()
                        };
                        (Some(cond), pairs)
                    }
                    Pattern::Variant {
                        variant, bindings, ..
                    } => {
                        let mut pairs = Vec::new();
                        for (j, b) in bindings.iter().enumerate() {
                            let Pattern::Binding(name) = b.pattern() else {
                                continue;
                            };
                            let field = match &b.kind {
                                PatternBindingKind::Named { field, .. } => {
                                    payload_prop(&field.name)
                                }
                                PatternBindingKind::Positional { .. } => {
                                    cx.positional_field_name(disc_ty, &variant.name, j, tys.types())
                                }
                            };
                            let local = cx
                                .resolved_local_name(&name.name)
                                .unwrap_or_else(|| ts_ident(&name.name));
                            // #1765 review: out of line, the dispatch below
                            // starts from the whole union, so its last arm (a
                            // bare `else`) isn't narrowed to this variant.
                            let payload = variant_payload(&value_text, &variant.name, true);
                            pairs.push((local, payload + "." + &field));
                        }
                        (
                            Some(format!("{value_text}.tag === \"{}\"", variant.name)),
                            pairs,
                        )
                    }
                    // #2641: a record alternative has no tag to test; its
                    // depth-1 names read straight off the receiver.
                    Pattern::Record { fields, .. } => {
                        let mut pairs = Vec::new();
                        for (fname, sub) in fields {
                            let bound = match sub {
                                None => fname,
                                Some(Pattern::Binding(n)) => n,
                                Some(_) => continue,
                            };
                            let local = cx
                                .resolved_local_name(&bound.name)
                                .unwrap_or_else(|| ts_ident(&bound.name));
                            pairs.push((local, format!("{value_text}.{}", fname.name)));
                        }
                        (None, pairs)
                    }
                    _ => (None, Vec::new()),
                };
                if i == last {
                    lines.push("} else {".to_string());
                } else {
                    let cond = tag.unwrap_or_else(|| "true".to_string());
                    let kw = if i == 0 { "if" } else { "} else if" };
                    lines.push(format!("{kw} ({cond}) {{"));
                }
                for (name, path) in &pairs {
                    lines.push(format!("  {name} = {path};"));
                }
            }
            lines.push("}".to_string());
            // 1A: the `let` and the dispatch stay text statements.
            out.extend(lines.into_iter().map(LoweredStmt::Text));
        }
    }
}

/// True when an `is` receiver is a simple, side-effect-free, repeatable
/// lvalue — an identifier or a field-access chain ending at one (optionally
/// parenthesised). Such receivers can be referenced textually as many times
/// as there are pattern bindings without re-evaluation. Anything else (calls,
/// matches, arithmetic, …) must be lifted to a temp before use; see
/// `LowerCtx::is_receiver_ref`.
pub(crate) fn is_simple_is_receiver(value: &Expr) -> bool {
    match &value.kind {
        ExprKind::Ident(_) => true,
        ExprKind::FieldAccess { receiver, .. } => is_simple_is_receiver(receiver),
        ExprKind::Paren(inner) => is_simple_is_receiver(inner),
        _ => false,
    }
}

/// Render a *simple* `is` receiver (see `is_simple_is_receiver`) as a textual
/// reference for binding lookups.
///
/// #1668: a complex receiver never reaches this function. Its only caller,
/// `LowerCtx::is_receiver_text`, serves the binding gatherer
/// (`gather_is_bindings_for_emit`), and each of the gatherer's call sites
/// (`lower_and_with_is`, the value-position `if` IIFE, `emit_if_tail`) lowers
/// the condition first. Lowering every `is` test the gatherer visits
/// (`bynk_check::narrowing::matched_is_tests`: through `&&`, `!` and parens)
/// runs `is_receiver_ref`, which lifts a complex receiver into a temp cached by
/// span, and `is_receiver_text` returns that temp. Reaching the last arm means
/// that ordering broke. It used to emit a placeholder into the output, invalid
/// TypeScript that surfaced as a distant `tsc` error; it now fails here, like
/// the emitter's other broken invariants.
pub(crate) fn value_text_for_is(value: &Expr) -> String {
    match &value.kind {
        ExprKind::Ident(id) => ts_ident(&id.name),
        ExprKind::FieldAccess { receiver, field } => {
            format!("{}.{}", value_text_for_is(receiver), field.name)
        }
        ExprKind::Paren(inner) => value_text_for_is(inner),
        _ => panic!(
            "bynk internal error (#1668): the `is` receiver at {:?} is not a simple \
             lvalue and was not lifted to a temp before its bindings were gathered \
             — the condition must be lowered first",
            value.span
        ),
    }
}

/// [`value_text_for_is`] as a [`PatPath`] (1A.14): the same chain, each step a
/// node when it prints as the text step does.
pub(super) fn path_for_is(value: &Expr) -> PatPath {
    match &value.kind {
        ExprKind::Ident(id) => PatPath::ident(&ts_ident(&id.name)),
        ExprKind::FieldAccess { receiver, field } => path_for_is(receiver).member(&field.name),
        ExprKind::Paren(inner) => path_for_is(inner),
        // Panics, with the invariant's message (#1668).
        _ => PatPath::from_text(&value_text_for_is(value)),
    }
}

/// Slice 0b (#2662): the `parTraverse` family joins through `__bynkSettleAll`,
/// so a module using it needs the import even with no `par` block.
/// `lower_query_method` has no `cx`, so its callers note it here.
fn note_par_fan_out(cx: &LowerCtx, method: &Ident) {
    if matches!(
        method.name.as_str(),
        "parTraverse" | "parTraverseAll" | "parTraverseTry"
    ) {
        cx.note_settle_all();
    }
}

/// v0.100 (real-time track slice 0): lower a `Stream[T]` op over a `source`
/// async-iterable expression. **Builders** (`map`/`take`) wrap the source in a
/// new async generator — still lazy, still an `AsyncIterable`. The **terminal**
/// `collect` drains the source into an array (an `Effect`, awaited with `<-`).
/// Emitted inline, like the collection kernels, so non-stream files are
/// untouched. Callbacks are wrapped in a single-arg arrow so no extra iterator
/// argument reaches a one-param Bynk fn.
///
/// K-streams (#2623): a stream is single-pass. Each op claims its source when
/// it first reads it ([`STREAM_CLAIM`]): a builder when its generator is first
/// pulled, so building is free and draining claims; `collect` when it runs.
fn lower_stream_method(source: String, op: StreamOp, a: &[String]) -> Option<String> {
    let claim = STREAM_CLAIM;
    match (op, a) {
        // -- builders (return an AsyncIterable) --
        (StreamOp::Map, [f]) => Some(format!(
            "(async function* (__s) {{ {claim} for await (const __e of __s) {{ yield ({f})(__e); }} }})({source})"
        )),
        (StreamOp::Take, [n]) => Some(format!(
            "(async function* (__s) {{ {claim} const __n = {n}; if (__n <= 0) {{ return; }} let __i = 0; for await (const __e of __s) {{ yield __e; if (++__i >= __n) {{ return; }} }} }})({source})"
        )),
        // -- terminal (drains to a list; Effect-typed) --
        (StreamOp::Collect, []) => Some(format!(
            "(async (__s) => {{ {claim} const __r = []; for await (const __e of __s) {{ __r.push(__e); }} return __r; }})({source})"
        )),
        _ => None,
    }
}

/// K-streams (#2623): mark the stream `__s` as read, or fault if it already
/// was. The mark lives on the stream object under a registered symbol, so it
/// holds across modules, and the runtime's response bodies set the same one
/// (`claimStream` in `bynk-emit/runtime/src/http.ts`). Inline, so a file that
/// uses streams still needs no runtime import (ADR 0128 D6).
const STREAM_CLAIM: &str = "const __m = __s as unknown as { [k: symbol]: boolean }; const __k = globalThis.Symbol.for(\"bynk.stream.read\"); if (__m[__k]) { throw new globalThis.Error(\"bynk: a stream can be read only once\"); } __m[__k] = true;";

/// What `lower_query_method` needs to know about the receiver beyond its
/// already-lowered arguments: the rendered element types, and whether a key-based
/// method (`distinct`, `distinctBy`, `groupBy`, `joinOn`, `leftJoin`) has a key
/// that is not a JS primitive and so goes through the keyed helpers (#2789).
struct QueryShape<'a> {
    elem_ts: &'a str,
    other_elem_ts: &'a str,
    keyed: bool,
    /// The comparator expression for `sortBy`/`min`/`max` (#2787); `None` keeps
    /// the bare `<`/`>` operators, for `Int`/`String`/`Duration`/`Instant` keys.
    key_cmp: Option<bynk_ts::TsExpr>,
    /// The module's runtime-helper record, so a lowering that needs a helper
    /// imports it (`Query.sum` over a `Decimal` key needs `__bynkDecimalAdd`, #3421).
    runtime_use: &'a RuntimeUse,
}

/// The comparator for a `Query` `sortBy`/`min`/`max` key, as `lower_list_kernel`
/// picks it: `None` for a plain base key, otherwise the key type's comparator.
fn query_key_cmp(method: &Ident, args: &[Expr], cx: &LowerCtx) -> Option<bynk_ts::TsExpr> {
    match (method.name.as_str(), args) {
        ("sortBy" | "min" | "max", [key]) => kernel_key_comparator(key, cx),
        ("sortByDescending", [key]) => kernel_key_comparator(key, cx),
        _ => None,
    }
}

/// Whether this `Query` method's key is a non-primitive `Key` type, noting the
/// `keys` runtime helper it then calls. Mirrors `lower_list_kernel`'s
/// `key_is_plain` / `key_fn_is_plain` choice.
fn query_keyed(e: &Expr, method: &Ident, args: &[Expr], cx: &LowerCtx) -> bool {
    let tys = cx.ty_render();
    let (helper, plain) = match (method.name.as_str(), args) {
        ("distinct", []) => (
            "__bynkDistinct",
            cx.commons().expr_types.get(&e.id).is_some_and(
                |te| matches!(&*tys.get(te.ty), Ty::Query(el) if key_is_plain(*el, tys.types())),
            ),
        ),
        ("distinctBy", [key]) => ("__bynkDistinctBy", key_fn_is_plain(key, cx)),
        ("groupBy", [key, _]) => ("__bynkGroupBy", key_fn_is_plain(key, cx)),
        ("joinOn", [_, left, ..]) => ("__bynkJoinOn", key_fn_is_plain(left, cx)),
        ("leftJoin", [_, left, ..]) => ("__bynkLeftJoin", key_fn_is_plain(left, cx)),
        _ => return false,
    };
    if !plain {
        cx.runtime_use().note_key(helper);
    }
    !plain
}

/// v0.91 (ADR 0119, query-algebra slice 2): lower a lazy storage-query op over a
/// `source` array expression. **Builders** wrap the source in a deferred thunk
/// `() => …[]` (so the terminal reads *staged* state when it runs —
/// read-your-writes, ADR 0109/0119 D7). **Terminals** read the source array and
/// produce the result; they are `Effect`-typed (awaited with `<-`), but the
/// staged map is in memory, so a synchronous expression suffices (`await` on a
/// non-promise is identity) — except `forEach`, which awaits its effectful fn.
/// The element type is inferred from the typed source (`Record`'s values), so no
/// `__x` annotations are needed. Callbacks are wrapped in a single-arg arrow so
/// the array index never reaches a one-param Bynk fn.
///
/// `shape.elem_ts` is the receiver's own element type, already rendered — resolved
/// by review of #1460 (the `List[T]` sibling, `lower_list_kernel`, already
/// threads the equivalent `elem: TyId` through for its own `distinctBy`/
/// `joinOn`/`leftJoin`/`groupBy` arms; this function's own four collection-
/// kernel sites just never reused it). A pre-rendered string, not a `TyId`,
/// because this function has four real callers with four different sources
/// for it: a genuine `store Query[T]` field/expression (`Ty::Query`'s own
/// element `TyId`, via `ts_ty`), a held `store Map[K, Connection]`'s
/// connection frame type (already a `String`, `agent_held_map_frame`), a
/// plain `store Map[K, V]`'s value type (`agent_store_map_value_ts`, added by
/// this review), and a `store Log[T]`'s element type
/// (`agent_store_log_value_ts`, ditto) — the latter two mirror
/// `held_maps`/`held_maps_ts`'s own established construction exactly, just
/// for the non-held case.
///
/// `shape.other_elem_ts` is `joinOn`/`leftJoin`'s own `other`-side bucket type,
/// computed by every caller via the same `join_other_elem_ts` helper
/// `lower_list_kernel`'s own `joinOn`/`leftJoin`/`join` arms already call —
/// pushed to the call site (rather than this function taking `args`/`cx`
/// itself, `join_other_elem_ts`'s own `args[0]` indexing); every caller
/// already has `args`/`cx` in scope and guards the empty-args case
/// (`distinct`/`collect`/etc. have no `other` to derive one from —
/// `other_elem_ts` is simply unused there). Both ride in [`QueryShape`], with
/// `keyed`, to stay under clippy's argument-count lint.
/// A store-lift receiver (`Callee::Store`/`Callee::Query`, a `store Map` lifted
/// into a scan) carries its operation as a name, so resolve it to a [`QueryOp`]
/// here; the value-kernel `Query` receiver has a typed op already.
fn lower_query_by_name(
    source: String,
    method: &Ident,
    a: &[String],
    result_ty: Option<TyId>,
    shape: &QueryShape,
    tys: TyRender<'_>,
) -> Option<String> {
    QueryOp::from_name(&method.name)
        .and_then(|op| lower_query_method(source, op, a, result_ty, shape, tys))
}

fn lower_query_method(
    source: String,
    op: QueryOp,
    a: &[String],
    result_ty: Option<TyId>,
    shape: &QueryShape,
    tys: TyRender<'_>,
) -> Option<String> {
    let QueryShape {
        elem_ts,
        other_elem_ts,
        keyed,
        key_cmp,
        runtime_use,
    } = shape;
    let keyed = *keyed;
    let elem_el = ts_array_elem(elem_ts);
    let other_elem_el = ts_array_elem(other_elem_ts);
    // #3189: the source may be a promise (a handle backed by storage reads asynchronously), so
    // every operation runs on it through `__bynkThen`, as `__q`. An in-memory handle's values
    // arrive at once and the callback runs at once.
    let orig = source;
    let source = "__q".to_string();
    let thunk = |body: String| format!("(() => __bynkThen({orig}, (__q) => {body}))");
    let thunk_async = |body: String| format!("(() => __bynkThen({orig}, async (__q) => {body}))");
    let terminal = |expr: String| format!("__bynkThen({orig}, (__q) => {expr})");
    Some(match (op, a) {
        // -- builders → a deferred thunk over the narrowed source --
        (QueryOp::Filter, [p]) => thunk(format!("{source}.filter((__x) => ({p})(__x))")),
        (QueryOp::Map, [f]) => thunk(format!("{source}.map((__x) => ({f})(__x))")),
        // storage flatMap: the fn returns a `Query` (a thunk) — invoke each.
        (QueryOp::FlatMap, [f]) => thunk_async(format!(
            "{{ const __parts = await globalThis.Promise.all({source}.map((__x) => ({f})(__x)())); return __parts.flat(); }}"
        )),
        (QueryOp::SortBy, [key]) if let Some(cmp) = key_cmp => thunk(with_comparator(cmp, |c| {
            format!("[...{source}].sort((__a, __b) => {c}(({key})(__a), ({key})(__b)))")
        })),
        (QueryOp::SortBy, [key]) => thunk(format!(
            "[...{source}].sort((__a, __b) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? -1 : __ka > __kb ? 1 : 0; }})"
        )),
        (QueryOp::SortByDescending, [key]) if let Some(cmp) = key_cmp => {
            thunk(with_comparator(cmp, |c| {
                format!("[...{source}].sort((__a, __b) => {c}(({key})(__b), ({key})(__a)))")
            }))
        }
        (QueryOp::SortByDescending, [key]) => thunk(format!(
            "[...{source}].sort((__a, __b) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? 1 : __ka > __kb ? -1 : 0; }})"
        )),
        (QueryOp::Take, [n]) => thunk(format!("{source}.slice(0, globalThis.Math.max(0, {n}))")),
        (QueryOp::Skip, [n]) => thunk(format!("{source}.slice(globalThis.Math.max(0, {n}))")),
        (QueryOp::Distinct, []) if keyed => thunk(format!("__bynkDistinct({source})")),
        (QueryOp::DistinctBy, [key]) if keyed => {
            thunk(format!("__bynkDistinctBy({source}, {key})"))
        }
        (QueryOp::GroupBy, [key, into]) if keyed => {
            thunk(format!("__bynkGroupBy({source}, {key}, {into})"))
        }
        (QueryOp::JoinOn, [other, left, right, into]) if keyed => thunk_async(format!(
            "__bynkJoinOn({source}, await ({other})(), {left}, {right}, {into})"
        )),
        (QueryOp::LeftJoin, [other, left, right, into]) if keyed => thunk_async(format!(
            "__bynkLeftJoin({source}, await ({other})(), {left}, {right}, {into})"
        )),
        (QueryOp::Distinct, []) => thunk(format!("[...new globalThis.Set({source})]")),
        // Review of #1460: narrowed, not deferred. A first attempt used
        // `unknown[]` here and broke real `tsc --strict` fixtures (228/231) —
        // the whole expression's result flows into a context expecting a
        // specific element type (e.g. `readonly Reservation[]`), inferred
        // *because* `__out`/`__h`'s buckets were left untyped for TS to infer
        // through; pinning them to `unknown[]` blocked that inference rather
        // than improving on it. The real fix is the receiver's own `Query[T]`
        // element type, `elem_ts` above — already resolved at every call site
        // via `Ty::Query(elem)`, just never threaded into this function before
        // (the `List[T]` sibling, `lower_list_kernel`, already does this for
        // its own `distinctBy`/`joinOn`/`leftJoin`/`groupBy`).
        (QueryOp::DistinctBy, [key]) => thunk(format!(
            "(() => {{ const __seen = new globalThis.Set(); const __out: {elem_el}[] = []; for (const __x of {source}) {{ const __k = ({key})(__x); if (!__seen.has(__k)) {{ __seen.add(__k); __out.push(__x); }} }} return __out; }})()"
        )),
        // v0.94 (ADR 0116/0120): joins & grouping over storage queries — lazy
        // builders. `other` is itself a `Query` thunk, invoked to materialise the
        // probed side; the result projects through `into` (no pair value). The
        // hash key is stringified (value-keyable); `groupBy`/`into` get the
        // original key, re-derived from a representative row.
        //
        // Review of #1460: `other`'s own element type is `other_elem_ts`,
        // computed at the call site via `join_other_elem_ts` (the same
        // helper `lower_list_kernel`'s own `joinOn`/`leftJoin`/`join` arms
        // already call over `args[0]`'s checked type) — reused, not
        // re-derived, and threaded as a param rather than called here to
        // keep this function's own argument count under clippy's lint.
        (QueryOp::JoinOn, [other, left, right, into]) => thunk_async(format!(
            "{{ const __h: globalThis.Record<string, {other_elem_el}[]> = globalThis.Object.create(null); for (const __u of (await ({other})())) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return {source}.flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.map((__u: {other_elem_ts}) => ({into})(__t, __u)); }}); }}"
        )),
        (QueryOp::LeftJoin, [other, left, right, into]) => thunk_async(format!(
            "{{ const __h: globalThis.Record<string, {other_elem_el}[]> = globalThis.Object.create(null); for (const __u of (await ({other})())) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return {source}.flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.length > 0 ? __m.map((__u: {other_elem_ts}) => ({into})(__t, Some(__u))) : [({into})(__t, None)]; }}); }}"
        )),
        (QueryOp::Join, [other, on, into]) => thunk_async(format!(
            "{{ const __b = await ({other})(); return {source}.flatMap((__t) => __b.filter((__u) => ({on})(__t, __u)).map((__u) => ({into})(__t, __u))); }}"
        )),
        // Review of #1460: `__h`'s buckets hold `{source}`'s own rows
        // (`elem_ts`), the receiver's element type — same as `distinctBy`
        // above, not `other`-derived (`groupBy` has no `other` argument).
        (QueryOp::GroupBy, [key, into]) => thunk(format!(
            "{{ const __h: globalThis.Record<string, {elem_el}[]> = globalThis.Object.create(null); const __order: string[] = []; for (const __t of {source}) {{ const __k = String(({key})(__t)); if (!(__k in __h)) {{ __h[__k] = []; __order.push(__k); }} __h[__k].push(__t); }} return __order.map((__k) => {{ const __rows = __h[__k]; return ({into})(({key})(__rows[0]), __rows); }}); }}"
        )),
        // -- terminals → read the source array (awaited at the `<-`) --
        (QueryOp::Collect, []) => terminal(source),
        (QueryOp::First, []) => terminal(format!(
            "(() => {{ const __a = {source}; return __a.length > 0 ? Some(__a[0]) : None; }})()"
        )),
        (QueryOp::FirstOrElse, [default]) => terminal(format!(
            "(() => {{ const __a = {source}; return __a.length > 0 ? __a[0] : ({default}); }})()"
        )),
        (QueryOp::Count, []) => terminal(format!("{source}.length")),
        (QueryOp::Fold, [init, f]) => terminal(format!(
            "(() => {{ let __acc = {init}; for (const __x of {source}) __acc = ({f})(__acc, __x); return __acc; }})()"
        )),
        (QueryOp::Any, [p]) => terminal(format!("{source}.some((__x) => ({p})(__x))")),
        (QueryOp::All, [p]) => terminal(format!("{source}.every((__x) => ({p})(__x))")),
        // #3421: a `Decimal` key sums exactly, as `List.sum` does (#2622): from the
        // zero decimal through the runtime's exact add, never a float `+`.
        (QueryOp::Sum, [key])
            if matches!(
                result_ty.map(|t| tys.get(t)).as_deref(),
                Some(Ty::Effect(inner)) if matches!(&*tys.get(*inner), Ty::Base(BaseType::Decimal))
            ) =>
        {
            runtime_use.note_decimal("__bynkDecimalAdd");
            terminal(format!(
                "{source}.reduce((__s: {}, __x) => __bynkDecimalAdd(__s, ({key})(__x)), {})",
                crate::emitter::ts_types::DECIMAL_TS_TYPE,
                crate::emitter::ts_types::decimal_ts_literal("0", 0)
            ))
        }
        (QueryOp::Sum, [key]) => terminal(format!(
            "{source}.reduce((__s: number, __x) => __s + ({key})(__x), 0)"
        )),
        (QueryOp::Min | QueryOp::Max, [key]) if let Some(key_cmp) = key_cmp => {
            let op = if op == QueryOp::Min { "<" } else { ">" };
            with_comparator(key_cmp, |c| {
                terminal(format!(
                    "(() => {{ const __a = {source}; if (__a.length === 0) return None; let __m = ({key})(__a[0]); for (const __x of __a) {{ const __k = ({key})(__x); if ({c}(__k, __m) {op} 0) __m = __k; }} return Some(__m); }})()"
                ))
            })
        }
        (QueryOp::Min | QueryOp::Max, [key]) => {
            let cmp = if op == QueryOp::Min { "<" } else { ">" };
            terminal(format!(
                "(() => {{ const __a = {source}; if (__a.length === 0) return None; let __m = ({key})(__a[0]); for (const __x of __a) {{ const __k = ({key})(__x); if (__k {cmp} __m) __m = __k; }} return Some(__m); }})()"
            ))
        }
        (QueryOp::Average, [key]) => {
            // Duration averages round to integer millis (checker result decides).
            let round = matches!(
                result_ty.map(|t| tys.get(t)).as_deref(),
                Some(Ty::Effect(inner))
                    if matches!(&*tys.get(*inner), Ty::Option(o)
                        if matches!(&*tys.get(*o), Ty::Base(BaseType::Duration)))
            );
            let mean = if round {
                "globalThis.Math.round(__s / __a.length)"
            } else {
                "__s / __a.length"
            };
            terminal(format!(
                "(() => {{ const __a = {source}; if (__a.length === 0) return None; let __s = 0; for (const __x of __a) __s += ({key})(__x); return Some({mean}); }})()"
            ))
        }
        (QueryOp::ForEach, [f]) => terminal(format!(
            "(async () => {{ for (const __x of {source}) {{ await ({f})(__x); }} }})()"
        )),
        // v0.107 (slice 4): the parallel broadcast form — issue the effectful fn over
        // every element concurrently and await them together, so one slow element
        // does not head-of-line-block the rest.
        (QueryOp::ParTraverse, [f]) => terminal(format!(
            "(async () => {{ await __bynkSettleAll({source}.map((__x) => () => ({f})(__x))); }})()"
        )),
        // v0.149 (ADR 0173): the collect-all terminals over a query/broadcast —
        // gather every `Result` outcome (an `Err` is a value, so nothing rejects).
        (QueryOp::TraverseAll, [f]) => {
            let res_ts = match result_ty.map(|t| tys.get(t)).as_deref() {
                Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
                    Ty::List(el) => ts_ty(*el, tys),
                    _ => ts_ty(*inner, tys),
                },
                _ => "any".to_string(),
            };
            let res_el = ts_array_elem(&res_ts);
            terminal(format!(
                "(async () => {{ const __out: {res_el}[] = []; for (const __x of {source}) {{ __out.push(await ({f})(__x)); }} return __out; }})()"
            ))
        }
        (QueryOp::ParTraverseAll, [f]) => terminal(format!(
            "(async () => await __bynkSettleAll({source}.map((__x) => () => ({f})(__x))))()"
        )),
        // v0.150 (ADR 0174): the short-circuit collect terminals — stop at the
        // first `Err`, returning `Result[List[U], E]`.
        (QueryOp::TraverseTry, [f]) => {
            let u_ts = list_ok_elem_ts(result_ty, tys);
            let u_el = ts_array_elem(&u_ts);
            terminal(format!(
                "(async () => {{ const __out: {u_el}[] = []; for (const __x of {source}) {{ const __r = await ({f})(__x); if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})()"
            ))
        }
        (QueryOp::ParTraverseTry, [f]) => {
            let u_ts = list_ok_elem_ts(result_ty, tys);
            let u_el = ts_array_elem(&u_ts);
            terminal(format!(
                "(async () => {{ const __rs = await __bynkSettleAll({source}.map((__x) => () => ({f})(__x))); const __out: {u_el}[] = []; for (const __r of __rs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})()"
            ))
        }
        _ => return None,
    })
}

// ---- v0.93 (ADR 0118): `@indexed` secondary-index emission ----------------
//
// For a `store Map[K, V] @indexed(by: f)` field, the map's handle keeps a posting list
// per index: a (stringified) field value maps to the primary keys whose value carries
// it, in insertion order (#3254). Its `put`/`remove` maintain it; an equality `filter`
// on `f` reads it through `byIndex` instead of scanning.

/// If `lam` is `(p) => p.<field> == <value>` (either order) where `<field>` is
/// indexed and `<value>` does not mention `p`, lower it to a posting-list lookup
/// thunk `() => map.byIndex(f, v)`. Otherwise `None` (fall back to scan).
fn route_indexed_filter(
    m: &str,
    fields: &[String],
    lam: &LambdaExpr,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    if fields.is_empty() {
        return None;
    }
    let [param] = lam.params.as_slice() else {
        return None;
    };
    let pname = param.name.name.as_str();
    let ExprKind::BinOp(BinOp::Eq, lhs, rhs) = &lam.body.kind else {
        return None;
    };
    // One side must be `p.<field>`; the other is the (param-independent) value.
    let (field, value) = match as_param_field(pname, lhs) {
        Some(f) => (f, rhs.as_ref()),
        None => (as_param_field(pname, rhs)?, lhs.as_ref()),
    };
    if !fields.iter().any(|f| f == field) || !param_independent(value, pname) {
        return None;
    }
    let v = pre.lower(value, cx);
    let text: Option<String> = Some(format!("(() => {m}.byIndex(\"{field}\", String({v})))"));
    text.map(|expr| pre.finish_text(expr))
}

/// `e` as `<param>.<field>` → the field name; else `None`.
fn as_param_field<'e>(pname: &str, e: &'e Expr) -> Option<&'e str> {
    if let ExprKind::FieldAccess { receiver, field } = &e.kind
        && let ExprKind::Ident(r) = &receiver.kind
        && r.name == pname
    {
        Some(field.name.as_str())
    } else {
        None
    }
}

/// Whether `e` provably does not reference the lambda parameter `pname` — only
/// then is it safe to hoist it out of the per-row predicate into one lookup key.
/// Conservative: unrecognised shapes return `false` (no routing).
fn param_independent(e: &Expr, pname: &str) -> bool {
    match &e.kind {
        ExprKind::IntLit { .. }
        | ExprKind::FloatLit { .. }
        | ExprKind::StrLit(_)
        | ExprKind::BoolLit(_)
        | ExprKind::DurationLit { .. } => true,
        ExprKind::Ident(id) => id.name != pname,
        ExprKind::FieldAccess { receiver, .. } => param_independent(receiver, pname),
        _ => false,
    }
}

/// v0.21/v0.22a: lower a built-in numeric kernel method. `toFloat` is the
/// identity at runtime (the Int/Float distinction is erased); everything
/// else maps onto `Math.*` / `Number.*`.
fn lower_int_kernel(
    receiver: &Expr,
    op: IntOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (IntOp::ToFloat, []) => Some(pre.lower(receiver, cx)),
        (IntOp::Abs, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.abs({recv})"))
        }
        (IntOp::Min | IntOp::Max, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("globalThis.Math.{}({recv}, {other})", op.name()))
        }
        (IntOp::Clamp, [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "globalThis.Math.min(globalThis.Math.max({recv}, {lo}), {hi})"
            ))
        }
        // v0.42 (ADR 0074): host number→string — `String(n)` is ECMAScript's
        // Number::toString (shortest round-trip; `1e21`/`Infinity`/`NaN` as the
        // host renders them). The normative contract is the platform's.
        (IntOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        // K6b (#2598): the `Int` division companions and exact power, through
        // `int.ts` helpers so their fault branches stay out of coverage.
        (
            IntOp::Div
            | IntOp::Rem
            | IntOp::Mod
            | IntOp::Pow
            | IntOp::And
            | IntOp::Or
            | IntOp::Xor
            | IntOp::AndNot
            | IntOp::Shl
            | IntOp::Shr,
            [other],
        ) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            let helper = match op {
                IntOp::Div => "__bynkIntDivFloor",
                IntOp::Rem => "__bynkIntRem",
                IntOp::Mod => "__bynkIntMod",
                // #2621: the bitwise operations, split hi/lo in the runtime.
                IntOp::And => "__bynkIntAnd",
                IntOp::Or => "__bynkIntOr",
                IntOp::Xor => "__bynkIntXor",
                IntOp::AndNot => "__bynkIntAndNot",
                IntOp::Shl => "__bynkIntShl",
                IntOp::Shr => "__bynkIntShr",
                _ => "__bynkIntPow",
            };
            cx.note_int();
            Some(format!("{helper}({recv}, {other})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// The `Float` kernel's lowering (see [`lower_int_kernel`]).
fn lower_float_kernel(
    receiver: &Expr,
    op: FloatOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (FloatOp::Abs, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.abs({recv})"))
        }
        // #1657 (runtime-semantics track §3.4, decided in S8): a Float→`Int`
        // conversion traps when the result is not a safe integer — a
        // non-finite `Float`, or one past ±2^53 — rather than producing an
        // `Int` outside the domain (`__bynkToInt`). The same posture as `Int`
        // division by zero.
        // Returning `Option[Int]` was rejected: a signature change to four
        // kernel methods.
        (FloatOp::Round | FloatOp::Floor | FloatOp::Ceil | FloatOp::Truncate, []) => {
            let recv = pre.lower(receiver, cx);
            let f = if op == FloatOp::Truncate {
                "trunc"
            } else {
                op.name()
            };
            cx.note_int();
            Some(format!(
                "__bynkToInt(globalThis.Math.{f}({recv}), \"{}\")",
                op.name()
            ))
        }
        (FloatOp::Min | FloatOp::Max, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("globalThis.Math.{}({recv}, {other})", op.name()))
        }
        (FloatOp::Clamp, [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "globalThis.Math.min(globalThis.Math.max({recv}, {lo}), {hi})"
            ))
        }
        (FloatOp::IsNaN | FloatOp::IsFinite, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Number.{}({recv})", op.name()))
        }
        (FloatOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        // K6b (#2598): the `Float` functions, straight onto JS `Math`.
        (
            FloatOp::Sqrt
            | FloatOp::Exp
            | FloatOp::Log10
            | FloatOp::Log2
            | FloatOp::Sin
            | FloatOp::Cos
            | FloatOp::Tan
            | FloatOp::Asin
            | FloatOp::Acos
            | FloatOp::Atan,
            [],
        ) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.{}({recv})", op.name()))
        }
        (FloatOp::Ln, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.log({recv})"))
        }
        (FloatOp::Pow, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("globalThis.Math.pow({recv}, {other})"))
        }
        // `y.atan2(x)`: the angle of the point `(x, y)`.
        (FloatOp::Atan2, [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("globalThis.Math.atan2({recv}, {x})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.86 (ADR 0112): lower a `Duration` kernel method. `toMillis` is the
/// identity (a `Duration` lowers to its milliseconds); `toString` renders it.
fn lower_duration_kernel(
    receiver: &Expr,
    op: DurationOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (DurationOp::ToMillis, []) => Some(pre.lower(receiver, cx)),
        (DurationOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.90 (ADR 0114): lower an `Instant` kernel method. `toEpochMillis` is the
/// identity (an `Instant` lowers to its epoch milliseconds); `toString` renders
/// it.
fn lower_instant_kernel(
    receiver: &Expr,
    op: InstantOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (InstantOp::ToEpochMillis, []) => Some(pre.lower(receiver, cx)),
        (InstantOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        // K3 (#2592): RFC 3339 in UTC.
        (InstantOp::ToIso, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_time_ops();
            Some(format!("__bynkInstantToIso({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.110 (ADR 0142 D3/D4): lower a `Bytes` kernel method. `length` is the
/// `Uint8Array.length` (octet count, not any string length); `toBase64` is a
/// total encode; `decodeUtf8` is a guarded fatal decode returning `Option`
/// (`None` on an invalid UTF-8 sequence).
fn lower_bytes_kernel(
    receiver: &Expr,
    op: BytesOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (op, args) {
        (BytesOp::Length, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).length"))
        }
        (BytesOp::ToBase64, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes();
            Some(format!("__bynkBytesToBase64({recv})"))
        }
        (BytesOp::DecodeUtf8, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes();
            Some(format!("__bynkBytesDecodeUtf8({recv})"))
        }
        // #2016 (standard-library K1). `slice` clamps negative indices to 0 as
        // `String.slice` does, and `Uint8Array.slice` copies, so a slice never
        // aliases a buffer a binding may still hold.
        (BytesOp::Slice, [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "({recv}).slice(globalThis.Math.max(0, {lo}), globalThis.Math.max(0, {hi}))"
            ))
        }
        (BytesOp::Concat, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesConcat({recv}, {other})"))
        }
        (BytesOp::Compare, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesCompare({recv}, {other})"))
        }
        (BytesOp::ToHex, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesToHex({recv})"))
        }
        (BytesOp::ToBase64Url, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesToBase64Url({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.22a: lower a built-in `String` kernel method (ADR 0046). Pinned
/// semantics: `replace` is replace-**all** (`replaceAll`) and inserts its
/// replacement literally (#1650); `chars()` is
/// code **points** (`[...s]`), not code units; `slice` clamps negative
/// indices to `0` (no TS wrap-around); `indexOf` turns `-1` into `None`.
/// K6c (#2600): a regex method's pattern literal as a `RegExp` construction,
/// its inline flags moved into the flags argument, plus `extra` flags (`g` for
/// a global scan, `y` for a sticky one). `anchored` pins it to the whole
/// string with lookarounds rather than `^…$`, which an `m` flag would turn into
/// line anchors. V8 caches a `RegExp`'s compilation by source and
/// flags, so building one per call compiles each pattern once; a fresh object
/// also keeps no `lastIndex` state between calls.
fn regex_construction(literal: &str, extra: &str, anchored: bool) -> String {
    let (pat, flags) = bynk_check::checker::split_regex_flags(literal);
    let source = if anchored {
        format!("(?<![\\s\\S])(?:{pat})(?![\\s\\S])")
    } else {
        pat
    };
    format!(
        "new globalThis.RegExp(\"{}\", \"{flags}{extra}\")",
        crate::emitter::escape_ts_string(&source)
    )
}

fn lower_string_kernel(
    receiver: &Expr,
    op: StringOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    // K6c (#2600): the checker admits a regex method only with a literal
    // pattern; anything else never reaches here.
    let pattern = match args.first().map(|a| &a.kind) {
        Some(ExprKind::StrLit(literal))
            if matches!(
                op,
                StringOp::MatchesRegex
                    | StringOp::ContainsRegex
                    | StringOp::FindRegex
                    | StringOp::FindAllRegex
                    | StringOp::ReplaceRegex
                    | StringOp::SplitRegex
            ) =>
        {
            Some(literal.clone())
        }
        _ => None,
    };
    if let Some(literal) = pattern {
        let recv = pre.lower(receiver, cx);
        let groups = "(__m: globalThis.RegExpMatchArray): Option<string>[] => \
                      [...__m].map((__g: string | undefined): Option<string> => \
                      __g === undefined ? None : Some(__g))";
        let text = match (op, args.len()) {
            (StringOp::MatchesRegex, 1) => Some(format!(
                "{}.test({recv})",
                regex_construction(&literal, "", true)
            )),
            (StringOp::ContainsRegex, 1) => Some(format!(
                "{}.test({recv})",
                regex_construction(&literal, "", false)
            )),
            (StringOp::FindRegex, 1) => Some(format!(
                "((__m: globalThis.RegExpExecArray | null): Option<readonly Option<string>[]> => \
                 __m === null ? None : Some(({groups})(__m)))({}.exec({recv}))",
                regex_construction(&literal, "", false)
            )),
            (StringOp::FindAllRegex, 1) => Some(format!(
                "([...({recv}).matchAll({})].map({groups}) as readonly (readonly Option<string>[])[])",
                regex_construction(&literal, "g", false)
            )),
            (StringOp::ReplaceRegex, 2) => {
                let with = pre.lower(&args[1], cx);
                Some(format!(
                    "({recv}).replace({}, {with})",
                    regex_construction(&literal, "g", false)
                ))
            }
            // JS `split` with a regex, minus the captured groups it splices in:
            // its own algorithm over a sticky scan, an empty match at the last
            // split point advancing by one code unit.
            (StringOp::SplitRegex, 1) => Some(format!(
                "((__s: string, __r: globalThis.RegExp): readonly string[] => {{ \
                 if (__s.length === 0) {{ return __r.exec(__s) === null ? [\"\"] : []; }} \
                 const __out: string[] = []; let __p = 0; let __q = 0; \
                 while (__q < __s.length) {{ __r.lastIndex = __q; const __hit = __r.exec(__s); \
                 const __e = __r.lastIndex; \
                 if (__hit === null || __e === __p) {{ __q++; }} \
                 else {{ __out.push(__s.slice(__p, __q)); __p = __e; __q = __p; }} }} \
                 __out.push(__s.slice(__p)); return __out; }})({recv}, {})",
                regex_construction(&literal, "y", false)
            )),
            _ => None,
        };
        return text.map(|expr| pre.finish_text(expr));
    }
    let text: Option<String> = match (op, args) {
        (StringOp::Length, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).length"))
        }
        (StringOp::Trim, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.trim()"))
        }
        (StringOp::ToUpper, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.toUpperCase()"))
        }
        (StringOp::ToLower, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.toLowerCase()"))
        }
        (StringOp::Chars, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...{recv}]"))
        }
        (StringOp::Split, [sep]) => {
            let recv = pre.lower(receiver, cx);
            let sep = pre.lower(sep, cx);
            Some(format!("{recv}.split({sep})"))
        }
        (StringOp::Contains, [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!("{recv}.includes({sub})"))
        }
        (StringOp::StartsWith | StringOp::EndsWith, [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!("{recv}.{}({sub})", op.name()))
        }
        (StringOp::Concat, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("{recv}.concat({other})"))
        }
        (StringOp::Replace, [from, to]) => {
            let recv = pre.lower(receiver, cx);
            let from = pre.lower(from, cx);
            let to = pre.lower(to, cx);
            // #1650: a *string* replacement is `$`-expanded by JS (`$&`, `$1`,
            // `` $` ``, `$'`, `$$`); a function replacer's return value never is,
            // so the replacement is inserted literally. `to` is a pure
            // expression, so its result is the same either way, but it is
            // evaluated once per match (and not at all with no match), so a
            // costly replacement is recomputed.
            Some(format!("{recv}.replaceAll({from}, () => {to})"))
        }
        (StringOp::Slice, [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "{recv}.slice(globalThis.Math.max(0, {lo}), globalThis.Math.max(0, {hi}))"
            ))
        }
        (StringOp::IndexOf, [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!(
                "((__i: number) => __i < 0 ? None : Some(__i))({recv}.indexOf({sub}))"
            ))
        }
        // K2 (#2594): percent-encoding over the UTF-8 bytes.
        (StringOp::PercentEncode, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_url_ops();
            Some(format!("__bynkPercentEncode({recv})"))
        }
        (StringOp::PercentDecode, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_url_ops();
            Some(format!("__bynkPercentDecode({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.22a: lower a built-in `Option[T]` kernel method (ADR 0048). Typed
/// IIFEs in the v0.20b posture — no runtime imports beyond the
/// `Some`/`None`/`Ok`/`Err` constructors every module already has.
#[allow(clippy::too_many_arguments)]
fn lower_option_kernel(
    e: &Expr,
    receiver: &Expr,
    op: OptionOp,
    args: &[Expr],
    inner: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let t = ts_ty(inner, tys);
    let text: Option<String> = match (op, args) {
        (OptionOp::Map, [f]) => {
            // The call's checked type is `Option[B]` — peel for B.
            let b = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Option(b)) => ts_ty(*b, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Option[_]` recorded \
                     for an `Option.map` call at {:?}, but found {other:?} — the checker \
                     should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__o: Option<{t}>, __f: (x: {t}) => {b}) => __o.tag === \"Some\" ? Some(__f(__o.value)) : None)({recv}, {f})"
            ))
        }
        (OptionOp::AndThen, [f]) => {
            let b = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Option(b)) => ts_ty(*b, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Option[_]` recorded \
                     for an `Option.andThen` call at {:?}, but found {other:?} — the checker \
                     should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__o: Option<{t}>, __f: (x: {t}) => Option<{b}>) => __o.tag === \"Some\" ? __f(__o.value) : None)({recv}, {f})"
            ))
        }
        (OptionOp::GetOrElse, [fallback]) => {
            let recv = pre.lower(receiver, cx);
            let fallback = pre.lower(fallback, cx);
            Some(format!(
                "((__o: Option<{t}>, __d: {t}) => __o.tag === \"Some\" ? __o.value : __d)({recv}, {fallback})"
            ))
        }
        (OptionOp::IsSome, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"Some\")"))
        }
        (OptionOp::OkOr, [error]) => {
            // The call's checked type is `Result[T, E]` — peel for E.
            let err = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Result(_, err)) => ts_ty(*err, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Result[_, _]` \
                     recorded for an `Option.okOr` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let error = pre.lower(error, cx);
            Some(format!(
                "((__o: Option<{t}>, __e: {err}) => __o.tag === \"Some\" ? Ok(__o.value) : Err(__e))({recv}, {error})"
            ))
        }
        (OptionOp::IsNone, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"None\")"))
        }
        (OptionOp::OrElse, [f]) => {
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__o: Option<{t}>, __f: () => Option<{t}>) => __o.tag === \"Some\" ? __o : __f())({recv}, {f})"
            ))
        }
        (OptionOp::Filter, [p]) => {
            let recv = pre.lower(receiver, cx);
            let p = pre.lower(p, cx);
            Some(format!(
                "((__o: Option<{t}>, __p: (x: {t}) => boolean) => __o.tag === \"Some\" && __p(__o.value) ? __o : None)({recv}, {p})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.22a: lower a built-in `Result[T, E]` kernel method (ADR 0048). The
/// miss branches return the narrowed receiver — TS's discriminated-union
/// narrowing makes the `Err` arm assignable to `Result<B, E>` directly.
#[allow(clippy::too_many_arguments)]
fn lower_result_kernel(
    e: &Expr,
    receiver: &Expr,
    op: ResultOp,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let t = ts_ty(ok, tys);
    let et = ts_ty(err, tys);
    let text: Option<String> = match (op, args) {
        (ResultOp::Map, [f]) => {
            // The call's checked type is `Result[B, E]` — peel for B.
            let b = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Result(b, _)) => ts_ty(*b, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Result[_, _]` \
                     recorded for a `Result.map` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __f: (x: {t}) => {b}) => __r.tag === \"Ok\" ? Ok(__f(__r.value)) : __r)({recv}, {f})"
            ))
        }
        (ResultOp::AndThen, [f]) => {
            let b = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Result(b, _)) => ts_ty(*b, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Result[_, _]` \
                     recorded for a `Result.andThen` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __f: (x: {t}) => Result<{b}, {et}>) => __r.tag === \"Ok\" ? __f(__r.value) : __r)({recv}, {f})"
            ))
        }
        (ResultOp::MapErr, [f]) => {
            // The call's checked type is `Result[T, F]` — peel for F.
            let fts = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Result(_, f)) => ts_ty(*f, tys),
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Result[_, _]` \
                     recorded for a `Result.mapErr` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __f: (e: {et}) => {fts}) => __r.tag === \"Err\" ? Err(__f(__r.error)) : __r)({recv}, {f})"
            ))
        }
        (ResultOp::GetOrElse, [fallback]) => {
            let recv = pre.lower(receiver, cx);
            let fallback = pre.lower(fallback, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __d: {t}) => __r.tag === \"Ok\" ? __r.value : __d)({recv}, {fallback})"
            ))
        }
        (ResultOp::IsOk, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"Ok\")"))
        }
        (ResultOp::IsErr, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"Err\")"))
        }
        (ResultOp::Ok, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>) => __r.tag === \"Ok\" ? Some(__r.value) : None)({recv})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// §2.8.3: lower an `Effect[Result[T, E]]` combinator. `Effect[T]` is
/// `Promise<T>` at runtime, so each method is an `async` IIFE that `await`s the
/// receiver Promise and rebuilds the transformed `Result` — the exact
/// desugaring the spec gives (`mapOk` ≡ `map(r => r.map(f))`, etc.), inlined so
/// no runtime helper is needed. `ok`/`err` are the receiver's peeled `Result`
/// parameters; the call's checked type supplies the transformed parameter for
/// the TS annotation.
#[allow(clippy::too_many_arguments)]
fn lower_effect_result_kernel(
    e: &Expr,
    receiver: &Expr,
    op: EffectResultOp,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let t = ts_ty(ok, tys);
    let et = ts_ty(err, tys);
    // The call's checked type is `Effect[Result[a, b]]`; peel `(a, b)` for the
    // transformed parameter's TS type.
    let (a_ts, b_ts) = match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
            Ty::Result(a, b) => (ts_ty(*a, tys), ts_ty(*b, tys)),
            other => panic!(
                "bynk internal error (finding #28): emitter expected `Effect[Result[_, _]]` \
                 recorded for an effectful `Result` combinator call at {:?}, but found \
                 `Effect[{other:?}]` — the checker should already have typed it",
                e.span
            ),
        },
        other => panic!(
            "bynk internal error (finding #28): emitter expected `Effect[Result[_, _]]` \
             recorded for an effectful `Result` combinator call at {:?}, but found {other:?} \
             — the checker should already have typed it",
            e.span
        ),
    };
    let text: Option<String> = match (op, args) {
        (EffectResultOp::MapOk, [f]) => {
            // result `Effect[Result[U, E]]` — `a_ts` is `U`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (x: {t}) => {a_ts}) => {{ const __r = await __e; return __r.tag === \"Ok\" ? Ok(__f(__r.value)) : __r; }})({recv}, {f})"
            ))
        }
        (EffectResultOp::MapErr, [f]) => {
            // result `Effect[Result[T, F]]` — `b_ts` is `F`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (e: {et}) => {b_ts}) => {{ const __r = await __e; return __r.tag === \"Err\" ? Err(__f(__r.error)) : __r; }})({recv}, {f})"
            ))
        }
        (EffectResultOp::FlatMapOk, [f]) => {
            // `f: T -> Effect[Result[U, E]]`; result `Effect[Result[U, E]]`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (x: {t}) => globalThis.Promise<Result<{a_ts}, {et}>>) => {{ const __r = await __e; return __r.tag === \"Ok\" ? await __f(__r.value) : __r; }})({recv}, {f})"
            ))
        }
        (EffectResultOp::FlatMapErr, [f]) => {
            // `f: E -> Effect[Result[T, F]]`; result `Effect[Result[T, F]]`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (e: {et}) => globalThis.Promise<Result<{t}, {b_ts}>>) => {{ const __r = await __e; return __r.tag === \"Err\" ? await __f(__r.error) : __r; }})({recv}, {f})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

/// v0.20b: lower a built-in `Map` kernel method. `insert` copies — the
/// emitted `ReadonlyMap` is never mutated in place; updating an existing key
/// keeps its insertion position (JS `Map` semantics, normative in §7).
/// K6a (#2596): lower a `Set` kernel method over the `ReadonlySet`. `insert`
/// and `remove` copy, so the receiver is never mutated; a copy keeps the
/// insertion order, and inserting a present element keeps its position.
fn lower_set_kernel(
    receiver: &Expr,
    op: SetOp,
    args: &[Expr],
    elem: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let elem_ts = ts_ty(elem, tys);
    let text: Option<String> = match (op, args) {
        (SetOp::Length, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).size"))
        }
        (SetOp::Contains, [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("({recv}).has({x})"))
        }
        (SetOp::Insert, [x]) if !key_is_plain(elem, tys.types()) => {
            cx.runtime_use().note_key("__bynkSetInsert");
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("__bynkSetInsert<{elem_ts}>({recv}, {x})"))
        }
        (SetOp::Remove, [x]) if !key_is_plain(elem, tys.types()) => {
            cx.runtime_use().note_key("__bynkSetRemove");
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("__bynkSetRemove<{elem_ts}>({recv}, {x})"))
        }
        (SetOp::Insert, [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!(
                "(new globalThis.Set<{elem_ts}>({recv}).add({x}) as ReadonlySet<{elem_ts}>)"
            ))
        }
        (SetOp::Remove, [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!(
                "((__s: globalThis.Set<{elem_ts}>) => (__s.delete({x}), __s as ReadonlySet<{elem_ts}>))(new globalThis.Set<{elem_ts}>({recv}))"
            ))
        }
        (SetOp::ToList, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!(
                "([...({recv})] as readonly {}[])",
                ts_array_elem(&elem_ts)
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

fn lower_map_kernel(
    receiver: &Expr,
    op: MapOp,
    args: &[Expr],
    key: TyId,
    val: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    let key_ts = ts_ty(key, tys);
    let val_ts = ts_ty(val, tys);
    let text: Option<String> = match (op, args) {
        (MapOp::Length, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).size"))
        }
        (MapOp::Keys, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...({recv}).keys()]"))
        }
        // v0.149 (ADR 0173): the values in key order — the `keys()` sibling over
        // the in-memory `ReadonlyMap`.
        (MapOp::Values, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...({recv}).values()]"))
        }
        (MapOp::Get, [k]) => {
            let recv = pre.lower(receiver, cx);
            let k = pre.lower(k, cx);
            Some(format!(
                "((__m: ReadonlyMap<{key_ts}, {val_ts}>, __k: {key_ts}) => __m.has(__k) ? Some(__m.get(__k) as {val_ts}) : None)({recv}, {k})"
            ))
        }
        (MapOp::Insert, [k, v]) if !key_is_plain(key, tys.types()) => {
            cx.runtime_use().note_key("__bynkMapInsert");
            let recv = pre.lower(receiver, cx);
            let k = pre.lower(k, cx);
            let v = pre.lower(v, cx);
            Some(format!(
                "__bynkMapInsert<{key_ts}, {val_ts}>({recv}, {k}, {v})"
            ))
        }
        (MapOp::Insert, [k, v]) => {
            let recv = pre.lower(receiver, cx);
            let k = pre.lower(k, cx);
            let v = pre.lower(v, cx);
            Some(format!("new globalThis.Map({recv}).set({k}, {v})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

fn lower_if(
    e: &Expr,
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let cond_lowered = pre.lower_node(cond, cx);
    // If the cond contains `is`-bindings, the then-branch needs a place
    // for the `const name = receiver.field;` declarations — a ternary
    // has no such place. Force the IIFE form. (`ternary_shaped`.)
    if ternary_shaped(cond, then_block, else_block, cx) {
        // T2.1 (R6.2): lower each branch's tail once, then ask the branches
        // themselves whether they hoisted, rather than asking `simple_expr` to
        // predict it.
        //
        // #1029 review: `return_ty` is deliberately **not** cleared here, unlike
        // the IIFE path below. That path clears it because a `return` in its
        // arms exits the arrow rather than the enclosing function, so an
        // embedding `?` must behave like a plain one (ADR 0178). This path
        // inverts that premise: when a branch hoists, `hoist_if_as_statement`
        // puts the statements in the *caller's* statement position, so the `?`
        // expansion's `return` does exit the enclosing function and the declared
        // embedding must still apply. Clearing it emitted a bare `return __rN;`
        // — an `Err` of the operand's error type returned from a function
        // declared with the embedding sum's.
        //
        // Preserving it cannot move the no-hoist branch: `return_ty` is read
        // only by `embed_conversion`, only from the `?` expansion, and a `?`
        // expansion always pushes to `pre` — so where `pre` is empty, nothing
        // read it.
        let then_tail = lower_expr(&then_block.tail, cx);
        let else_tail = lower_expr(&else_block.tail, cx);
        if then_tail.pre.is_empty() && else_tail.pre.is_empty() {
            // The `Paren` keeps today's outer brackets.
            return pre.finish_built(ternary_built(cond_lowered, then_tail.expr, else_tail.expr));
        }
        // A branch hoisted. Before T2.1 there was nowhere to put its statements
        // from here — a value position had no statement slot — and the only
        // shape available was the arrow below, which swallows a hoisted `?`'s
        // early return. `lower_if` now returns its own `pre`, so the `if` hoists
        // as a real statement in the caller's statement position and the
        // `return` keeps exiting the enclosing function. See
        // `hoist_if_as_statement`.
        let slot = hoist_if_value(&mut pre, cond_lowered, then_tail, else_tail, e, cx);
        pre.finish(LoweredExpr::Node(ident_node(&slot)))
    } else {
        // T2.2 (R6.4): isolate the effectfulness flag to just this IIFE's own
        // body. Unlike `lower_match_as_iife`'s `disc_expr` (spliced *outside*
        // the returned arrow, at the call site), `cond`'s *text* is spliced
        // inside this arrow's own `if (...)` below — only its hoisted
        // statements landed in `pre` above. A bare `await` in `cond`'s text
        // would need to feed `needs_async`, not `saved_await`; the
        // debug_assert after the arrow closes documents that this can't
        // happen today (a literal `await` comes only from `EffectLet`/`Do`,
        // both hoisted to `pre`, or from a nested `finish_async_iife` wrap,
        // which would require a value-position `match`/`if` needing the async
        // wrap to itself type as `Bool` — ruled out by the checker's
        // Effect-typing) rather than silently trusting it.
        let saved_await = std::mem::take(&mut cx.emitted_await);
        debug_assert!(
            !match &cond_lowered {
                LoweredExpr::Node(n) => bynk_ts::print_expr(n),
                LoweredExpr::Text(t) => t.clone(),
            }
            .contains("await "),
            "cond lands inside this arrow; its awaits must feed `needs_async`, not `saved_await`"
        );
        // Inject is-binding declarations on the truthy side. #908: scoped to
        // the then-branch only — pushed here, popped once it's emitted —
        // so a read inside resolves to the binding rather than an outer
        // `let` rename, and the registration doesn't leak into the else
        // branch or beyond.
        cx.shadow_scopes.push(HashMap::new());
        let mut is_bindings = Vec::new();
        let mut found = false;
        gather_is_bindings_for_emit(cond, true, cx, &mut is_bindings, &mut found);
        // 1A.12: each branch is built as a `Body` and rendered, or turned into
        // nodes, once both are in hand (its `is` bindings, nodes or text lines,
        // open it); the context saves and restores below
        // sit around the same lowering calls, in the same order, as when each
        // branch was rendered straight into the text.
        let mut then_body = Body::new();
        // The `is` bindings open their branch, before its statements.
        then_body.extend(is_bindings.iter().map(LoweredStmt::item));
        // v0.154 (ADR 0178): a value-position `if` lowers to an IIFE, whose
        // `return`s exit the arrow, so clear the enclosing `return_ty`. #1750:
        // an `if` whose branches hold a `?` no longer reaches here (it becomes a
        // statement, `hoist_value_as_statement`, so the `?` exits the function
        // and keeps its embedding), so nothing inside this arrow reads it.
        let saved = cx.return_ty.take();
        // #1750: the arrow's own tails `return` from the arrow.
        let saved_slot = cx.tail_slot.take();
        // The branches keep their marks (1A.15, Decision D): as nodes the printer
        // records them, and the text fallback below drops them (`render_text`).
        block_body_into(&mut then_body, then_block, cx, false);
        cx.shadow_scopes.pop();
        // #1654: the bindings the condition's falsity proves, scoped to the
        // else-branch the same way.
        cx.shadow_scopes.push(HashMap::new());
        let mut else_bindings = Vec::new();
        gather_is_bindings_for_emit(cond, false, cx, &mut else_bindings, &mut found);
        let mut else_body = Body::new();
        else_body.extend(else_bindings.iter().map(LoweredStmt::item));
        block_body_into(&mut else_body, else_block, cx, false);
        cx.shadow_scopes.pop();
        cx.return_ty = saved;
        cx.tail_slot = saved_slot;
        // #2 review: unlike `lower_match_as_iife`'s IIFE, this one used to never
        // consult the async-wrap decision at all — confirmed live: `let r = if
        // c { let y <- fetch(); y + 1 } else { 0 }` emitted a bare `await`
        // inside this arrow with no `async` keyword, a hard JS syntax error
        // ("Unexpected reserved word"), not merely a missed optimisation.
        let needs_async = cx.emitted_await;
        cx.emitted_await = saved_await || needs_async;
        // The node form: a node condition and branches that are all nodes. An
        // `is` binding that is still text (a variant payload read) keeps the
        // text form below.
        let cond_node = match cond_lowered {
            LoweredExpr::Node(n) => Ok(n),
            LoweredExpr::Text(t) => Err(t),
        };
        if let Ok(cond) = &cond_node
            && let Some(then_stmts) = then_body.to_nodes()
            && let Some(else_stmts) = else_body.to_nodes()
        {
            let branch = bynk_ts::TsStmt::if_else_same_line_stmt(
                cond.clone(),
                bynk_ts::TsStmt::block(then_stmts, None),
                bynk_ts::TsStmt::block(else_stmts, None),
                None,
            );
            return pre.finish(LoweredExpr::Node(iife_call(needs_async, vec![branch])));
        }
        // The text form, byte for byte what it was.
        let cond_expr = match cond_node {
            Ok(n) => bynk_ts::print_expr(&n),
            Err(t) => t,
        };
        let mut iife = String::new();
        iife.push_str("(() => {\n");
        iife.push_str("    if (");
        iife.push_str(&cond_expr);
        iife.push_str(") {\n");
        render_text(&then_body, &mut iife, INDENT_STEP * 3);
        for _ in 0..(INDENT_STEP * 2) {
            iife.push(' ');
        }
        iife.push_str("} else {\n");
        render_text(&else_body, &mut iife, INDENT_STEP * 3);
        for _ in 0..(INDENT_STEP * 2) {
            iife.push(' ');
        }
        iife.push_str("}\n");
        for _ in 0..INDENT_STEP {
            iife.push(' ');
        }
        iife.push_str("})()");
        pre.finish_text(finish_async_iife(iife, needs_async))
    }
}

/// `(() => { <body> })()`, an immediately invoked arrow over statement nodes,
/// `async` and awaited when `is_async` (1A.12). The text counterpart is
/// `finish_async_iife`, now only the `match` IIFE's and the text fallbacks'.
fn iife_call(is_async: bool, body: Vec<bynk_ts::TsStmt>) -> bynk_ts::TsExpr {
    arrow_call(Vec::new(), Vec::new(), is_async, body)
}

/// `((params) => { <body> })(args)`, `async` and awaited when `is_async`: the
/// `match` IIFE's `(__d) => { … }` over its discriminant, and [`iife_call`]'s
/// parameterless one (1A.13).
fn arrow_call(
    params: Vec<bynk_ts::TsParam>,
    args: Vec<bynk_ts::TsExpr>,
    is_async: bool,
    body: Vec<bynk_ts::TsStmt>,
) -> bynk_ts::TsExpr {
    let call = bynk_ts::TsExpr::Call {
        callee: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
            params,
            is_async,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Multiline(body)),
        }))),
        args,
    };
    if is_async {
        bynk_ts::TsExpr::Await(Box::new(call))
    } else {
        call
    }
}

/// True if `e` proves at least one `is` test when true — the tests
/// `gather_is_bindings_for_emit` declares, by the shared rule
/// [`bynk_check::narrowing::matched_is_tests`] (#1654), which descends `!`,
/// `&&`, `||` and `implies` with polarity. Used by `lower_and_with_is` to decide
/// whether the `is`-binding flow applies before doing any lowering.
fn cond_contains_is(e: &Expr) -> bool {
    !bynk_check::narrowing::matched_is_tests(e, true).is_empty()
}

/// Slice 1A.4 (#3701): the identifier lowering, as a [`Built`] (a node, or the
/// text of a branch that still builds a closure or thunk). The branch order is
/// deliberate and unchanged: constants, `Ord`/needs closures, transition
/// records, invariant fields, store cells, the held/store-`Map` and `Log`
/// thunks, `HttpResult`/`QueueResult` constructors, sum variants, the actor-sum
/// binder, then a plain local.
fn lower_ident(e: &Expr, id: &Ident, cx: &mut LowerCtx) -> Built {
    // #2617: a constant is a hoisted, cached function (`emit_consts`), so a
    // name the checker resolved to one reads it with a call. First, since the
    // checker has already ruled out a local or any other meaning of the name.
    if let Some(name) = cx.const_ref(e) {
        return Ok(bynk_ts::TsExpr::Call {
            callee: Box::new(ident_node(&ts_ident(name))),
            args: Vec::new(),
        });
    }
    // Constrained generics slice 1 (DECISION E): a generic function with `Ord`
    // bounds used as a value is instantiated at the reference, as a closure that
    // captures the comparator witnesses.
    if let Some(Callee::FnValue(decl, witnesses)) = cx.commons().callee(e.id)
        && (!witnesses.is_empty()
            || !decl.given.is_empty()
            || cx.commons().exec_plan.fn_needs(decl))
    {
        let tys = cx.ty_render();
        let params: Vec<String> = match cx
            .commons()
            .expr_types
            .get(&e.id)
            .map(|te| (*tys.get(te.ty)).clone())
        {
            Some(Ty::Fn { params, .. }) => params
                .iter()
                .enumerate()
                .map(|(i, p)| format!("__a{i}: {}", ts_ty(*p, tys)))
                .collect(),
            _ => (0..decl.params.len())
                .map(|i| format!("__a{i}: any"))
                .collect(),
        };
        let names: Vec<String> = (0..params.len()).map(|i| format!("__a{i}")).collect();
        let mut args = names.clone();
        // 1A: witness printed for a text caller
        args.extend(print_witnesses(cx.witness_args(decl, witnesses)));
        // #2868: a `given` function closes over the `deps` in scope where it is
        // named.
        if !decl.given.is_empty() {
            args.push(cx.cap_deps_expr().to_string());
        }
        // #2719: a function in the needs-context set closes over the context in
        // scope where it is named.
        if cx.commons().exec_plan.fn_needs(decl) {
            args.push(cx.exec_expr());
        }
        // 1A: text until the closure/query family converts
        return Err(format!(
            "(({}) => {}({}))",
            params.join(", "),
            ts_ident(&id.name),
            args.join(", ")
        ));
    }
    // v0.116: inside a `transition` predicate, the contextual `old`/`new` idents
    // are the pre-/post-commit state records, lowered to their JS names (`new` is
    // reserved, so both are renamed). Field access `old.<f>` reads off the record.
    // Checked first so `old`/`new` never collide with the heuristics below.
    if let Some((old_var, new_var)) = cx.transition_states() {
        if id.name == "old" {
            return Ok(ident_node(old_var));
        }
        if id.name == "new" {
            return Ok(ident_node(new_var));
        }
    }
    // v0.80: inside an invariant predicate, a bare ident naming a state field
    // reads it off the proposed-state value (`s.<field>`). Checked first so a
    // field never collides with the variant-constructor heuristics below.
    if let Some((var, fields)) = cx.invariant_state()
        && fields.contains(&id.name)
    {
        return Ok(member_node(var, &id.name));
    }
    // v0.81: inside a `store`-agent handler, a bare ident naming a `Cell` field
    // reads it off the mutable working state (`__state.<cell>`), so a read after
    // a `:=` write in the same handler sees the written value (read-your-writes).
    if let Some((var, cells)) = cx.agent_store_cells()
        && cells.contains(&id.name)
    {
        return Ok(member_node(var, &id.name));
    }
    // v0.104/v0.105 (real-time track slice 3b): a bare held-`Map` ident used as a
    // value is a lazy `Query` over its **resolved** connections — the persisted
    // `Record<K, connId>` mapped through `resolveConnection`, keeping the present
    // ones. Checked before the persisted-`Map` branch (held maps are excluded from
    // `agent_store_maps`).
    if !cx.is_local(&id.name)
        && let Some(f_ts) = cx.agent_held_map_frame(&id.name)
    {
        let var = cx.agent_store_var();
        // 1A: text until the closure/query family converts
        return Err(format!(
            "(() => {var}.{name}.values().flatMap((__cid) => {{ const __c = __resolveConnection<{f_ts}>(this.state, __cid); return __c.tag === \"Some\" ? [__c.value] : []; }}))",
            name = id.name
        ));
    }
    // v0.94 (ADR 0120): a bare `store Map` ident used as a **value** — not a
    // method receiver (those are handled in the method dispatch) — is a lazy
    // `Query` over the whole map, e.g. the `other` side of a join. It lowers to
    // the same deferred thunk a query builder yields: `() => Object.values(map)`.
    if !cx.is_local(&id.name) && cx.is_agent_store_map(&id.name) {
        let var = cx.agent_store_var();
        // 1A: text until the closure/query family converts
        return Err(format!("(() => {var}.{}.values())", id.name));
    }
    // v0.95 (ADR 0121): a bare `store Log` ident used as a value is a lazy
    // `Query` over its entry values — `() => log.map((__e) => __e.v)`.
    if !cx.is_local(&id.name) && cx.is_agent_store_log(&id.name) {
        let var = cx.agent_store_var();
        // 1A: text until the closure/query family converts
        return Err(format!("(() => {var}.{}.values())", id.name));
    }
    // v0.9: a nullary HttpResult variant (whose checker type is
    // `HttpResult[_]`) constructs an HttpResult.<Variant>.
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // the `Ty::HttpResult`/`Ty::QueueResult` type check plus a bare
    // `http_variant`/`queue_variant` keyword lookup — consistency with
    // every other static-constructor branch in this module, not a new
    // defect closure: the resolved-type guard here was already immune to
    // name-matching (a shadowing local never types as `Ty::HttpResult`).
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == HTTP_RESULT
    ) {
        cx.note_http_result();
        return Ok(member_node("HttpResult", &id.name));
    }
    // v0.44: a nullary QueueResult variant (`Ack`) constructs `QueueResult.Ack`.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == QUEUE_RESULT
    ) {
        cx.note_queue_result();
        return Ok(member_node("QueueResult", &id.name));
    }
    // A bare ident whose name matches a declared variant of a sum
    // type (and whose checker type is that sum) is a nullary
    // variant constructor reference. Qualify it as `Type.Variant`.
    // Otherwise (locals, params, `self`) emit the identifier as-is.
    if let Some(Ty::Named {
        kind: NamedKind::Sum,
        name: type_name,
        ..
    }) = cx.commons().expr_ty(e.id).as_deref()
        && let Some(decl) = cx.commons().types.get(type_name)
        && let TypeBody::Sum(s) = &decl.body
        && s.variants.iter().any(|v| v.name.name == id.name)
    {
        if let Some(lit) = consumed_variant_literal(e, type_name, &id.name, &[], cx) {
            // 1A: text until the consumed-variant literal converts
            return Err(lit);
        }
        return Ok(member_node(type_name, &id.name));
    }
    // v0.52: the multi-actor sum binder is not a runtime local — the resolved
    // actor is threaded through `deps.who` at the boundary wrapper, so the
    // binder ident lowers to it (the tagged union the body `match`es).
    if cx.actor_sum_binder() == Some(id.name.as_str())
        && matches!(cx.commons().expr_ty(e.id).as_deref(), Some(Ty::ActorSum(_)))
    {
        return Ok(member_node("deps", "who"));
    }
    // #908: a re-`let` of this name (in this or an enclosing block) renamed
    // its emitted binding — resolve to it rather than the natural
    // (now-shadowed) `ts_ident` name.
    Ok(ident_node(
        &cx.resolved_local_name(&id.name)
            .unwrap_or_else(|| ts_ident(&id.name)),
    ))
}

/// #2045: on `workers`, a variant of a consumed context's sum built inline —
/// `({ tag: "Held", reason: x } as Status)` — rather than through the callee's
/// companion (`Status.Held(x)`), which would make the callee's module a value
/// import of this Worker (#661). The companion's constructors are pure data,
/// so the literal is the same value; the cast keeps the tag narrow, through
/// the type-only import of `Status`. `None` for any other sum.
fn consumed_variant_literal(
    e: &Expr,
    sum: &str,
    tag: &str,
    args: &[String],
    cx: &LowerCtx,
) -> Option<String> {
    if !cx.is_consumed_context_type(sum) {
        return None;
    }
    let decl = cx.commons().types.get(sum)?;
    let TypeBody::Sum(s) = &decl.body else {
        return None;
    };
    let variant = s.variants.iter().find(|v| v.name.name == tag)?;
    if variant.payload.len() != args.len() {
        return None;
    }
    let ty = cx.commons().expr_types.get(&e.id)?.ty;
    let ts_ty = bynk_ts::print_type(&crate::emitter::ts_types::ts_ty_to_ts_type(
        ty,
        cx.ty_render(),
    ));
    let mut fields = vec![format!("tag: \"{tag}\"")];
    for (p, a) in variant.payload.iter().zip(args) {
        fields.push(format!(
            "{}: {a}",
            crate::emitter::payload_prop(&p.name.name)
        ));
    }
    Some(format!("({{ {} }} as {ts_ty})", fields.join(", ")))
}

/// Lower a variant call's arguments. #2341 (L4): a `RawError`'s first argument
/// is an `ErrorStatus` name the checker resolved, so it lowers to its number
/// (`Forbidden` → `403`), not to the `HttpResult` value of that name.
fn lower_http_variant_args(
    variant: &str,
    args: &[Expr],
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> Vec<LoweredExpr> {
    args.iter()
        .enumerate()
        .map(|(i, a)| {
            if variant == "RawError"
                && i == 0
                && let Some(status) = bynk_syntax::ast::raw_error_status(a)
            {
                LoweredExpr::Node(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Num(
                    status.to_string(),
                )))
            } else {
                pre.lower_node(a, cx)
            }
        })
        .collect()
}

/// #2719 (G): which helper a test `case` that calls a function in the
/// needs-context set directly settles the call with: on virtual time it is
/// driven (auto-advance) and then settled, otherwise the sends it handed the
/// harness's execution context are awaited, so the case's `expect`s see their
/// effects. `None` outside a test case, or when nothing needs settling.
fn settle_helper(cx: &LowerCtx) -> Option<&'static str> {
    if !cx.in_test_case() {
        None
    } else if cx.runtime_use().virtual_time() {
        Some("__bynkSettleVirtual")
    } else if cx.test_settles_sends() {
        Some("__bynkSettle")
    } else {
        None
    }
}

/// [`settle_helper`] over a call built as text (the dispatchers' side).
fn settle_exec_call(call: String, cx: &LowerCtx) -> String {
    match settle_helper(cx) {
        Some(helper) => format!("{helper}(deps, {call})"),
        None => call,
    }
}

/// [`settle_helper`] over a built call: `helper(deps, call)` as a node when the
/// call is one, else today's text.
fn settle_built(call: Built, cx: &LowerCtx) -> Built {
    match (settle_helper(cx), call) {
        (None, call) => call,
        (Some(helper), Ok(node)) => Ok(bynk_ts::TsExpr::Call {
            callee: Box::new(ident_node(helper)),
            args: vec![ident_node("deps"), node],
        }),
        (Some(helper), Err(text)) => Err(format!("{helper}(deps, {text})")),
    }
}

fn lower_call(e: &Expr, name: &Ident, args: &[Expr], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // Bare variant constructor with payload → qualify.
    let mut args_lowered = lower_http_variant_args(&name.name, args, &mut pre, cx);
    // #2620: `unreachable(msg)` / `todo(msg)` lower to a call of the runtime's
    // `never`-returning `__fault`, which throws — valid in any expression
    // position, and inside an async body it rejects the effect.
    if let Some(Callee::Fault { kind }) = cx.commons().callee(e.id) {
        // The location is a string-literal node: the printer escapes it with
        // the same `escape_ts_literal` the text form used.
        let location = cx
            .assert_loc()
            .map(|_| expect_location(cx, e.span.start))
            .or_else(|| cx.fault_location(e.span.start))
            .unwrap_or_else(|| format!("offset {}", e.span.start));
        cx.note_fault();
        args_lowered.insert(0, LoweredExpr::Node(str_node(*kind)));
        args_lowered.push(LoweredExpr::Node(str_node(location)));
        return pre.finish_built(call_built(ident_node("__fault"), args_lowered));
    }
    // #3051: `TestConnection[F]()` — a case's recording handle for a websocket
    // connection.
    if let Some(Callee::Intrinsic {
        ns: "TestConnection",
        ..
    }) = cx.commons().callee(e.id)
    {
        cx.runtime_use().note_test_connection();
        let frame = match cx.commons().expr_ty(e.id).as_deref() {
            Some(Ty::TestConnection(f)) => bynk_ts::print_type(
                &crate::emitter::ts_types::ts_ty_to_ts_type(*f, cx.ty_render()),
            ),
            _ => "unknown".to_string(),
        };
        // 1A: `bynk_ts` `New` has no `type_args` field (the explicit
        // `<frame>` has no node home), so this stays text.
        return pre.finish_text(format!("new TestConnection<{frame}>()"));
    }
    // v0.9: HttpResult variant call.
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // the `Ty::HttpResult`/`Ty::QueueResult` type check plus a bare
    // keyword lookup — consistency with the bare-`Ident` sibling in
    // `lower_ident`, not a new defect closure.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == HTTP_RESULT
    ) {
        cx.note_http_result();
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident_node("HttpResult")),
            property: name.name.clone(),
        };
        return pre.finish_built(call_built(callee, args_lowered));
    }
    // v0.44: a QueueResult variant call (`Retry(reason)`) → `QueueResult.Retry(...)`.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == QUEUE_RESULT
    ) {
        cx.note_queue_result();
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident_node("QueueResult")),
            property: name.name.clone(),
        };
        return pre.finish_built(call_built(callee, args_lowered));
    }
    // P6.21 (real P6.2 cutover, partial — #1143/#1140's `Callee` classification
    // read back instead of re-derived): agent instantiation `AgentName(key)`
    // lowers to the generated `__makeAgentName(key)` factory, which obtains
    // the instance for this key (lookup-or-create against the registry in
    // bundle mode, or a typed DO proxy in workers mode). Skipped when this
    // Call is the receiver of a MethodCall — that path folds construction
    // and the method invocation together. Previously guarded by
    // `cx.local_agents.contains(&name.name)`, a name-matched re-derivation
    // of what the checker's own `Callee::AgentInit` already resolved once —
    // the same defect class `block_writes_state`'s receiver-name matching
    // carried before #1196 converted it to read `Callee::Store` directly.
    if matches!(cx.commons().callee(e.id), Some(Callee::AgentInit(_))) {
        // 1A: `agent_construct` builds text over `cx` state, so the key is
        // printed here.
        let arg_count = args_lowered.len();
        let key_expr = args_lowered
            .into_iter()
            .next()
            .map(LoweredExpr::into_text)
            .unwrap_or_else(|| {
                panic!(
                    "bynk internal error (ADR 0334): Callee::AgentInit records agent \
                     construction `{}(...)` with {arg_count} args, but every AgentDecl \
                     declares exactly one key field — the checker already accepted this call",
                    name.name,
                )
            });
        return pre.finish_text(cx.agent_construct(&name.name, &key_expr));
    }
    // P6.21: sum-variant construction is `Callee::Ctor` — reads the sum
    // type's own name and the variant's own tag straight off the checker's
    // classification instead of `call_is_sum_variant`'s own name-matched
    // `sum_name`/`call_name` string comparison against the call's checked
    // type (the same re-derivation this whole `Callee` precedent, P6.0,
    // exists to close).
    if let Some(Callee::Ctor { sum, tag }) = cx.commons().callee(e.id) {
        if cx.is_consumed_context_type(&sum.name.name) {
            // 1A: a consumed-context variant literal is built as text over the
            // printed arguments; if it declines, the call falls back to text
            // over the same printed arguments.
            let texts: Vec<String> = args_lowered
                .into_iter()
                .map(LoweredExpr::into_text)
                .collect();
            if let Some(lit) = consumed_variant_literal(e, &sum.name.name, tag, &texts, cx) {
                return pre.finish_text(lit);
            }
            return pre.finish_text(format!("{}.{}({})", sum.name.name, tag, texts.join(", ")));
        }
        let callee = bynk_ts::TsExpr::Member {
            object: Box::new(ident_node(&sum.name.name)),
            property: tag.to_string(),
        };
        return pre.finish_built(call_built(callee, args_lowered));
    }
    // Constrained generics slice 1: the call's comparator witnesses follow the
    // ordinary arguments (an `Ord`-bounded callee's hidden trailing parameters).
    let mut needs_exec = false;
    if let Some(Callee::Fn(decl, witnesses)) = cx.commons().callee(e.id) {
        args_lowered.extend(
            cx.witness_args(decl, witnesses)
                .into_iter()
                .map(LoweredExpr::Node),
        );
        // #2868: a function that declares `given` takes the caller's `deps`,
        // which the checker has made carry its capabilities.
        if !decl.given.is_empty() {
            args_lowered.push(LoweredExpr::Node(cx.cap_deps_node()));
        }
        // #2719: a function in the needs-context set takes the context last.
        if cx.commons().exec_plan.fn_needs(decl) {
            args_lowered.push(LoweredExpr::Node(cx.exec_node()));
            needs_exec = true;
        }
    }
    let mut built = call_built(ident_node(&ts_ident(&name.name)), args_lowered);
    if needs_exec {
        built = settle_built(built, cx);
    }
    pre.finish_built(built)
}

/// `true` / `false` as a node.
fn bool_node(value: bool) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(value))
}

/// `!(l)`: a `Unary(Not)` over an explicit `Paren` (Decision C of #3715: the
/// printer's own brackets would give `!x`), or today's `!({l})` text.
fn negated_paren(l: LoweredExpr) -> LoweredExpr {
    node_or_text(
        vec![l],
        |c| bynk_ts::TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(one(c)))),
        },
        |c| format!("!({})", one(c)),
    )
    .into()
}

/// The short-circuit operators over two operands: `l && r`, `l || r`, and
/// `P implies Q` as `(!(P) || Q)` with its explicit brackets kept. A node when
/// both operands are nodes (the printer brackets `&&`/`||` by precedence, as
/// the text's own spelling already did), else today's text. The left operand is
/// one value used once, so it stays textually present for `tsc`'s narrowing
/// (#1044).
fn short_circuit(op: BinOp, l: LoweredExpr, r: LoweredExpr) -> Built {
    let (ts_op, implies) = match op {
        BinOp::And => (bynk_ts::TsBinaryOp::And, false),
        BinOp::Or => (bynk_ts::TsBinaryOp::Or, false),
        BinOp::Implies => (bynk_ts::TsBinaryOp::Or, true),
        _ => unreachable!("only the short-circuit operators reach `short_circuit`"),
    };
    node_or_text(
        vec![l, r],
        |c| {
            let (l, r) = two(c);
            if implies {
                bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Binary {
                    op: ts_op,
                    left: Box::new(bynk_ts::TsExpr::Unary {
                        op: bynk_ts::TsUnaryOp::Not,
                        expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(l))),
                    }),
                    right: Box::new(r),
                }))
            } else {
                bynk_ts::TsExpr::Binary {
                    op: ts_op,
                    left: Box::new(l),
                    right: Box::new(r),
                }
            }
        },
        |c| {
            let (l, r) = two(c);
            match op {
                BinOp::And => format!("{l} && {r}"),
                BinOp::Or => format!("{l} || {r}"),
                _ => format!("(!({l}) || {r})"),
            }
        },
    )
}

/// `(() => { s1; s2; return r; })()` on one line: a `Call` over a bracketed
/// compact-block `Arrow` when `r` and every statement are nodes and each prints
/// on one line, else today's text. The statements are an expression's own
/// hoists (and an `is` test's bindings), which must run only when the operator
/// reaches its right operand. A statement or value that prints over several
/// lines keeps the text, which joins a statement's lines with spaces.
fn compact_iife(stmts: Vec<LoweredStmt>, ret: LoweredExpr) -> Built {
    let one_line = |n: &bynk_ts::TsStmt| !bynk_ts::print_stmt(n, 0).trim_end().contains('\n');
    let ret_one_line = match &ret {
        LoweredExpr::Node(n) => !bynk_ts::print_expr(n).contains('\n'),
        LoweredExpr::Text(_) => false,
    };
    let all_nodes = ret_one_line
        && stmts
            .iter()
            .all(|s| matches!(s, LoweredStmt::Node(n) if one_line(n)));
    if all_nodes {
        let mut body: Vec<bynk_ts::TsStmt> = stmts
            .into_iter()
            .filter_map(|s| match s {
                LoweredStmt::Node(n) => Some(*n),
                LoweredStmt::Text(_) => None,
            })
            .collect();
        if let LoweredExpr::Node(value) = ret {
            body.push(bynk_ts::TsStmt::return_stmt(Some(value), None));
        }
        return Ok(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
                params: Vec::new(),
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Block(body)),
            }))),
            args: Vec::new(),
        });
    }
    let mut s = String::from("(() => { ");
    for p in &stmts {
        for line in p.lines() {
            s.push_str(&line);
            s.push(' ');
        }
    }
    s.push_str(&format!("return {}; }})()", ret.into_text()));
    Err(s)
}

/// A short-circuit operator's `?`-hoisting form (T2.1/T2.3): `let slot:
/// boolean; if (cond) { …; slot = then; } else { …; slot = else; }` as
/// statement nodes when the condition, both tails and everything they hoisted
/// are nodes ([`hoisted_if_nodes`]), else today's text
/// ([`hoist_if_as_statement`]). Returns the slot.
fn hoist_bool_if(
    pre: &mut Pre,
    cond: LoweredExpr,
    then_tail: Lowered,
    else_tail: Lowered,
    cx: &mut LowerCtx,
) -> String {
    let all_nodes = matches!(cond, LoweredExpr::Node(_))
        && [&then_tail, &else_tail].iter().all(|t| {
            matches!(t.expr, LoweredExpr::Node(_))
                && t.pre.iter().all(|s| matches!(s, LoweredStmt::Node(_)))
        });
    if all_nodes {
        let slot = cx.fresh();
        let ty = Some(bynk_ts::TsType::named("boolean"));
        hoisted_if_nodes(pre, &slot, ty, cond, then_tail, else_tail);
        slot
    } else {
        hoist_if_as_statement(
            pre,
            cond.into_text(),
            then_tail,
            else_tail,
            Some("boolean".to_string()),
            cx,
        )
    }
}

fn lower_bin_op(op: BinOp, lhs: &Expr, rhs: &Expr, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    // For `&&` we need to lower `is` bindings into the rhs scope.
    // We handle that here by collecting bindings from lhs, emitting
    // them as `const` declarations before evaluating rhs — but
    // `&&` short-circuits, so simply emitting them inline is wrong.
    // We compile `lhs && (...is binding flow...) rhs` to a function
    // expression: `(lhs && ((bindings) => rhs)())`. Simpler: rely
    // on TypeScript's narrowing for the value-from-is part of
    // `is` patterns. For now, for the special pattern `x is Ok(n)`
    // we lower the rhs assuming the binding `n = x.value` was
    // captured. We use a parenthesised IIFE to scope the binding.
    // `&&` and `implies` with an `is` in the antecedent: its bindings go in the
    // consequent's scope. v0.80: `P implies Q` lowers to `(!(P) || Q)`, and as
    // with `&&` an `is` test in the antecedent binds into the consequent (only
    // reached when the antecedent holds), so both reuse the same is-binding IIFE
    // flow.
    if matches!(op, BinOp::And | BinOp::Implies)
        && let Some(AndWithIs {
            bindings,
            lhs: lhs_lowered,
            rhs: rhs_expr,
            rhs_returns,
            hoisted,
        }) = lower_and_with_is(lhs, rhs, cx)
    {
        pre.extend_text(hoisted);
        let lhs_expr = pre.absorb_node(lhs_lowered);
        if bindings.is_empty() {
            return pre.finish_built(short_circuit(op, lhs_expr, rhs_expr));
        }
        // Every path below emits `rhs` out of line.
        mark_unnarrowed(rhs, cx);
        // T2.3 (R6.3): `bindings` includes rhs's own hoisted statements
        // (`lower_and_with_is`'s doc comment above), so if one of those is a
        // `?`'s propagating early return, the arrow-IIFE below would capture
        // it instead of letting it exit the enclosing function — reuse
        // `hoist_bool_if` (built on T2.1's `if`-hoisting) to hoist this
        // operator as a real `if` statement instead.
        //
        // #1044 review: `hoist_if_as_statement`'s bare `slot` return is right
        // for the general path (no narrowing at stake there — see below), but
        // wrong here. `lhs_expr` is the very `is`-check `lower_if`'s own
        // `gather_is_bindings_for_emit` relies on being *textually present* in
        // the condition it hands to `tsc` — that's what lets `o` narrow to
        // `Hit` for a hoisted `const score = o.score;` in the caller's
        // then-block. Dropping `lhs_expr` for a disconnected `let __rN:
        // boolean` (assigned across an `if`/`else`, not a `tsc`-4.4+ aliased
        // `const`) severs that link and turns a silent `?`-escape miscompile
        // into a hard `tsc --strict` failure on the same input. Re-including
        // `lhs_expr` in the returned text — safe to re-evaluate, since it is
        // always a pure tag/field read on an already-hoisted receiver, never
        // the original (possibly effectful) `is` subject — keeps the
        // conjunction's left side exactly where a caller's narrowing expects
        // to find it; `slot` only stands in for rhs's value, gated by the
        // same `if` that already guards evaluating it. (For `implies` the guard
        // is `lhs_expr` itself, not its negation, for the same reason.)
        if rhs_returns {
            let otherwise = if op == BinOp::And { "false" } else { "true" };
            let slot = hoist_bool_if(
                &mut pre,
                lhs_expr.clone(),
                Lowered {
                    pre: bindings,
                    expr: rhs_expr,
                },
                Lowered::node(bool_node(otherwise == "true")),
                cx,
            );
            return pre.finish_built(short_circuit(
                op,
                lhs_expr,
                LoweredExpr::Node(ident_node(&slot)),
            ));
        }
        // Emit:  lhs && ((() => { const n = ...; return rhs; })())
        let iife: LoweredExpr = compact_iife(bindings, rhs_expr).into();
        return pre.finish_built(short_circuit(op, lhs_expr, paren(iife).into()));
    }
    let l_lowered = pre.lower_node(lhs, cx);
    // #1/#3 review: `&&`/`||`/`implies` short-circuit — `design/bynk-type-
    // system.md:1671-1678` calls this "both a performance property and a
    // safety property … developers can rely on." Lowering rhs into the same
    // `pre` as lhs — the previous behaviour for every case that didn't go
    // through `lower_and_with_is` above (i.e. `||` always, and `&&`/`implies`
    // whenever the antecedent contains no `is`) — hoisted rhs's
    // pre-statements unconditionally, running them even when lhs's value
    // means rhs is never evaluated. Only inline `r.expr` bare when rhs
    // hoists nothing; otherwise wrap it in an IIFE so the hoist runs only
    // when the operator actually reaches rhs.
    //
    // T2.3 (R6.3): if rhs's hoist itself contains a `?`'s propagating early
    // return (`if (__rN.tag === "Err") return __rN;`), a plain
    // `(() => { ...; return expr; })()` wrap would capture that `return`
    // instead of letting it exit the enclosing function — the same class of
    // miscompile `lower_match_as_iife` had. `LowerCtx::emitted_early_return`
    // says whether rhs's own lowering hit that case; when it did, hoist this
    // operator as a real `if` statement via `hoist_bool_if` instead of an
    // expression-position IIFE — the `return` then keeps exiting the
    // enclosing function, and still only runs when the operator actually
    // reaches rhs. An ordinary hoist (no `?` involved) keeps the cheaper arrow
    // form; nothing inside it needs to escape past the arrow, and this is the
    // shape the `947_short_circuit_rhs_hoist` fixture pins.
    if matches!(op, BinOp::And | BinOp::Or | BinOp::Implies) {
        let l = l_lowered;
        let saved_early_return = std::mem::take(&mut cx.emitted_early_return);
        // #1751: `rhs` may be lowered inside an arrow (below), which would scope
        // an `is` receiver temp it introduces away from a branch that reads it
        // (`if !(f(a) is Some(x)) || !(f(b) is Some(y)) { … } else { x + y }`).
        // Hoist the temps' declarations out of the arrow, as `lower_and_with_is`
        // does for `&&` (#1654); a nested operator adds to the outermost list.
        let outermost = cx.is_temp_hoist.is_none();
        if outermost {
            cx.is_temp_hoist = Some(Vec::new());
        }
        let r = lower_expr(rhs, cx);
        if outermost {
            pre.extend_text(cx.is_temp_hoist.take().unwrap_or_default());
        }
        let rhs_returns = cx.emitted_early_return;
        cx.emitted_early_return = saved_early_return || rhs_returns;
        if !r.pre.is_empty() {
            // Both paths below emit `rhs` out of line.
            mark_unnarrowed(rhs, cx);
        }
        if !r.pre.is_empty() && rhs_returns {
            let value = match op {
                BinOp::And => hoist_bool_if(&mut pre, l, r, Lowered::node(bool_node(false)), cx),
                BinOp::Or => hoist_bool_if(&mut pre, l, Lowered::node(bool_node(true)), r, cx),
                BinOp::Implies => hoist_bool_if(
                    &mut pre,
                    negated_paren(l),
                    Lowered::node(bool_node(true)),
                    r,
                    cx,
                ),
                _ => unreachable!("guarded by the outer `matches!`"),
            };
            return pre.finish(LoweredExpr::Node(ident_node(&value)));
        }
        let Lowered {
            pre: r_pre,
            expr: r_expr,
        } = r;
        let rhs_value: LoweredExpr = if r_pre.is_empty() {
            r_expr
        } else {
            compact_iife(r_pre, r_expr).into()
        };
        // `P implies Q` ≡ `!P || Q` (no `is` bindings in the antecedent — that
        // case returned above via `lower_and_with_is`).
        return pre.finish_built(short_circuit(op, l, rhs_value));
    }
    let r_lowered = pre.lower_node(rhs, cx);
    let children = vec![l_lowered, r_lowered];
    // #2622: `Decimal` arithmetic is exact and goes through the runtime; the
    // checker rejects `/` and any mixed operand, so the left operand decides.
    let decimal_helper = matches!(op, BinOp::Add | BinOp::Sub | BinOp::Mul)
        .then(|| {
            cx.commons()
                .expr_types
                .get(&lhs.id)
                .and_then(|te| te.ty.base(tys.types()))
        })
        .flatten()
        .filter(|b| *b == BaseType::Decimal)
        .map(|_| match op {
            BinOp::Add => "__bynkDecimalAdd",
            BinOp::Sub => "__bynkDecimalSub",
            _ => "__bynkDecimalMul",
        });
    // Slice 1A.5 (#3703): each value-operator branch builds a node when both
    // operands are nodes (`node_or_text`), today's text otherwise. The
    // `cx.note_*` side effects fire here, once, on both paths.
    let built = if let Some(helper) = decimal_helper {
        cx.note_decimal(helper);
        helper_call(helper, children)
    } else if op == BinOp::Div {
        // v0.21: division is operand-typed (ADR 0042) — `Float`
        // true-divides; `Int` keeps truncating. The checker rejects
        // mixed operands, so the left operand decides; a missing
        // type entry falls back to the `Int` (truncating) lowering.
        let lhs_is_float = cx
            .commons()
            .expr_types
            .get(&lhs.id)
            .and_then(|te| te.ty.base(tys.types()))
            == Some(BaseType::Float);
        if lhs_is_float {
            plain_binary(op, children)
        } else {
            // #1657: `Int` division by zero traps, a runtime fault, rather than
            // producing `Infinity`/`NaN` as an `Int`. A runtime helper rather than
            // an inline guard, so coverage never reports its fault branch.
            cx.note_int();
            helper_call("__bynkIntDiv", children)
        }
    } else if let Some(helper) = matches!(op, BinOp::Eq | BinOp::NotEq)
        .then(|| cx.commons().expr_types.get(&lhs.id).map(|te| te.ty))
        .and_then(|operand| equality_helper(operand, tys.types(), cx))
    {
        // #1652 (runtime-semantics track §3.1): `==` is structural, dispatched by
        // the operand's type, the same operand-typed shape as `Div` above (and
        // `Bytes`, ADR 0142 D4). The checker rejects mixed operands, so the left
        // operand decides. A primitive operand returns `None` here and falls
        // through to host `===`/`!==` below.
        let call = helper_call(helper, children);
        if op == BinOp::Eq { call } else { negated(call) }
    } else if let Some(operand) = matches!(op, BinOp::Lt | BinOp::LtEq | BinOp::Gt | BinOp::GtEq)
        .then(|| cx.commons().expr_types.get(&lhs.id).map(|te| te.ty))
        .flatten()
        .filter(|t| !is_bare_ordered(*t, tys.types()))
    {
        // Constrained generics slice 1 (DECISION F): an `Ord` type that is not
        // a plain base compares through its comparator. A base type keeps the
        // bare JS operator (and so `Float` keeps its IEEE meaning, `NaN < x` is
        // `false`).
        let cmp = cx.comparator(operand);
        ordered_comparison(op, cmp, children)
    } else {
        plain_binary(op, children)
    };
    match built {
        Ok(node) => pre.finish(LoweredExpr::Node(node)),
        Err(text) => pre.finish_text(text),
    }
}

/// The `TsBinaryOp` for a value operator (the short-circuit operators are
/// lowered as text above and never reach a node).
fn value_binary_op(op: BinOp) -> bynk_ts::TsBinaryOp {
    use bynk_ts::TsBinaryOp as T;
    match op {
        BinOp::Eq => T::StrictEq,
        BinOp::NotEq => T::StrictNotEq,
        BinOp::Lt => T::LessThan,
        BinOp::LtEq => T::LessThanEq,
        BinOp::Gt => T::GreaterThan,
        BinOp::GtEq => T::GreaterThanEq,
        BinOp::Add => T::Add,
        BinOp::Sub => T::Sub,
        BinOp::Mul => T::Mul,
        BinOp::Div => T::Div,
        BinOp::And | BinOp::Or | BinOp::Implies => {
            unreachable!("short-circuit operators are lowered as text before `value_binary_op`")
        }
    }
}

/// `l <op> r`: a `Binary` node (the printer decides the brackets), or today's
/// `{l} {op} {r}` text.
fn plain_binary(op: BinOp, children: Vec<LoweredExpr>) -> Built {
    node_or_text(
        children,
        |c| {
            let (l, r) = two(c);
            bynk_ts::TsExpr::Binary {
                op: value_binary_op(op),
                left: Box::new(l),
                right: Box::new(r),
            }
        },
        |c| {
            let (l, r) = two(c);
            format!("{l} {} {r}", ts_binop(op))
        },
    )
}

/// `helper(l, r)` over a runtime helper's genuine identifier.
fn helper_call(helper: &'static str, children: Vec<LoweredExpr>) -> Built {
    node_or_text(
        children,
        |c| {
            let (l, r) = two(c);
            bynk_ts::TsExpr::Call {
                callee: Box::new(ident_node(helper)),
                args: vec![l, r],
            }
        },
        |c| {
            let (l, r) = two(c);
            format!("{helper}({l}, {r})")
        },
    )
}

/// `!call`: a `Unary(Not)` node over a call node, or today's `!{call}` text.
fn negated(call: Built) -> Built {
    match call {
        Ok(node) => Ok(bynk_ts::TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(node),
        }),
        Err(text) => Err(format!("!{text}")),
    }
}

/// `cmp(l, r) <op> 0` for a non-base `Ord` type. The comparator is a node, so a
/// composite comparator (`__bynkCmpList(__bynkCmpInt)`) is a callee like any
/// other (1A.16 closes the text path 1A.5 left for it).
fn ordered_comparison(op: BinOp, cmp: bynk_ts::TsExpr, children: Vec<LoweredExpr>) -> Built {
    let cmp_text = bynk_ts::print_expr(&cmp);
    node_or_text(
        children,
        |c| {
            let (l, r) = two(c);
            bynk_ts::TsExpr::Binary {
                op: value_binary_op(op),
                left: Box::new(bynk_ts::TsExpr::Call {
                    callee: Box::new(cmp),
                    args: vec![l, r],
                }),
                right: Box::new(num_lit("0".to_string())),
            }
        },
        |c| {
            let (l, r) = two(c);
            format!("{cmp_text}({l}, {r}) {} 0", ts_binop(op))
        },
    )
}

/// The two elements of a two-child builder's argument.
fn two<T>(children: Vec<T>) -> (T, T) {
    let mut it = children.into_iter();
    match (it.next(), it.next(), it.next()) {
        (Some(l), Some(r), None) => (l, r),
        _ => unreachable!("a two-child builder is given exactly two children"),
    }
}

/// Whether a key or operand of this type orders with the bare JS `<`/`>`: a base
/// type other than `Bytes` (a `Uint8Array`), or a refined type over one. Every
/// other `Ord` type goes through a comparator.
fn is_bare_ordered(ty: TyId, tys: &Types) -> bool {
    matches!(
        ty.base(tys),
        Some(
            BaseType::Int
                | BaseType::Float
                | BaseType::String
                | BaseType::Duration
                | BaseType::Instant
        )
    )
}

/// How `==`/`!=` lowers for an operand of a given type. #1652 (runtime-semantics
/// track §3.1).
#[derive(Debug, PartialEq)]
enum EqualityLowering {
    /// Host `===`: every base type except `Bytes` erases to a JS primitive
    /// (`number`/`string`/`boolean`), as does a refined or opaque type over one,
    /// and `()` erases to `undefined`.
    Host,
    /// `__bynkBytesEqual`: `Bytes`, including an opaque type over it, erases to
    /// a `Uint8Array`, which `===` compares by reference (ADR 0142 D4).
    Bytes,
    /// `__bynkDecimalEqual` (#2622): a `Decimal` is a `{ coefficient, scale }`
    /// object that compares by numeric value (`1.20 == 1.2`).
    Decimal,
    /// `__bynkEq`, the runtime's structural walker: records, sums, `Option`,
    /// `Result`, `List`, value-level `Map`, the built-in error records, and type
    /// variables (a generic `T` is not monomorphised, so its runtime shape is
    /// only known to the walker).
    Structural,
}

/// The runtime helper `==`/`!=` calls for an operand type, noting its import, or
/// `None` for a host-`===` operand. A missing type entry takes the walker, which
/// is also correct for primitives.
fn equality_helper(operand: Option<TyId>, tys: &Types, cx: &LowerCtx) -> Option<&'static str> {
    match operand.map(|t| equality_lowering(t, tys)) {
        Some(EqualityLowering::Host) => None,
        Some(EqualityLowering::Bytes) => {
            cx.note_bytes();
            Some("__bynkBytesEqual")
        }
        Some(EqualityLowering::Decimal) => {
            cx.note_decimal("__bynkDecimalEqual");
            Some("__bynkDecimalEqual")
        }
        Some(EqualityLowering::Structural) | None => {
            cx.note_eq();
            Some("__bynkEq")
        }
    }
}

fn equality_lowering(ty: TyId, tys: &Types) -> EqualityLowering {
    match &*tys.get(ty) {
        Ty::Base(BaseType::Bytes)
        | Ty::Named {
            kind: NamedKind::Refined(BaseType::Bytes) | NamedKind::Opaque(BaseType::Bytes),
            ..
        } => EqualityLowering::Bytes,
        Ty::Base(BaseType::Decimal)
        | Ty::Named {
            kind: NamedKind::Refined(BaseType::Decimal) | NamedKind::Opaque(BaseType::Decimal),
            ..
        } => EqualityLowering::Decimal,
        Ty::Base(_)
        | Ty::Unit
        | Ty::Named {
            kind: NamedKind::Refined(_) | NamedKind::Opaque(_),
            ..
        } => EqualityLowering::Host,
        Ty::Named {
            kind: NamedKind::Record | NamedKind::Sum,
            ..
        }
        | Ty::Option(_)
        | Ty::Result(..)
        | Ty::List(_)
        | Ty::Set(_)
        | Ty::Map(..)
        | Ty::HttpResult(_)
        | Ty::HttpOutcome(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Actor(_)
        | Ty::ActorSum(_)
        | Ty::Var(_) => EqualityLowering::Structural,
        // Not equality-supporting: the checker rejects `==` on these
        // (`checker/equality.rs`), so no certified program reaches here. The
        // walker is the safe answer if one ever did.
        Ty::Error
        | Ty::Never
        | Ty::Fn { .. }
        | Ty::Effect(_)
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::TestConnection(_)
        | Ty::Connection(_) => EqualityLowering::Structural,
    }
}

/// How one object-literal field is written: `key: <child>` (taking the next
/// child), or the bare shorthand `key`.
enum FieldSlot {
    Prop(String),
    Shorthand(String),
}

/// The fields of a record literal as `(slot, child)` pairs, lowering each
/// value through `pre`. A shorthand `{ x }` references the binding `x`; if the
/// binding was renamed (a reserved word) it expands to `x: __id_x` — the key is
/// wire format and must keep the source spelling.
fn field_inits(
    fields: &[FieldInit],
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> (Vec<FieldSlot>, Vec<LoweredExpr>) {
    let mut slots = Vec::new();
    let mut children = Vec::new();
    for f in fields {
        match &f.value {
            Some(v) => {
                children.push(pre.lower_node(v, cx));
                slots.push(FieldSlot::Prop(f.name.name.clone()));
            }
            None => {
                let v = ts_ident(&f.name.name);
                if v == f.name.name {
                    slots.push(FieldSlot::Shorthand(f.name.name.clone()));
                } else {
                    children.push(LoweredExpr::Node(ident_node(&v)));
                    slots.push(FieldSlot::Prop(f.name.name.clone()));
                }
            }
        }
    }
    (slots, children)
}

/// `{ k: v, … }` (or, with `spread`, `{ ...base, k: v, … }`): an `Object` node,
/// or today's text. `children` holds the spread base first (when present), then
/// one child per [`FieldSlot::Prop`], in order.
fn object_built(has_spread: bool, slots: Vec<FieldSlot>, children: Vec<LoweredExpr>) -> Built {
    node_or_text(
        children,
        |nodes| {
            let mut nodes = nodes.into_iter();
            let mut entries = Vec::new();
            if has_spread && let Some(base) = nodes.next() {
                entries.push(bynk_ts::TsObjectEntry::Spread(base));
            }
            for slot in &slots {
                entries.push(match slot {
                    FieldSlot::Prop(key) => match nodes.next() {
                        Some(v) => bynk_ts::TsObjectEntry::Prop(key.clone(), v),
                        None => unreachable!("one child per `Prop` slot"),
                    },
                    FieldSlot::Shorthand(key) => bynk_ts::TsObjectEntry::Shorthand(key.clone()),
                });
            }
            bynk_ts::TsExpr::Object {
                entries,
                multiline: false,
            }
        },
        |texts| {
            let mut texts = texts.into_iter();
            let mut parts = Vec::new();
            if has_spread && let Some(base) = texts.next() {
                parts.push(format!("...{base}"));
            }
            for slot in &slots {
                parts.push(match slot {
                    FieldSlot::Prop(key) => match texts.next() {
                        Some(v) => format!("{key}: {v}"),
                        None => unreachable!("one child per `Prop` slot"),
                    },
                    FieldSlot::Shorthand(key) => key.clone(),
                });
            }
            format!("{{ {} }}", parts.join(", "))
        },
    )
}

fn lower_record_construction(
    type_name: &Ident,
    fields: &[FieldInit],
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let (mut slots, mut children) = field_inits(fields, &mut pre, cx);
    // #2607 (Decision C): each declared field the literal leaves out takes its
    // default, written as a self-contained value (`lower_field_default_value`)
    // so it needs nothing imported into this module. The checker has already
    // required every field without a default, and rejected any default that
    // has no such value.
    //
    // 1A: field defaults are front-half text (bynk_check::wire_default). A
    // default is a `Text` child, so a record that takes one is never a node
    // (`node_or_text`) and keeps today's text byte for byte.
    let types = &cx.commons().types;
    if let Some(decl) = types.get(&type_name.name)
        && let TypeBody::Record(r) = &decl.body
    {
        for declared in &r.fields {
            let Some(init) = &declared.init else {
                continue;
            };
            if fields.iter().any(|f| f.name.name == declared.name.name) {
                continue;
            }
            match bynk_check::wire_default::lower_field_default_value(
                init,
                &declared.type_ref,
                types,
            ) {
                Ok(v) => {
                    slots.push(FieldSlot::Prop(declared.name.name.clone()));
                    let text: Built = Err(v);
                    children.push(text.into());
                }
                Err(reason) => unreachable!(
                    "field `{}`'s default passed the checker but has no value: {reason}",
                    declared.name.name
                ),
            }
        }
    }
    let built = object_built(false, slots, children);
    pre.finish_built(built)
}

/// `lower_field_access`'s tail: finish a [`Built`] as a node or as text.
fn built_or_text_lowered(pre: Pre, built: Built) -> Lowered {
    match built {
        Ok(node) => pre.finish(LoweredExpr::Node(node)),
        Err(text) => pre.finish_text(text),
    }
}

fn lower_field_access(e: &Expr, receiver: &Expr, field: &Ident, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    // v0.158 (ADR 0184): `<map>.entries` / `.keys` / `.values` on a `store
    // Map[K, V]` field — the key-exposing lazy queries. Like every store-map
    // query, they lower to a deferred thunk over the working record. `.values`
    // scans `Object.values` (the existing value lift); `.keys` scans
    // `Object.keys`; `.entries` zips both into `{ key, value }` records
    // (`MapEntry[K, V]`). A persisted key is a JS object key (a string), so an
    // `Int`-typed key is decoded back with `Number(...)`; a `String`-typed key
    // is already correct; a refined or opaque key is asserted to its branded
    // type (#1982).
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.is_agent_store_map(&id.name)
        && !cx.is_agent_held_map(&id.name)
        && matches!(
            field.name.as_str(),
            map_query::ENTRIES | map_query::KEYS | map_query::VALUES
        )
    {
        let var = cx.agent_store_var().to_string();
        let m = format!("{var}.{}", id.name);
        return pre.finish_text(match field.name.as_str() {
            map_query::VALUES => format!("(() => {m}.values())"),
            map_query::KEYS => {
                let decoded = decode_map_key(map_key_ty(e, cx), "__k", tys);
                if decoded == "__k" {
                    format!("(() => {m}.keys())")
                } else {
                    format!("(() => __bynkThen({m}.keys(), (__ks) => __ks.map((__k) => {decoded})))")
                }
            }
            // entries
            _ => {
                let decoded = decode_map_key(map_key_ty(e, cx), "__k", tys);
                format!(
                    "(() => __bynkThen({m}.entries(), (__es) => __es.map(([__k, __v]) => ({{ key: {decoded}, value: __v }}))))"
                )
            }
        });
    }
    // v0.9: `HttpResult.Variant` (nullary).
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` instead of
    // `id.name == HTTP_RESULT` — see the `MethodCall` sibling above (this
    // module's own qualified-with-args `HttpResult.Variant(args)` branch)
    // for the full reasoning.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, .. }) if *ns == HTTP_RESULT
        )
    {
        cx.note_http_result();
        return pre.finish(LoweredExpr::Node(member_node("HttpResult", &field.name)));
    }
    // #2073 (with #2045): a qualified nullary variant of a consumed context's
    // sum (`Region.Domestic`), on `workers`, is built inline as its bare form
    // is, so the callee's module stays a type-only import.
    if let ExprKind::Ident(id) = &receiver.kind
        && !cx.is_local(&id.name)
        && let Some(lit) = consumed_variant_literal(e, &id.name, &field.name, &[], cx)
    {
        return pre.finish_text(lit);
    }
    // Agent-handler `self.<key>` rewrite.
    if cx.in_agent_handler()
        && let ExprKind::Ident(id) = &receiver.kind
        && id.name == "self"
        && let Some(k) = cx.agent_key_field()
        && field.name == *k
    {
        // #1818: the key this instance was addressed by. It used to read
        // `this.state.id.toString()` (the Durable Object's hex id on workers,
        // "[object Object]" on bundle) cast to the key's *name*.
        return pre.finish(LoweredExpr::Node(member_node("this", "__key")));
    }
    // v0.45: `<binder>.identity` on a verified actor binding. The binder is not
    // a runtime value; the identity is minted at the verification seam. For the
    // zero-crypto schemes the sealed identity carries no payload, so it lowers
    // to the unit value (`undefined`). Authenticated identities (Bearer/
    // Signature) and the calling-context value arrive with their later slices.
    //
    // v0.47: a Bearer handler's identity is minted at the verification seam and
    // threaded through `deps`, so `<binder>.identity` reads `deps.identity`.
    // Other (unit) identities — `None`/`Internal` actors — carry no payload and
    // stay the unit value `undefined`.
    if field.name == "identity"
        && matches!(
            cx.commons().expr_ty(receiver.id).as_deref(),
            Some(Ty::Actor(_))
        )
    {
        if let (Some(binder), ExprKind::Ident(id)) = (cx.deps_identity_binder(), &receiver.kind)
            && id.name == binder
        {
            return pre.finish(LoweredExpr::Node(member_node("deps", "identity")));
        }
        return pre.finish(LoweredExpr::Node(ident_node("undefined")));
    }
    // #2380 (L5): an `Oidc` binder's `.claims` reads `deps.claims`, which its
    // seam filled from the verified token.
    if field.name == "claims"
        && matches!(
            cx.commons().expr_ty(receiver.id).as_deref(),
            Some(Ty::Actor(_))
        )
        && let (Some(binder), ExprKind::Ident(id)) = (cx.deps_identity_binder(), &receiver.kind)
        && id.name == binder
    {
        return pre.finish(LoweredExpr::Node(member_node("deps", "claims")));
    }
    let r = pre.lower_node(receiver, cx);
    // `.raw` on an opaque value compiles to a TypeScript type
    // assertion back to the base type. The checker has already
    // verified that the receiver is opaque and the call site is
    // inside the defining commons.
    if field.name == RAW
        && let Some(Ty::Named {
            kind: NamedKind::Opaque(base),
            ..
        }) = cx.commons().expr_ty(receiver.id).as_deref()
    {
        // 1A: text until the `as` assertion has a node for a base-type name
        return pre.finish_text(format!("({} as {})", r.into_text(), ts_base(*base)));
    }
    built_or_text_lowered(
        pre,
        node_or_text(
            vec![r],
            |c| bynk_ts::TsExpr::Member {
                object: Box::new(one(c)),
                property: field.name.clone(),
            },
            |c| format!("{}.{}", one(c), field.name),
        ),
    )
}

/// v0.158 (ADR 0184): the key type `K` of a map `.entries`/`.keys` query, read
/// off the field-access result type — `Query[MapEntry[K, V]]` for `.entries`,
/// `Query[K]` for `.keys`. Used to decode persisted string object-keys back to
/// `K`. `None` only when the accessor's type was not recorded — which the
/// checker never leaves unset for a well-typed `.entries`/`.keys` (it stamps the
/// result type at `e.span`), so a `None` here is a checker/emitter invariant
/// break, not a runtime input. The `debug_assert!` in [`decode_map_key`] pins
/// that: the fallback identity decode must never be reached for an `Int` key,
/// which would silently emit a string where a `number` is expected.
fn map_key_ty(e: &Expr, cx: &LowerCtx) -> Option<TyId> {
    let tys = cx.ty_render();
    match &*tys.get(cx.commons().expr_types.get(&e.id).map(|te| te.ty)?) {
        Ty::Query(inner) => match &*tys.get(*inner) {
            Ty::Named { name, args, .. } if name == MAP_ENTRY && !args.is_empty() => Some(args[0]),
            _ => Some(*inner),
        },
        _ => None,
    }
}

/// v0.158 (ADR 0184): decode a persisted map key (a JS object key, always a
/// string) back to its bynk type. Value-keyable keys are `Int`/`String` and
/// refinements/opaques over them (ADR 0110 D5); an `Int`-based key erases to
/// `number`, so it is parsed with `Number(...)`, while a `String`-based key is
/// already correct and passes through unchanged. A refined or opaque key is
/// then asserted back to its branded type (#1982).
fn decode_map_key(k: Option<TyId>, raw: &str, tys: TyRender<'_>) -> String {
    // The key type is always recorded for a well-typed `.entries`/`.keys` — a
    // `None` means the checker failed to stamp it, and the identity fallback
    // below would silently emit a string for an `Int` key. Pin the invariant in
    // debug builds so it can never become load-bearing by accident.
    debug_assert!(
        k.is_some(),
        "map key type unrecorded — `.entries`/`.keys` key decode would fall back to identity",
    );
    let (base, branded) = match k.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Base(b)) => (Some(*b), false),
        Some(Ty::Named {
            kind: NamedKind::Refined(b) | NamedKind::Opaque(b),
            ..
        }) => (Some(*b), true),
        _ => (None, false),
    };
    // #1982: a refined or opaque key is a branded TS type (`Sku = string & {
    // readonly __brand: "Sku" }`), which neither the raw string nor its
    // `Number(...)` is. The key was that type when it was put, and its storage
    // is the base value, so asserting it back is sound.
    let brand = |decoded: String| match (branded, k) {
        (true, Some(k)) => format!("({decoded} as {})", ts_ty(k, tys)),
        _ => decoded,
    };
    match base {
        Some(BaseType::Int) => brand(format!("globalThis.Number({raw})")),
        Some(BaseType::String) => brand(raw.to_string()),
        // #70/#71 review: `resolver.rs`'s `check_map_key_keyable` and
        // `validate.rs`'s `@indexed`-field check both gate value-keyability
        // (ADR 0110 D5) on exactly `String`/`Int` — the same pair this match
        // now names explicitly. If either of those independent copies ever
        // widens the accepted set without this one being taught the new
        // type's wire shape, silently falling through to bare `raw` would
        // *miscompile* rather than reject — e.g. a `Bool` key needs
        // `raw === "true"`, not the raw string. Fail loudly instead.
        Some(other) => unreachable!(
            "map key base type {other:?} is not value-keyable (ADR 0110 D5) — \
             decode_map_key has no wire-decode case for it"
        ),
        None => raw.to_string(),
    }
}

#[cfg(test)]
mod decode_map_key_tests {
    use super::*;
    use crate::emitter::ts_types::no_spellings;

    /// One table for this module's fixtures — `Types` is `Send + Sync`
    /// (T3.6b), so a `LazyLock` static serves every test without threading.
    static TYS: std::sync::LazyLock<Arc<Types>> =
        std::sync::LazyLock::new(|| Arc::new(Types::new()));

    #[test]
    fn int_key_is_parsed_with_number() {
        let int = TYS.intern(Ty::Base(BaseType::Int));
        assert_eq!(
            decode_map_key(Some(int), "k", TyRender::new(&TYS, no_spellings())),
            "globalThis.Number(k)"
        );
    }

    #[test]
    fn string_key_passes_through() {
        let string = TYS.intern(Ty::Base(BaseType::String));
        assert_eq!(
            decode_map_key(Some(string), "k", TyRender::new(&TYS, no_spellings())),
            "k"
        );
    }

    #[test]
    fn refined_int_key_is_parsed_with_number() {
        let refined = TYS.intern(Ty::Named {
            name: "UserId".to_string(),
            kind: NamedKind::Refined(BaseType::Int),
            args: Vec::new(),
        });
        assert_eq!(
            decode_map_key(Some(refined), "k", TyRender::new(&TYS, no_spellings())),
            "(globalThis.Number(k) as UserId)"
        );
    }

    #[test]
    fn opaque_string_key_is_asserted_to_its_brand() {
        // #1982: `Object.keys` yields `string`, not the branded `Slug`.
        let opaque = TYS.intern(Ty::Named {
            name: "Slug".to_string(),
            kind: NamedKind::Opaque(BaseType::String),
            args: Vec::new(),
        });
        assert_eq!(
            decode_map_key(Some(opaque), "k", TyRender::new(&TYS, no_spellings())),
            "(k as Slug)"
        );
    }

    // #70/#71 review: a non-`String`/`Int` base reaching here means the
    // value-keyable set widened elsewhere (`resolver.rs`/`validate.rs`)
    // without teaching this decode the new type's wire shape — it must fail
    // loudly, not silently emit a wrong pass-through.
    #[test]
    #[should_panic(expected = "is not value-keyable")]
    fn a_widened_non_string_int_base_panics_instead_of_silently_passing_through() {
        let b = TYS.intern(Ty::Base(BaseType::Bool));
        decode_map_key(Some(b), "k", TyRender::new(&TYS, no_spellings()));
    }
}

#[cfg(test)]
mod value_text_for_is_tests {
    use super::*;
    use bynk_syntax::ast::Ident;
    use bynk_syntax::span::Span;

    fn expr(kind: ExprKind) -> Expr {
        Expr {
            id: ExprId::SYNTHETIC,
            kind,
            span: Span::new(0, 0),
        }
    }

    fn ident(name: &str) -> Ident {
        Ident {
            name: name.to_string(),
            span: Span::new(0, 0),
        }
    }

    #[test]
    fn a_simple_receiver_renders_as_its_lvalue() {
        let field = expr(ExprKind::FieldAccess {
            receiver: Box::new(expr(ExprKind::Paren(Box::new(expr(ExprKind::Ident(
                ident("b"),
            )))))),
            field: ident("room"),
        });
        assert_eq!(value_text_for_is(&field), "b.room");
    }

    // #1668 review: a complex receiver reaching here means it was not lifted
    // to a temp before its bindings were gathered. It must fail loudly, not
    // fall back to emitted text, which is what the arm used to do.
    #[test]
    #[should_panic(expected = "bynk internal error (#1668)")]
    fn a_complex_receiver_panics_instead_of_emitting_a_placeholder() {
        value_text_for_is(&expr(ExprKind::IntLit {
            value: 1,
            lexeme: "1".to_string(),
        }));
    }
}

fn lower_lambda(e: &Expr, lambda: &LambdaExpr, cx: &mut LowerCtx) -> Built {
    let tys = cx.ty_render();
    let is_async = matches!(
        cx.commons().expr_ty(e.id).as_deref(),
        Some(bynk_check::checker::Ty::Fn { ret, .. }) if ret.is_effect(tys.types())
    );
    let prefix = if is_async { "async " } else { "" };
    // #2712: a record-pattern parameter has no annotation to print; its type is
    // the checked function type's, so a lambda with nothing to type it from
    // (held in a `let`) still compiles under `tsc --strict`. #3228: the same
    // holds for any unannotated parameter, which TypeScript can type from
    // context only when the lambda is handed straight to a typed parameter: a
    // spread element (`[...acc, (x) => …]`) or an immediately invoked arrow
    // (`((x) => …)(__x)`) gives it none, and `--strict` rejects the implicit
    // `any` (TS7006). So every parameter is annotated with its checked type.
    let fn_params = match cx.commons().expr_ty(e.id).as_deref() {
        Some(bynk_check::checker::Ty::Fn { params, .. }) => params.clone(),
        _ => Vec::new(),
    };
    let params: Vec<String> = lambda
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| match (&p.type_ref, &p.pattern, fn_params.get(i)) {
            (Some(tr), _, _) => format!("{}: {}", ts_ident(&p.name.name), ts_type_ref(tr)),
            (None, _, Some(ty)) => format!("{}: {}", ts_ident(&p.name.name), ts_ty(*ty, tys)),
            _ => ts_ident(&p.name.name),
        })
        .collect();
    let params = params.join(", ");
    // 1A.12: the same parameters as nodes, when each type prints through the
    // type builders exactly as the text above did; otherwise the lambda stays
    // text.
    let node_params: Option<Vec<bynk_ts::TsParam>> = lambda
        .params
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let ty = match (&p.type_ref, &p.pattern, fn_params.get(i)) {
                (Some(tr), _, _) => {
                    let ty = ts_type_ref_to_ts_type(tr, None);
                    (bynk_ts::print_type(&ty) == ts_type_ref(tr)).then_some(Some(ty))
                }
                (None, _, Some(ty)) => Some(Some(ts_ty_to_ts_type(*ty, tys))),
                _ => Some(None),
            }?;
            Some(bynk_ts::TsParam {
                name: ts_ident(&p.name.name),
                ty,
                optional: false,
            })
        })
        .collect();
    // #908: a param's name is the lambda body's own binding — push a frame and
    // register each so a body read resolves to the param rather than an
    // outer `let` rename, popped once the body is lowered so the
    // registration doesn't leak past the lambda.
    cx.shadow_scopes.push(HashMap::new());
    cx.lambda_depth += 1;
    for p in &lambda.params {
        cx.declare_binder(&p.name.name);
    }
    // v0.154 (ADR 0178): a lambda is its own return scope — a `return` in its
    // body (from a `?`) exits the arrow, not the enclosing function. Rebind
    // `return_ty` to the lambda's own return (its `Fn` type's `ret`) so an
    // embedding `?` anywhere in the body — block-bodied or expression-bodied —
    // uses that, not the outer one; restore after either form.
    let lam_ret = match cx.commons().expr_ty(e.id).as_deref() {
        Some(bynk_check::checker::Ty::Fn { ret, .. }) => Some(*ret),
        _ => None,
    };
    let saved = cx.return_ty.take();
    cx.return_ty = lam_ret;
    // #1750: the lambda's own tails `return` from the lambda.
    let saved_slot = cx.tail_slot.take();
    // #1764 review: an enclosing short-circuit's temp hoist stops here. A
    // receiver temp inside the lambda is read only inside it, and hoisting its
    // declaration out would share one slot between the lambda's invocations.
    let saved_hoist = cx.is_temp_hoist.take();
    let result: Built = match &lambda.body.kind {
        ExprKind::Block(b) => {
            // The body keeps its marks (1A.15, Decision D).
            let body = block_body(b, cx, is_async);
            match (node_params, body.to_nodes()) {
                (Some(params), Some(stmts)) => Ok(lambda_node(
                    params,
                    is_async,
                    bynk_ts::TsArrowBody::Multiline(stmts),
                )),
                _ => {
                    let mut out = format!("{prefix}({params}) => {{\n");
                    render_text(&body, &mut out, INDENT_STEP * 2);
                    for _ in 0..INDENT_STEP {
                        out.push(' ');
                    }
                    out.push('}');
                    Err(out)
                }
            }
        }
        _ => {
            let lowered = lower_expr(&lambda.body, cx);
            match (node_params, lowered) {
                (
                    Some(params),
                    Lowered {
                        pre: hoisted,
                        expr: LoweredExpr::Node(value),
                    },
                ) if hoisted.iter().all(|s| matches!(s, LoweredStmt::Node(_))) => {
                    let body = if hoisted.is_empty() {
                        // An object-literal body is bracketed by the printer.
                        bynk_ts::TsArrowBody::Expr(Box::new(value))
                    } else {
                        let mut stmts: Vec<bynk_ts::TsStmt> = hoisted
                            .into_iter()
                            .filter_map(|s| match s {
                                LoweredStmt::Node(n) => Some(*n),
                                LoweredStmt::Text(_) => None,
                            })
                            .collect();
                        stmts.push(bynk_ts::TsStmt::return_stmt(Some(value), None));
                        bynk_ts::TsArrowBody::Multiline(stmts)
                    };
                    Ok(lambda_node(params, is_async, body))
                }
                (_, lowered) => {
                    let body_stmts = lowered.pre_texts();
                    let body = lowered.expr.into_text();
                    Err(if body_stmts.is_empty() {
                        // An object-literal body (a record construction/spread) must be
                        // parenthesised, or `(x) => { … }` reads as a block, not an object.
                        let body = if body.trim_start().starts_with('{') {
                            format!("({body})")
                        } else {
                            body
                        };
                        format!("{prefix}({params}) => {body}")
                    } else {
                        let mut out = format!("{prefix}({params}) => {{\n");
                        for s in &body_stmts {
                            out.push_str(s);
                            out.push('\n');
                        }
                        out.push_str(&format!("  return {body};\n}}"));
                        out
                    })
                }
            }
        }
    };
    cx.is_temp_hoist = saved_hoist;
    cx.tail_slot = saved_slot;
    cx.return_ty = saved;
    cx.lambda_depth -= 1;
    cx.shadow_scopes.pop();
    result
}

/// A lambda as an arrow node (1A.12).
fn lambda_node(
    params: Vec<bynk_ts::TsParam>,
    is_async: bool,
    body: bynk_ts::TsArrowBody,
) -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::Arrow {
        params,
        is_async,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(body),
    }
}

fn lower_record_spread(base: &Expr, overrides: &[FieldInit], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    let mut children = vec![pre.lower_node(base, cx)];
    let (slots, field_children) = field_inits(overrides, &mut pre, cx);
    children.extend(field_children);
    let built = object_built(true, slots, children);
    pre.finish_built(built)
}

fn lower_block_as_expr(b: &Block, cx: &mut LowerCtx) -> Built {
    // #1750: the arrow's own tails `return` from the arrow.
    let saved_slot = cx.tail_slot.take();
    // IIFE is a synchronous arrow function; the surrounding expression context
    // expects a concrete value, so `Effect.pure(...)` must still wrap as
    // `Promise.resolve(...)`.
    // v0.154 (ADR 0178): a `return` here exits the arrow, not the function, so
    // clear `return_ty`. #1750: a block holding a `?` no longer reaches here
    // (`hoist_value_as_statement`).
    let saved = cx.return_ty.take();
    // The body keeps its marks (1A.15, Decision D).
    let body = block_body(b, cx, false);
    cx.return_ty = saved;
    cx.tail_slot = saved_slot;
    if let Some(stmts) = body.to_nodes() {
        return Ok(iife_call(false, stmts));
    }
    let mut iife = String::new();
    iife.push_str("(() => {\n");
    render_text(&body, &mut iife, INDENT_STEP * 2);
    for _ in 0..INDENT_STEP {
        iife.push(' ');
    }
    iife.push_str("})()");
    Err(iife)
}

/// #1 review: the discriminant's hoisted statements (`pre`) are the
/// caller's own — pushed straight into whatever scope is lowering this
/// `match` expression, exactly like every other `ExprKind` arm in
/// `lower_expr_into`. This used to lower the discriminant into a *local*,
/// throwaway vector and — when non-empty — splice it into a freshly
/// synthesised wrapper arrow via `(() => { ...stmts; return inner; })()`. A
/// `?` in the discriminant hoists `if (__rN.tag === "Err") return __rN;`; a
/// `return` inside that wrapper arrow exits the wrapper, not the enclosing
/// function, so the early return's `Err` silently became the whole match
/// expression's *value* instead of propagating out — a miscompile with no
/// diagnostic and no assertion, unlike its two ternary-path siblings (see
/// `lower_if`/`lower_tail_expr`, which at least `debug_assert!`ed the
/// analogous case before this same class of fix).
fn lower_match_as_iife(discriminant: &Expr, arms: &[MatchArm], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    let disc_ty = cx
        .commons()
        .expr_types
        .get(&discriminant.id)
        .map(|te| te.ty);
    let disc = pre.lower_node(discriminant, cx);
    // v0.154 (ADR 0178): a value-position `match` lowers to an IIFE, whose
    // `return`s exit the arrow, so clear `return_ty`. #1750: a `match` whose
    // arms hold a `?` no longer reaches here (`hoist_value_as_statement`).
    let saved = cx.return_ty.take();
    // #1750: the arrow's own tails `return` from the arrow.
    let saved_slot = cx.tail_slot.take();
    // T2.2 (R6.4): isolate the effectfulness flag to just this IIFE's own
    // body — `disc` was already lowered above, so any await it needed landed
    // in `pre`, not here. Restore as `saved_await || needs_async` so an
    // `await (async …)()` this call produces (if it does) still marks the
    // *caller's* scope — the propagation the old text scan got for free from
    // the substring surviving in the returned string.
    let saved_await = std::mem::take(&mut cx.emitted_await);
    // The arrow's body keeps its marks (1A.15, Decision D).
    let mut body = Body::new();
    match_dispatch(&mut body, &PatPath::ident("__d"), &disc_ty, arms, cx, false);
    let needs_async = cx.emitted_await;
    cx.emitted_await = saved_await || needs_async;
    cx.return_ty = saved;
    cx.tail_slot = saved_slot;
    // The node form (1A.13): a node discriminant and a body that is all nodes.
    let disc = match disc {
        LoweredExpr::Node(n) => Ok(n),
        LoweredExpr::Text(t) => Err(t),
    };
    if let Ok(disc) = &disc
        && let Some(stmts) = body.to_nodes()
    {
        let params = vec![bynk_ts::TsParam {
            name: "__d".to_string(),
            ty: None,
            optional: false,
        }];
        return pre.finish(LoweredExpr::Node(arrow_call(
            params,
            vec![disc.clone()],
            needs_async,
            stmts,
        )));
    }
    // The text form, byte for byte what it was.
    let disc = match disc {
        Ok(n) => bynk_ts::print_expr(&n),
        Err(t) => t,
    };
    let mut iife = String::new();
    iife.push_str("((__d) => {\n");
    render_text(&body, &mut iife, INDENT_STEP * 2);
    for _ in 0..INDENT_STEP {
        iife.push(' ');
    }
    iife.push_str(&format!("}})({disc})"));
    pre.finish_text(finish_async_iife(iife, needs_async))
}

/// v0.9.2: a match/if lowered to an IIFE in *expression* position may have arms
/// that `await` (an effectful `let x <- …` or `do …`). A synchronous arrow can't
/// host `await`, so when `needs_async` says the body just constructed emitted one
/// (T2.2, R6.4: a flag set at the two statement sites that emit a literal
/// `await`, read by the caller — see `LowerCtx::emitted_await` — not a scan of
/// `iife` for the substring `"await "`), make the outermost arrow `async` and
/// `await` its call. A nested match/if that already did this leaves its own
/// `await (async …)()` in the body text *and* restores the flag on `cx` before
/// returning, so the caller's own read-and-reset still sees it and the
/// transform still propagates outward.
fn finish_async_iife(iife: String, needs_async: bool) -> String {
    if !needs_async {
        return iife;
    }
    let async_iife = if let Some(rest) = iife.strip_prefix("((__d) =>") {
        format!("(async (__d) =>{rest}")
    } else if let Some(rest) = iife.strip_prefix("(() => {") {
        format!("(async () => {{{rest}")
    } else {
        // Defensive: unreachable given the two current callers, which always
        // build one of the two headers above. `needs_async` is authoritative
        // that `iife` holds a literal `await`, so falling through here emits a
        // bare `await` in a synchronous arrow — a hard `SyntaxError`, not a
        // missed optimisation. Fail here instead of at `tsc`.
        debug_assert!(false, "finish_async_iife: unrecognised IIFE header: {iife}");
        return iife;
    };
    format!("await {async_iife}")
}

/// A literal pattern's value as a node (1A.13, Decision C of #3714): the twin of
/// [`literal_case_label`], which prints byte for byte the same (`TsLit::Str`
/// shares `escape_ts_string`'s escaper, `TsLit::Num` is verbatim).
pub(super) fn literal_label_node(value: &LiteralValue) -> bynk_ts::TsExpr {
    use bynk_ts::{TsExpr, TsLit};
    match value {
        LiteralValue::Int(n) => num_lit(n.to_string()),
        LiteralValue::Str(s) => TsExpr::Lit(TsLit::Str(s.clone())),
        LiteralValue::Bool(b) => TsExpr::Lit(TsLit::Bool(*b)),
    }
}

/// The runtime path a pattern is tested against (`__d`, `x.f`, `x[0].g`): its
/// text, which is what the `is` lowering and every text fallback use, and its
/// node when the scrutinee lowered to one (1A.13, Decision C of #3714). A step
/// keeps the node only when it prints exactly as the text step reads, so the
/// node form can never differ from the text form.
#[derive(Clone)]
pub(super) struct PatPath {
    text: String,
    node: Option<bynk_ts::TsExpr>,
}

impl PatPath {
    /// A path known only as text: every test and binding built over it is text.
    pub(super) fn from_text(text: &str) -> Self {
        PatPath {
            text: text.to_string(),
            node: None,
        }
    }

    /// A bare local.
    pub(super) fn ident(name: &str) -> Self {
        PatPath {
            text: name.to_string(),
            node: Some(ident_node(name)),
        }
    }

    pub(super) fn from_lowered(value: LoweredExpr) -> Self {
        match value {
            LoweredExpr::Node(n) => PatPath {
                text: bynk_ts::print_expr(&n),
                node: Some(n),
            },
            LoweredExpr::Text(text) => PatPath { text, node: None },
        }
    }

    pub(super) fn as_str(&self) -> &str {
        &self.text
    }

    pub(super) fn node(&self) -> Option<&bynk_ts::TsExpr> {
        self.node.as_ref()
    }

    /// The path as a bridge value.
    pub(super) fn lowered(&self) -> LoweredExpr {
        let built: Built = match &self.node {
            Some(n) => Ok(n.clone()),
            None => Err(self.text.clone()),
        };
        built.into()
    }

    /// `path.property`.
    pub(super) fn member(&self, property: &str) -> Self {
        let text = format!("{}.{property}", self.text);
        let node = self
            .node
            .as_ref()
            .map(|n| bynk_ts::TsExpr::Member {
                object: Box::new(n.clone()),
                property: property.to_string(),
            })
            .filter(|n| bynk_ts::print_expr(n) == text);
        PatPath { text, node }
    }

    /// `path[i]`.
    pub(super) fn index(&self, i: usize) -> Self {
        let text = format!("{}[{i}]", self.text);
        let node = self
            .node
            .as_ref()
            .map(|n| bynk_ts::TsExpr::Index {
                object: Box::new(n.clone()),
                index: Box::new(num_lit(i.to_string())),
            })
            .filter(|n| bynk_ts::print_expr(n) == text);
        PatPath { text, node }
    }
}

/// `path <op> rhs`, a node when the path is one, else the `text` the caller
/// built from the same pieces.
fn path_test(
    path: &PatPath,
    op: bynk_ts::TsBinaryOp,
    rhs: bynk_ts::TsExpr,
    text: String,
) -> LoweredExpr {
    let built: Built = match path.node() {
        Some(n) => Ok(bynk_ts::TsExpr::Binary {
            op,
            left: Box::new(n.clone()),
            right: Box::new(rhs),
        }),
        None => Err(text),
    };
    built.into()
}

/// `a && b && …`: a left-leaning `And` chain when every test is a node, else
/// today's ` && `-joined text. `tests` is not empty.
pub(super) fn and_join(tests: Vec<LoweredExpr>) -> LoweredExpr {
    let built = node_or_text(
        tests,
        |nodes| {
            nodes
                .into_iter()
                .reduce(|left, right| bynk_ts::TsExpr::Binary {
                    op: bynk_ts::TsBinaryOp::And,
                    left: Box::new(left),
                    right: Box::new(right),
                })
                .unwrap_or_else(|| unreachable!("and_join is given at least one test"))
        },
        |texts| texts.join(" && "),
    );
    built.into()
}

/// v0.130: render a literal pattern's value as a JS `switch` case label.
pub(super) fn literal_case_label(value: &LiteralValue) -> String {
    match value {
        LiteralValue::Int(n) => n.to_string(),
        LiteralValue::Str(s) => format!("\"{}\"", escape_ts_string(s)),
        LiteralValue::Bool(b) => b.to_string(),
    }
}

/// #472: the `Int`/`String`/`Bool` base of a literal-kind type, or `None` for
/// anything else. Mirrors the checker's `literal_base_of` (and the
/// per-variant logic in [`is_literal_match`]) — used to feed
/// [`refined_check_as_bool`] the base a `_ where predicate` pattern's
/// scrutinee actually has.
fn literal_base_of_ty(ty: TyId, tys: &Arc<Types>) -> Option<BaseType> {
    let base = match &*tys.get(ty) {
        Ty::Base(b) => *b,
        Ty::Named {
            kind: NamedKind::Refined(b),
            ..
        } => *b,
        _ => return None,
    };
    matches!(base, BaseType::Int | BaseType::String | BaseType::Bool).then_some(base)
}

/// #2641: the type of field `field` of the record at static type `path_ty`, at
/// that type's arguments. `None` when `path_ty` is not a known record (a loose
/// scrutinee), so a nested multi-field variant under it falls back as it
/// does for any untyped path.
fn record_field_ty(path_ty: Option<TyId>, field: &str, cx: &LowerCtx) -> Option<TyId> {
    let tys = cx.ty_render();
    let ty = path_ty?;
    let Ty::Named {
        name,
        kind: NamedKind::Record,
        args,
    } = &*tys.get(ty)
    else {
        return None;
    };
    let types = &cx.commons().types;
    let decl = types.get(name)?;
    let TypeBody::Record(r) = &decl.body else {
        return None;
    };
    let f = r.fields.iter().find(|f| f.name.name == field)?;
    bynk_check::checker::instantiate_field_ty(decl, args, &f.type_ref, types, tys.types())
}

/// #2680: the element type of the list at static type `path_ty`.
fn list_elem_ty(path_ty: Option<TyId>, cx: &LowerCtx) -> Option<TyId> {
    match &*cx.commons().tys().get(path_ty?) {
        Ty::List(elem) => Some(*elem),
        _ => None,
    }
}

/// #2680: the expression a list pattern's `..rest` binds, given the list at
/// `path` and the `n` elements before it. `[..rest]` binds the list itself:
/// lists are never mutated in place, so sharing it is unobservable. Any other
/// rest copies the remaining elements, O(length - n).
fn list_rest_expr(path: &str, n: usize) -> String {
    if n == 0 {
        path.to_string()
    } else {
        format!("{path}.slice({n})")
    }
}

/// Emit the boolean tests that must hold for `pattern` to match the value at
/// runtime `path` (static type `path_ty`), each a node when the path is one and the
/// test builds from nodes (1A.13, Decision C of #3714), else text. Irrefutable
/// patterns add nothing; nested variant/literal payloads recurse into
/// `path.<field>`.
pub(super) fn pattern_tests(
    path: &PatPath,
    path_ty: Option<TyId>,
    pattern: &Pattern,
    cx: &LowerCtx,
    tests: &mut Vec<LoweredExpr>,
) {
    use bynk_ts::{TsBinaryOp, TsExpr, TsLit};
    let tys = cx.ty_render();
    let p = path.as_str();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Binding(_) => {}
        Pattern::Literal { value, .. } => {
            tests.push(path_test(
                path,
                TsBinaryOp::StrictEq,
                literal_label_node(value),
                format!("{p} === {}", literal_case_label(value)),
            ));
        }
        // #2699: `x >= lo && x <= hi`, or the one side an open range has.
        Pattern::Range(r) => {
            if let Some(lo) = r.lo {
                tests.push(path_test(
                    path,
                    TsBinaryOp::GreaterThanEq,
                    num_lit(lo.to_string()),
                    format!("{p} >= {}", lo),
                ));
            }
            if let Some(hi) = r.hi {
                tests.push(path_test(
                    path,
                    TsBinaryOp::LessThanEq,
                    num_lit(hi.to_string()),
                    format!("{p} <= {}", hi),
                ));
            }
        }
        // #472: `p where predicate` — the inner pattern's own tests, then the
        // predicate as a boolean guard over the runtime value at `path`,
        // reusing `refined_check_as_bool` verbatim (the same helper `value is
        // RefinedType` lowers through). That helper builds text, so a refined
        // arm's tests stay text.
        Pattern::Refined {
            inner, predicate, ..
        } => {
            pattern_tests(path, path_ty, inner, cx, tests);
            // The checker (`check_pattern`) already rejected any scrutinee
            // that isn't literal-kind before this point, so `path_ty` is
            // always `Int` or `String` here for a well-typed program — a
            // silent fallback would mask a real bug (wrong predicate codegen
            // against the wrong base) rather than surfacing it loudly, the
            // same posture as `match_case`'s `Pattern::Refined` arm.
            let base = path_ty
                .and_then(|t| literal_base_of_ty(t, tys.types()))
                .unwrap_or_else(|| {
                    panic!(
                        "a refined pattern's scrutinee must be literal-kind (Int/String), got {:?}",
                        path_ty
                    )
                });
            for p in &predicate.predicates {
                cx.note_predicate(&p.kind);
            }
            let check: Built = Err(refined_check_as_bool(path.as_str(), base, Some(predicate)));
            tests.push(check.into());
        }
        // #2680: a length test (none for `[..]`, which matches every list),
        // then each refutable element against `path[i]`.
        Pattern::List { elems, rest, .. } => {
            let length = path.member("length");
            let length_test = |op: TsBinaryOp, n: usize, sym: &str| -> LoweredExpr {
                let built: Built = match length.node() {
                    Some(l) => Ok(TsExpr::Binary {
                        op,
                        left: Box::new(l.clone()),
                        right: Box::new(num_lit(n.to_string())),
                    }),
                    None => Err(format!("{p}.length {sym} {n}")),
                };
                built.into()
            };
            match (rest.is_some(), elems.len()) {
                (true, 0) => {}
                (true, n) => tests.push(length_test(TsBinaryOp::GreaterThanEq, n, ">=")),
                (false, n) => tests.push(length_test(TsBinaryOp::StrictEq, n, "===")),
            }
            let elem_ty = list_elem_ty(path_ty, cx);
            for (i, sub) in elems.iter().enumerate() {
                if sub.is_irrefutable() {
                    continue;
                }
                pattern_tests(&path.index(i), elem_ty, sub, cx, tests);
            }
        }
        // #2641: test each refutable field sub-pattern against `path.field`;
        // there is no tag test, since the checker proved the scrutinee is this
        // record.
        Pattern::Record { fields, .. } => {
            for (name, sub) in fields {
                let Some(sub) = sub else { continue };
                if sub.is_irrefutable() {
                    continue;
                }
                let field_ty = record_field_ty(path_ty, &name.name, cx);
                pattern_tests(&path.member(&name.name), field_ty, sub, cx, tests);
            }
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            tests.push(path_test(
                &path.member("tag"),
                TsBinaryOp::StrictEq,
                TsExpr::Lit(TsLit::Str(variant.name.clone())),
                format!("{p}.tag === \"{}\"", variant.name),
            ));
            for (i, b) in bindings.iter().enumerate() {
                let sp = b.pattern();
                if sp.is_irrefutable() {
                    continue;
                }
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys.types())
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys.types());
                pattern_tests(&path.member(&field), field_ty, sp, cx, tests);
            }
        }
        // #474: an or-pattern matches `path` when any alternative does — AND
        // each alternative's own tests, then OR the alternatives together, and
        // push the whole disjunction as a single combined test (so a caller
        // AND-joining `tests` composes it correctly with sibling tests, e.g. a
        // nested `Some(1 | 2)`'s outer `.tag === "Some"` test).
        Pattern::Or(alts, _) => {
            let terms: Vec<LoweredExpr> = alts
                .iter()
                .map(|alt| {
                    let mut t = Vec::new();
                    pattern_tests(path, path_ty, alt, cx, &mut t);
                    if t.is_empty() {
                        LoweredExpr::Node(TsExpr::Lit(TsLit::Bool(true)))
                    } else {
                        and_join(t)
                    }
                })
                .collect();
            let built = node_or_text(
                terms,
                |nodes| {
                    TsExpr::Paren(Box::new(
                        nodes
                            .into_iter()
                            .reduce(|left, right| TsExpr::Binary {
                                op: TsBinaryOp::Or,
                                left: Box::new(left),
                                right: Box::new(right),
                            })
                            .unwrap_or_else(|| unreachable!("an or-pattern has alternatives")),
                    ))
                },
                |texts| format!("({})", texts.join(" || ")),
            );
            tests.push(built.into());
        }
    }
}

/// #1187's slice 5 (the `Service` emitter cutover): the JS boolean guard for
/// a `from Events(E { field: value, .. })` subscription filter, AND-joining
/// one test per listed field — reads [`bynk_model::EventPatternIr`] (already
/// resolved by [`bynk_model::lower_protocol_ir`]) rather than the raw
/// AST `EventPattern`; [`EventPatternValueIr`]'s own doc comment already
/// named this function as its sole intended consumer, never wired up until
/// now (the AST-driven original this replaces produced byte-identical guard
/// text: `EventPatternValueIr::Const` only ever holds the same closed
/// `Int`/`Str`/`Bool` set `literal_case_label` covers, and `Variant { tag }`
/// already carries exactly the bare, unqualified tag the AST version's
/// `variant.name` destructured down to). No `Ty`/`LowerCtx` needed:
/// `check_event_pattern` (`bynk-check/src/project_model.rs`) has already
/// proven every field exists and every value type-checks, so this is total
/// and cannot panic. Returns `None` for a pattern-less subscription (nothing
/// to guard).
pub(crate) fn event_pattern_guard_ir(
    path: &str,
    pattern: Option<&EventPatternIr>,
) -> Option<String> {
    let pattern = pattern?;
    let tests: Vec<String> = pattern
        .fields
        .iter()
        .map(|(name, value)| match value {
            EventPatternValueIr::Const(ConstVal::Int(n)) => format!("{path}.{name} === {n}"),
            EventPatternValueIr::Const(ConstVal::Str(s)) => {
                format!("{path}.{name} === \"{}\"", escape_ts_string(s))
            }
            EventPatternValueIr::Const(ConstVal::Bool(b)) => format!("{path}.{name} === {b}"),
            EventPatternValueIr::Const(
                ConstVal::Float(_) | ConstVal::DurationMillis(_) | ConstVal::Unit,
            ) => {
                unreachable!(
                    "EventPatternValueIr::Const only ever holds Int/Str/Bool (its own doc \
                     comment) — the checker's own check_event_pattern already restricted a \
                     from Events(...) filter to that closed set before this pattern could exist"
                )
            }
            EventPatternValueIr::Variant { tag } => format!("{path}.{name}.tag === \"{tag}\""),
        })
        .collect();
    Some(tests.join(" && "))
}

#[cfg(test)]
mod event_pattern_guard_ir_tests {
    use super::*;

    /// Review of #1198: the byte-identity evidence against the deleted
    /// AST-driven `event_pattern_guard` was entirely fixture-level. This
    /// pins the guard text format directly, over a hand-built
    /// `EventPatternIr` covering all four cases `EventPatternValueIr` can
    /// hold (`Int`/`Str`/`Bool`/`Variant`).
    #[test]
    fn covers_every_value_shape() {
        let pattern = EventPatternIr {
            fields: vec![
                (
                    "n".to_string(),
                    EventPatternValueIr::Const(ConstVal::Int(3)),
                ),
                (
                    "s".to_string(),
                    EventPatternValueIr::Const(ConstVal::Str("a\"b".to_string())),
                ),
                (
                    "b".to_string(),
                    EventPatternValueIr::Const(ConstVal::Bool(true)),
                ),
                (
                    "status".to_string(),
                    EventPatternValueIr::Variant {
                        tag: "Active".to_string(),
                    },
                ),
            ],
        };
        assert_eq!(
            event_pattern_guard_ir("e", Some(&pattern)),
            Some(
                "e.n === 3 && e.s === \"a\\\"b\" && e.b === true && e.status.tag === \"Active\""
                    .to_string()
            )
        );
        assert_eq!(
            event_pattern_guard_ir("e", None),
            None,
            "a pattern-less subscription has nothing to guard"
        );
    }
}

/// Emit `const` declarations binding the names in `pattern` from runtime `path`,
/// recursing through nested payloads (ADR 0169). Each is a `Const` node when
/// the path is one (1A.13, Decision C of #3714), else its text line; an
/// or-pattern's `let` and `if`/`else` dispatch stays text lines.
pub(super) fn emit_pattern_bindings(
    body: &mut Body,
    path: &PatPath,
    path_ty: Option<TyId>,
    pattern: &Pattern,
    cx: &mut LowerCtx,
) {
    let tys = cx.ty_render();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Literal { .. } | Pattern::Range(_) => {}
        Pattern::Binding(id) => {
            let local = cx.bind_local_name(&id.name);
            body.push(const_binding(&local, path.lowered()));
        }
        // #472: a refined pattern binds only what its inner pattern binds
        // (`_ where P` binds nothing today).
        Pattern::Refined { inner, .. } => {
            emit_pattern_bindings(body, path, path_ty, inner, cx);
        }
        // #2680: each element binds from `path[i]`; a named rest from the
        // remaining elements.
        Pattern::List { elems, rest, .. } => {
            let elem_ty = list_elem_ty(path_ty, cx);
            for (i, sub) in elems.iter().enumerate() {
                emit_pattern_bindings(body, &path.index(i), elem_ty, sub, cx);
            }
            if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                let local = cx.bind_local_name(&binder.name);
                let value = list_rest_value(path, elems.len());
                body.push(const_binding(&local, value));
            }
        }
        // #2641: a shorthand field binds a local of its own name; `f: p`
        // recurses into `path.f`.
        Pattern::Record { fields, .. } => {
            for (name, sub) in fields {
                let field_path = path.member(&name.name);
                match sub {
                    Some(sub) => {
                        let field_ty = record_field_ty(path_ty, &name.name, cx);
                        emit_pattern_bindings(body, &field_path, field_ty, sub, cx);
                    }
                    None => {
                        let local = cx.bind_local_name(&name.name);
                        body.push(const_binding(&local, field_path.lowered()));
                    }
                }
            }
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            for (i, b) in bindings.iter().enumerate() {
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys.types())
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys.types());
                emit_pattern_bindings(body, &path.member(&field), field_ty, b.pattern(), cx);
            }
        }
        // #474: different alternatives can bind the same name at different
        // structural paths (`Held`'s field 2 vs `Confirmed`'s field 4), so a
        // single `const` can't express it. Declare each shared name once with
        // `let`, then dispatch per alternative — only the (small) binding
        // resolution is duplicated per alternative, not the arm body that
        // follows. Alternatives are mutually exclusive at runtime (a value has
        // exactly one tag, or a literal one value), so a plain if/else-if/else
        // chain is exhaustive over them once the caller's own combined test
        // (built by `pattern_tests`) has already passed.
        Pattern::Or(alts, _) => {
            let names = pattern.bound_names();
            if names.is_empty() {
                return;
            }
            let decl: Vec<String> = names
                .iter()
                .map(|id| cx.bind_local_name(&id.name))
                .collect();
            body.push(Item::Line(format!("let {};", decl.join(", "))));
            let last = alts.len() - 1;
            for (i, alt) in alts.iter().enumerate() {
                if i == last {
                    body.push(Item::Line("} else {".to_string()));
                } else {
                    let mut t = Vec::new();
                    pattern_tests(path, path_ty, alt, cx, &mut t);
                    let cond = if t.is_empty() {
                        "true".to_string()
                    } else {
                        and_join(t).into_text()
                    };
                    let kw = if i == 0 { "if" } else { "} else if" };
                    body.push(Item::Line(format!("{kw} ({cond}) {{")));
                }
                let mut assigns = Body::new();
                let mut pairs = Vec::new();
                pattern_binding_paths(path.as_str(), path_ty, alt, cx, &mut pairs);
                for (name, p) in &pairs {
                    let local = cx
                        .resolved_local_name(name)
                        .unwrap_or_else(|| ts_ident(name));
                    assigns.push(Item::Line(format!("{local} = {p};")));
                }
                body.push(Item::Indent(assigns));
            }
            body.push(Item::Line("}".to_string()));
        }
    }
}

/// `const <local> = <value>;` as an item (see `lower_stmt::const_item`).
fn const_binding(local: &str, value: LoweredExpr) -> Item {
    super::lower_stmt::const_item(true, local, None, value, false)
}

/// The value a list pattern's `..rest` binds, as a bridge value: the list
/// itself, or `path.slice(n)` (see [`list_rest_expr`], which is the text).
fn list_rest_value(path: &PatPath, n: usize) -> LoweredExpr {
    if n == 0 {
        return path.lowered();
    }
    let text = list_rest_expr(path.as_str(), n);
    let built: Built = match path.node() {
        Some(p) => Ok(bynk_ts::TsExpr::Call {
            callee: Box::new(bynk_ts::TsExpr::Member {
                object: Box::new(p.clone()),
                property: "slice".to_string(),
            }),
            args: vec![num_lit(n.to_string())],
        }),
        None => Err(text),
    };
    built.into()
}

/// Collect `(name, runtime-path)` pairs for the names `pattern` binds from
/// `path`, recursing through nested payloads — the same traversal as
/// `emit_pattern_bindings`, but returning the resolved paths instead of
/// writing `const` declarations, so an or-pattern's per-alternative dispatch
/// (above) can assign a shared `let` from whichever alternative matched.
fn pattern_binding_paths(
    path: &str,
    path_ty: Option<TyId>,
    pattern: &Pattern,
    cx: &LowerCtx,
    out: &mut Vec<(String, String)>,
) {
    let tys = cx.ty_render();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Literal { .. } | Pattern::Range(_) => {}
        Pattern::Binding(id) => out.push((id.name.clone(), path.to_string())),
        Pattern::Variant {
            variant, bindings, ..
        } => {
            for (i, b) in bindings.iter().enumerate() {
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys.types())
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys.types());
                pattern_binding_paths(&format!("{path}.{field}"), field_ty, b.pattern(), cx, out);
            }
        }
        // Never reached from the parser (an `Or`'s alternatives are always
        // leaves), but recurse into the first alternative defensively rather
        // than panic, mirroring `gather_pattern_bindings`'s checker-side choice.
        Pattern::Or(alts, _) => {
            if let Some(first) = alts.first() {
                pattern_binding_paths(path, path_ty, first, cx, out);
            }
        }
        // #472: binds only what its inner pattern binds (`_ where P` binds
        // nothing today) — same recursion as `emit_pattern_bindings`'s
        // `Refined` arm.
        Pattern::Refined { inner, .. } => pattern_binding_paths(path, path_ty, inner, cx, out),
        Pattern::List { elems, rest, .. } => {
            let elem_ty = list_elem_ty(path_ty, cx);
            for (i, sub) in elems.iter().enumerate() {
                pattern_binding_paths(&format!("{path}[{i}]"), elem_ty, sub, cx, out);
            }
            if let Some(binder) = rest.as_ref().and_then(|r| r.binder.as_ref()) {
                out.push((binder.name.clone(), list_rest_expr(path, elems.len())));
            }
        }
        Pattern::Record { fields, .. } => {
            for (name, sub) in fields {
                let field_path = format!("{path}.{}", name.name);
                match sub {
                    Some(sub) => {
                        let field_ty = record_field_ty(path_ty, &name.name, cx);
                        pattern_binding_paths(&field_path, field_ty, sub, cx, out);
                    }
                    None => out.push((name.name.clone(), field_path)),
                }
            }
        }
    }
}

fn lower_is(value: &Expr, pattern: &Pattern, cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // v0.13: refinement check — `value is RefinedType` lowers to the refined
    // type's predicates as a boolean expression. The receiver is forced to a
    // temp so the narrowing binding (`const n = <temp> as Quantity`) can shadow
    // the value's name without a TDZ.
    if let Pattern::Variant {
        variant, bindings, ..
    } = pattern
        && bindings.is_empty()
        && cx.is_refined_is_check(value, &variant.name)
        && let Some(TypeBody::Refined {
            base, refinement, ..
        }) = cx
            .commons()
            .types
            .get(&variant.name)
            .map(|d| d.body.clone())
    {
        let recv = pre.absorb(cx.is_receiver_ref_forced(value));
        for p in refinement.iter().flat_map(|r| &r.predicates) {
            cx.note_predicate(&p.kind);
        }
        return pre.finish_text(refined_check_as_bool(&recv, base, refinement.as_ref()));
    }
    let path = PatPath::from_lowered(pre.absorb_node(cx.is_receiver_ref(value)));
    let scrut_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
    let tested: LoweredExpr = match pattern {
        // A bare name binding after `is` matches anything over a sum (ADR 0169).
        Pattern::Wildcard(_) | Pattern::Binding(_) => LoweredExpr::Node(bool_node(true)),
        // v0.130 (DECISION F): the checker rejects a literal on the RHS of `is`,
        // so this is unreachable for a well-typed program; lower it to the
        // value-equality it would mean, defensively.
        Pattern::Literal { value, .. } => path_test(
            &path,
            bynk_ts::TsBinaryOp::StrictEq,
            literal_label_node(value),
            format!("{} === {}", path.as_str(), literal_case_label(value)),
        ),
        // #472: the checker rejects a refined pattern on the RHS of `is`
        // (D5), so this is unreachable for a well-typed program; lower it
        // defensively via the same `pattern_tests` a `match` arm uses (which
        // already handles `Pattern::Refined` — inner tests plus the predicate
        // via `refined_check_as_bool`).
        //
        // #705: a variant pattern lowers to its full structural test — the outer
        // tag *and* any nested refutable payload patterns, so
        // `r is Rejected(RefinementViolation(_))` checks both levels (the same
        // tests a `match` arm emits). A plain `is Ok(_)` / `is Rejected(_)` still
        // yields a single `.tag` test (the `_` payload is irrefutable). For a
        // loose scrutinee — the `Wire`-call outcome — the single-field `"value"`
        // fallback matches the runtime `{ tag, value }` shape, so the nested test
        // lands without a static type. A multi-payload nested pattern on a loose
        // scrutinee is unreachable today (the only loose variant sums are
        // single-field) and shared with `match`'s lowering; precise for any
        // typed scrutinee.
        //
        // #2641: a record pattern is its fields' tests, `true` when every field
        // is irrefutable. #474 §2.3.6: an or-pattern is legal after `is`;
        // `pattern_tests` ORs its alternatives into one combined term.
        Pattern::Refined { .. }
        | Pattern::Record { .. }
        | Pattern::List { .. }
        | Pattern::Range(_)
        | Pattern::Variant { .. }
        | Pattern::Or(..) => {
            let mut tests = Vec::new();
            pattern_tests(&path, scrut_ty, pattern, cx, &mut tests);
            // `pattern_tests` always pushes the outer `.tag` test for a variant
            // pattern, so a variant `is` is never vacuously `true` — a
            // silently-passing `expect` is exactly the failure this reuse removes.
            debug_assert!(
                !matches!(pattern, Pattern::Variant { .. }) || !tests.is_empty(),
                "a variant `is` pattern must emit at least its outer tag test"
            );
            if tests.is_empty() {
                LoweredExpr::Node(bool_node(true))
            } else {
                and_join(tests)
            }
        }
    };
    // #2836: a bare `r.tag === "Ok"` condition makes tsc narrow `r` for the code
    // it guards, so a later exhaustive `match r` (after `expect r is Ok(_)`, or in
    // the `else` of `if r is Err(_)`) has an impossible `case "Err":` (TS2678).
    // Passing the boolean through an identity arrow keeps the test and hands tsc
    // nothing to narrow by; the bindings read the payload through the proven
    // variant instead (`variant_payload`).
    let is_true = matches!(
        &tested,
        LoweredExpr::Node(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)))
    );
    if matches!(pattern, Pattern::Variant { .. } | Pattern::Or(..)) && !is_true {
        return pre.finish_built(node_or_text(
            vec![tested],
            |c| bynk_ts::TsExpr::Call {
                callee: Box::new(bynk_ts::TsExpr::Paren(Box::new(bynk_ts::TsExpr::Arrow {
                    params: vec![bynk_ts::TsParam {
                        name: "__c".to_string(),
                        ty: Some(bynk_ts::TsType::named("boolean")),
                        optional: false,
                    }],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(ident_node("__c")))),
                }))),
                args: vec![one(c)],
            },
            |c| format!("((__c: boolean) => __c)({})", one(c)),
        ));
    }
    pre.finish(tested)
}

/// v0.13: render a refined type's predicates as a single boolean expression over
/// `recv`, for `value is RefinedType`. Mirrors `emit_pred_check`'s per-predicate
/// logic but as `&&`-joined terms instead of `Result`-returning statements.
fn refined_check_as_bool(recv: &str, base: BaseType, refinement: Option<&Refinement>) -> String {
    let mut terms: Vec<String> = Vec::new();
    if base == BaseType::Int {
        // #1657: an `Int` is a JS safe integer.
        terms.push(format!("globalThis.Number.isSafeInteger({recv})"));
    }
    // v0.21: validated `Float` values are finite (ADR 0040).
    if base == BaseType::Float {
        terms.push(format!("globalThis.Number.isFinite({recv})"));
    }
    if let Some(r) = refinement {
        for p in bynk_syntax::ast::in_check_order(&r.predicates, |p| &p.kind) {
            terms.push(match &p.kind {
                PredKind::NonNegative if base == BaseType::Decimal => {
                    format!("({recv}).coefficient >= 0n")
                }
                PredKind::Positive if base == BaseType::Decimal => {
                    format!("({recv}).coefficient > 0n")
                }
                PredKind::InRangeD(a, b) => crate::emitter::decimal_range_condition(recv, a, b),
                PredKind::NonNegative => format!("{recv} >= 0"),
                PredKind::Positive => format!("{recv} > 0"),
                PredKind::InRange(a, b) => {
                    format!("({recv} >= {} && {recv} <= {})", a.value, b.value)
                }
                PredKind::InRangeF(a, b) => {
                    format!("({recv} >= {} && {recv} <= {})", a.lexeme, b.lexeme)
                }
                PredKind::NonEmpty => format!("{recv}.length > 0"),
                PredKind::IsoDate => format!("__bynkIsIsoDate({recv})"),
                PredKind::MinLength(n) => format!("{recv}.length >= {n}"),
                PredKind::MaxLength(n) => format!("{recv}.length <= {n}"),
                PredKind::Length(n) => format!("{recv}.length === {n}"),
                PredKind::Matches(pat) => {
                    let escaped = escape_ts_string(pat);
                    format!("new globalThis.RegExp(\"^(?:\" + \"{escaped}\" + \")$\").test({recv})")
                }
            });
        }
    }
    if terms.is_empty() {
        "true".to_string()
    } else {
        format!("({})", terms.join(" && "))
    }
}

#[cfg(test)]
mod idempotency_scoping_tests {
    use super::*;
    use crate::emitter::ctx::HandlerShared;
    use bynk_check::checker::TypedCommons;

    // P6.38 (design/tracks/the-ir.md §6a): reads `TypedCommons::empty()`
    // (bynk-check) instead of hand-building an empty `Commons` here — this
    // module's own last `bynk_syntax::ast` dependency, now gone.
    fn empty_commons() -> TypedCommons {
        TypedCommons::empty()
    }

    /// #934 (Decision C): a first-party `Idempotency.dedup`/`remember` call
    /// reaching `scope_idempotency_key` with no `handler_scope` set means some
    /// emission site populated `capabilities` without also wiring the scope —
    /// a compiler bug. This must fail loudly, not silently ship an unscoped
    /// key; nothing in the fixture suite exercises a missing scope (every real
    /// site sets it), so this is the only proof the guard actually fires.
    #[test]
    #[should_panic(expected = "handler_scope")]
    fn missing_handler_scope_panics() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::Method,
        );
        let mut args = vec!["orderId".to_string()];
        scope_idempotency_key(true, "dedup", &mut args, &cx);
    }

    /// The mirror case: a real scope present, the call proceeds and the key
    /// is prefixed — confirms the panic in the test above is really guarding
    /// the `None` case and not firing unconditionally.
    #[test]
    fn present_handler_scope_prefixes_the_key() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    handler_scope: Some("shop.reserve.ordering.call".to_string()),
                    ..HandlerShared::default()
                },
                deps_identity_binder: None,
                actor_sum_binder: None,
            },
        );
        let mut args = vec!["orderId".to_string()];
        scope_idempotency_key(true, "dedup", &mut args, &cx);
        assert_eq!(args[0], "`shop.reserve.ordering.call::${orderId}`");
    }

    /// `claim` takes its key first, like `dedup` and `remember`, so it is
    /// scoped to the calling handler the same way.
    #[test]
    fn claim_is_scoped_like_dedup() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::ServiceHandler {
                handler: HandlerShared {
                    handler_scope: Some("shop.hooks.api.POST".to_string()),
                    ..HandlerShared::default()
                },
                deps_identity_binder: None,
                actor_sum_binder: None,
            },
        );
        let mut args = vec!["id".to_string(), "3600000".to_string()];
        scope_idempotency_key(true, "claim", &mut args, &cx);
        assert_eq!(args[0], "`shop.hooks.api.POST::${id}`");
        assert_eq!(args[1], "3600000");
    }

    /// A provider with no `__track` gets a scope that takes a claim for its
    /// whole window, so nothing is cut to the lease with no settle to extend it.
    #[test]
    fn a_provider_without_a_scope_takes_the_full_window() {
        let before = CLAIM_SCOPE_DECLARATION;
        assert!(before.contains("lease: (t: number) => t"), "{before}");
        assert!(before.contains("__track?.() ??"), "{before}");
    }

    /// A capability merely named `"Idempotency"` from some other adapter or
    /// context (not first-party `bynk`) must pass through untouched even with
    /// no scope set — `is_first_party: false` short-circuits before the
    /// `handler_scope` check, so this must NOT panic.
    #[test]
    fn non_first_party_capability_is_not_scoped() {
        let commons = empty_commons();
        let cross_context = bynk_check::resolver::CrossContextInfo::default();
        let runtime_use = RuntimeUse::default();
        let cx = LowerCtx::new(
            ModuleCtx::new(
                &commons,
                &cross_context,
                &runtime_use,
                UnitFacts::detached(),
            ),
            BodyMode::Method,
        );
        let mut args = vec!["orderId".to_string()];
        scope_idempotency_key(false, "dedup", &mut args, &cx);
        assert_eq!(args[0], "orderId");
    }
}

/// T2.2 (#1018, R6.4): `maybe_async_iife`'s `if !iife.contains("await ")`
/// replaced by `LowerCtx::emitted_await`, a flag set at the two statement
/// sites that emit a literal `await` (`EffectLet`, `Do`) and read-and-reset
/// around a value-position `match`/`if` IIFE's own body construction — see
/// `finish_async_iife`. Same precedent as `conditional_runtime_import_tests`
/// (`body.contains("__bynkBytes")` → `RuntimeUse`): the scan and the flag
/// agree on every case except where the scan's substring match wasn't really
/// about *this* scope's own effectfulness.
#[cfg(test)]
mod async_iife_effectfulness_tests {
    use crate::testkit::emit_source;

    /// The over-match this slice closes: a `Query`/broadcast iterator terminal
    /// (`forEach`) lowers to a self-contained, always-`async () => {...}` IIFE
    /// — a plain `Effect`-typed *value*, not something this arm itself awaits.
    /// The old scan saw the literal `"await "` inside that embedded string and
    /// wrongly wrapped the enclosing `match` arrow as `async` too, producing a
    /// spurious `await await (async (__d) => …)()`. Neither arm needs the
    /// switch's own arrow to be async, so it must stay a plain `(__d) => {…}`,
    /// invoked with a single `await` from the enclosing `let r <- …`.
    #[test]
    fn match_arm_iterator_terminal_does_not_force_the_switch_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(names: List[Int], flag: Bool) -> Effect[()] {\n  \
             let r <- match flag {\n    \
             true => names.forEach(noop)\n    \
             false => Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await ((__d) => {"),
            "the switch arrow must stay synchronous, awaited once by the `let <-`: {ts}"
        );
        assert!(!ts.contains("await await"), "{ts}");
    }

    /// The same over-match, reached through the `if`-IIFE path (an `is`-binding
    /// forces the IIFE form rather than a ternary) instead of `match`.
    #[test]
    fn if_arm_iterator_terminal_does_not_force_the_iife_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(o: Option[Int], names: List[Int]) -> Effect[()] {\n  \
             let r <- if o is Some(n) {\n    \
             names.forEach(noop)\n  \
             } else {\n    \
             Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await (() => {"),
            "the if-IIFE arrow must stay synchronous, awaited once by the `let <-`: {ts}"
        );
        assert!(!ts.contains("await await"), "{ts}");
    }

    /// Non-regression: the defect `#2 review` fixed in `8068c0db` (a genuinely
    /// effectful `is`-binding `if` in value position) must still emit a real
    /// `async` IIFE — this is not an over-match, so it keeps needing the wrap
    /// that closed the original hard `SyntaxError`.
    #[test]
    fn if_arm_genuine_effect_still_forces_the_iife_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn fetch(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\n\
             fn run(o: Option[Int], c: Bool) -> Effect[Int] {\n  \
             let r <- if o is Some(n) && c {\n    \
             let y <- fetch(n)\n    \
             Effect.pure(y + 1)\n  \
             } else {\n    \
             Effect.pure(0)\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("async () => {") && ts.contains("await fetch(n)"),
            "a real await inside the IIFE must still force the async wrap: {ts}"
        );
    }

    /// Non-regression: a genuinely effectful *nested* `match` (embedded
    /// directly as an outer arm's value, not via an intermediate `let <-`)
    /// must still propagate its own async-wrap need to the enclosing switch
    /// arrow — the flag's `saved || needs_async` restore is what replaces the
    /// old scan's implicit propagation (the substring surviving in the
    /// returned string).
    #[test]
    fn nested_match_effectfulness_propagates_to_the_outer_switch_arrow() {
        let ts = emit_source(
            "commons t\n\n\
             fn fetch(n: Int) -> Effect[Int] {\n  Effect.pure(n)\n}\n\n\
             fn run(a: Bool, b: Bool) -> Effect[Int] {\n  \
             let r <- match a {\n    \
             true => match b {\n      \
             true => {\n        let x <- fetch(1)\n        Effect.pure(x + 1)\n      }\n      \
             false => Effect.pure(2)\n    \
             }\n    \
             false => Effect.pure(3)\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await await (async (__d) => {"),
            "the outer switch arrow must also become async: {ts}"
        );
    }

    /// Non-regression, mirroring the `EffectLet` case above for the *other*
    /// write site: an arm whose only effect is a `do` (no binder) must still
    /// force its switch arrow async. The old scan was blind to which
    /// statement produced the `await`, so `Do` came along for free; the flag
    /// has to name it explicitly at its own site (`emit_statement`'s
    /// `Statement::Do` arm) and nothing else pins that it does.
    #[test]
    fn match_arm_do_only_effect_still_forces_the_switch_arrow_async() {
        let ts = emit_source(
            "commons t\n\n\
             fn noop(n: Int) -> Effect[()] {\n  Effect.pure(())\n}\n\n\
             fn run(flag: Bool) -> Effect[()] {\n  \
             let r <- match flag {\n    \
             true => {\n      do noop(1)\n      Effect.pure(())\n    }\n    \
             false => Effect.pure(())\n  \
             }\n  r\n}\n",
        );
        assert!(
            ts.contains("const r = await await (async (__d) => {"),
            "a do-only effectful arm must also force the switch arrow async: {ts}"
        );
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    /// Each piece's binding is the identifier after a leading `const `, as
    /// the AST would give it.
    fn rewrite(pieces: &[&str], kinds: &[SettleKind], tail: &str) -> String {
        let mut out = String::new();
        let mut bounds = Vec::new();
        let mut binds = Vec::new();
        for p in pieces {
            let start = out.len();
            out.push_str(p);
            bounds.push((start, out.len()));
            binds.push(
                p.strip_prefix("const ")
                    .and_then(|r| r.split(' ').next())
                    .map(|n| vec![n.to_string()])
                    .unwrap_or_default(),
            );
        }
        out.push_str(tail);
        settle_segments(&out, &bounds, kinds, &binds)
    }

    #[test]
    fn the_reads_since_the_last_unsafe_address_are_retried_with_their_expects() {
        use SettleKind::*;
        let text = rewrite(
            &[
                "const a = await post();\n",
                "const n = await get();\n",
                "if (!(n === 1)) { throw fail(); }\n",
                "const after = n;\n",
            ],
            &[Unsafe, Safe, Expect, Other],
            "void (after);\n",
        );
        assert_eq!(
            text,
            "const a = await post();\nconst { n } = await __bynkSettle(async () => {\nconst n = await get();\nif (!(n === 1)) { throw fail(); }\nreturn { n };\n});\nconst after = n;\nvoid (after);\n"
        );
    }

    /// Two segments in one case, each ended by an unsafe address; the
    /// statements after a run's last `expect` stay outside its attempt.
    #[test]
    fn each_run_between_unsafe_addresses_is_its_own_segment() {
        use SettleKind::*;
        let text = rewrite(
            &[
                "const a = await post();\n",
                "const n = await get();\n",
                "if (!(n)) { throw fail(); }\n",
                "const t = n;\n",
                "const b = await post();\n",
                "const m = await get();\n",
                "if (!(m)) { throw fail(); }\n",
                "const u = await get();\n",
            ],
            &[Unsafe, Safe, Expect, Other, Unsafe, Safe, Expect, Safe],
            "void (u);\n",
        );
        assert_eq!(
            text,
            "const a = await post();\n\
             const { n } = await __bynkSettle(async () => {\n\
             const n = await get();\n\
             if (!(n)) { throw fail(); }\n\
             return { n };\n\
             });\n\
             const t = n;\n\
             const b = await post();\n\
             const { m } = await __bynkSettle(async () => {\n\
             const m = await get();\n\
             if (!(m)) { throw fail(); }\n\
             return { m };\n\
             });\n\
             const u = await get();\n\
             void (u);\n"
        );
    }

    /// A pure `let` before the first safe address is part of the attempt
    /// (re-running it is harmless) and its binding is returned too.
    #[test]
    fn an_other_before_the_first_read_joins_the_attempt() {
        use SettleKind::*;
        let text = rewrite(
            &[
                "const q = 1;\n",
                "const n = await get();\n",
                "if (!(n === q)) { throw fail(); }\n",
            ],
            &[Other, Safe, Expect],
            "",
        );
        assert_eq!(
            text,
            "const { q, n } = await __bynkSettle(async () => {\n\
             const q = 1;\n\
             const n = await get();\n\
             if (!(n === q)) { throw fail(); }\n\
             return { q, n };\n\
             });\n"
        );
    }

    /// The segment's text moves in unchanged: a multi-line template literal
    /// keeps its contents.
    #[test]
    fn a_template_literal_in_a_segment_is_not_reindented() {
        use SettleKind::*;
        let text = rewrite(
            &[
                "const n = await get(`a\nb`);\n",
                "if (!(n)) { throw fail(); }\n",
            ],
            &[Safe, Expect],
            "",
        );
        assert!(text.contains("get(`a\nb`)"), "{text}");
    }

    #[test]
    fn an_expect_with_no_read_before_it_is_not_retried() {
        use SettleKind::*;
        let pieces = ["const a = await post();\n", "if (!(a)) { throw fail(); }\n"];
        let text = rewrite(&pieces, &[Unsafe, Expect], "");
        assert_eq!(text, pieces.concat());
    }
}

/// #2622: lower a `Decimal` kernel method to its runtime helper. `round` and
/// `div` carry a rounding mode, read from the `RoundingMode.<mode>` argument
/// and lowered to its name as a string literal.
fn lower_decimal_kernel(
    receiver: &Expr,
    op: DecimalOp,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let mode = |a: &Expr| bynk_syntax::ast::rounding_mode(a).map(|m| format!("\"{}\"", m.name()));
    let text: Option<String> = match (op, args) {
        (DecimalOp::Scale, []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).scale"))
        }
        (DecimalOp::ToString, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_decimal("__bynkDecimalToString");
            Some(format!("__bynkDecimalToString({recv})"))
        }
        (DecimalOp::Abs, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_decimal("__bynkDecimalAbs");
            Some(format!("__bynkDecimalAbs({recv})"))
        }
        (DecimalOp::ToInt, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_decimal("__bynkDecimalToInt");
            Some(format!("__bynkDecimalToInt({recv})"))
        }
        (DecimalOp::ToFloat, []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_decimal("__bynkDecimalToFloat");
            Some(format!("__bynkDecimalToFloat({recv})"))
        }
        (DecimalOp::Min | DecimalOp::Max, [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            let (helper, name) = if op == DecimalOp::Min {
                ("__bynkDecimalMin", "__bynkDecimalMin")
            } else {
                ("__bynkDecimalMax", "__bynkDecimalMax")
            };
            cx.note_decimal(name);
            Some(format!("{helper}({recv}, {other})"))
        }
        (DecimalOp::Round, [scale, m]) => {
            let mode = mode(m)?;
            let recv = pre.lower(receiver, cx);
            let scale = pre.lower(scale, cx);
            cx.note_decimal("__bynkDecimalRound");
            Some(format!("__bynkDecimalRound({recv}, {scale}, {mode})"))
        }
        (DecimalOp::Div, [other, scale, m]) => {
            let mode = mode(m)?;
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            let scale = pre.lower(scale, cx);
            cx.note_decimal("__bynkDecimalDiv");
            Some(format!(
                "__bynkDecimalDiv({recv}, {other}, {scale}, {mode})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish_text(expr))
}

#[cfg(test)]
mod bridge_tests {
    use super::*;

    fn lit_text(l: Lowered) -> String {
        assert!(l.pre.is_empty(), "a literal hoists nothing");
        assert!(
            matches!(l.expr, LoweredExpr::Node(_)),
            "a literal is a node"
        );
        l.expr.into_text()
    }

    #[test]
    fn numeric_literal_nodes_print_their_digits_verbatim() {
        for digits in [
            "0",
            "42",
            "170141183460469231731687303715884105727",
            "1e10",
            "1.50",
        ] {
            assert_eq!(lit_text(Lowered::node(num_lit(digits.to_string()))), digits);
        }
    }

    #[test]
    fn decimal_literal_node_matches_the_text_form() {
        for (coef, scale) in [
            ("5", 2),
            ("-5", 2),
            ("007", 0),
            ("-0", 3),
            ("12345678901234567890", 18),
        ] {
            assert_eq!(
                lit_text(Lowered::node(decimal_lit(coef, scale))),
                crate::emitter::ts_types::decimal_ts_literal(coef, scale),
                "{coef}/{scale}"
            );
        }
    }

    #[test]
    fn string_literal_node_matches_the_hand_escaped_text() {
        for s in [
            "",
            "plain",
            "say \"hi\"",
            "a\\b",
            "line\nbreak\ttab\r",
            "nul\0",
            "ls\u{2028}ps\u{2029}",
            "`tick` ${x}",
            "unicode ·",
        ] {
            let node = bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Str(s.to_string()));
            assert_eq!(
                lit_text(Lowered::node(node)),
                format!("\"{}\"", escape_ts_string(s)),
                "{s:?}"
            );
        }
    }

    #[test]
    fn keyword_literal_nodes_print_as_before() {
        let t = bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true));
        let f = bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(false));
        assert_eq!(lit_text(Lowered::node(t)), "true");
        assert_eq!(lit_text(Lowered::node(f)), "false");
    }

    #[test]
    fn pre_hands_converted_and_unconverted_children_the_same_way() {
        let mut pre = Pre::new();
        assert_eq!(pre.absorb(Lowered::bare("x + 1")), "x + 1");
        assert!(matches!(
            pre.absorb_node(Lowered::bare("x")),
            LoweredExpr::Text(_)
        ));
        assert!(matches!(
            pre.absorb_node(Lowered::node(num_lit("1".to_string()))),
            LoweredExpr::Node(_)
        ));
    }

    #[test]
    fn hoisted_statements_keep_their_order_and_text_becomes_raw() {
        let mut pre = Pre::new();
        pre.push_text("const a = 1;".to_string());
        pre.push_text("const b = 2;".to_string());
        assert_eq!(pre.stmts(), ["const a = 1;", "const b = 2;"]);
        let out = pre.ts_stmts("  ");
        assert_eq!(out.len(), 2);
        let printed: String = out.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect();
        assert_eq!(printed, "  const a = 1;\n  const b = 2;\n");
    }

    const LITERALS: &str = r#"
commons lits

fn int() -> Int { 42 }
fn big() -> Int { 9007199254740991 }
fn float() -> Float { 1.5e3 }
fn dec() -> Decimal { 1.20d }
fn dur() -> Duration { 5.minutes }
fn text() -> String { "a\"b\nc" }
fn yes() -> Bool { true }
fn no() -> Bool { false }
fn unit() -> () { () }
fn none() -> Option[Int] { None }
fn neg() -> Int { -5 }
fn negd() -> Decimal { -1.20d }
fn minus() -> Float { 2.5 - 1.5e3 }
fn minus_neg() -> Int { 5 - -3 }
"#;

    /// The body of the emitted function `name`: the text of its `return`.
    fn returned(ts: &str, name: &str) -> String {
        let head = format!("export function {name}(");
        let from = ts
            .find(&head)
            .unwrap_or_else(|| panic!("no {name} in:\n{ts}"));
        let rest = &ts[from..];
        let ret = rest.find("return ").expect("a return") + "return ".len();
        let end = rest[ret..].find(";\n").expect("a terminator");
        rest[ret..ret + end].to_string()
    }

    #[test]
    fn each_literal_kind_lowers_to_its_emitted_text() {
        let ts = crate::testkit::emit_source(LITERALS);
        for (name, want) in [
            ("int", "42"),
            ("big", "9007199254740991"),
            ("float", "1.5e3"),
            ("dec", "{ coefficient: 120n, scale: 2 }"),
            ("dur", "300000"),
            ("text", "\"a\\\"b\\nc\""),
            ("yes", "true"),
            ("no", "false"),
            ("unit", "undefined"),
            ("none", "None"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn a_negative_literal_in_operand_position_lowers_as_before() {
        // `UnaryOp::Neg` splices its operand's printed text after `-`, and a
        // binary operator splices a node child between its own spaces.
        let ts = crate::testkit::emit_source(LITERALS);
        for (name, want) in [
            ("neg", "-5"),
            ("negd", "__bynkDecimalNeg({ coefficient: 120n, scale: 2 })"),
            ("minus", "2.5 - 1.5e3"),
            ("minus_neg", "5 - -3"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    fn id(n: &str) -> bynk_ts::TsExpr {
        bynk_ts::TsExpr::Ident(n.to_string())
    }

    fn bin(op: bynk_ts::TsBinaryOp, l: &str, r: &str) -> bynk_ts::TsExpr {
        bynk_ts::TsExpr::Binary {
            op,
            left: Box::new(id(l)),
            right: Box::new(id(r)),
        }
    }

    fn node(e: bynk_ts::TsExpr) -> LoweredExpr {
        LoweredExpr::Node(e)
    }

    fn text(t: &str) -> LoweredExpr {
        LoweredExpr::Text(t.to_string())
    }

    /// A [`Built`] as the bridge value (test-only; production text leaves
    /// through `lower_expr`'s shared exit).
    fn bridge(b: Built) -> LoweredExpr {
        match b {
            Ok(n) => LoweredExpr::Node(n),
            Err(t) => LoweredExpr::Text(t),
        }
    }

    // The production helpers return a `Built`; these shadow them with the
    // bridge value so the tests read the same either way.
    fn t_unary_op(op: UnaryOp, c: LoweredExpr) -> LoweredExpr {
        bridge(super::unary_op(op, c))
    }

    fn t_paren(c: LoweredExpr) -> LoweredExpr {
        bridge(super::paren(c))
    }

    fn t_runtime_ctor_call(
        object: Option<&'static str>,
        name: &'static str,
        arg: LoweredExpr,
    ) -> LoweredExpr {
        bridge(super::runtime_ctor_call(object, name, arg))
    }

    fn t_node_or_text(
        children: Vec<LoweredExpr>,
        node: impl FnOnce(Vec<bynk_ts::TsExpr>) -> bynk_ts::TsExpr,
        text: impl FnOnce(Vec<String>) -> String,
    ) -> LoweredExpr {
        bridge(super::node_or_text(children, node, text))
    }

    fn is_node(e: &LoweredExpr) -> bool {
        matches!(e, LoweredExpr::Node(_))
    }

    #[test]
    fn unary_operators_take_their_brackets_from_the_printer() {
        use bynk_ts::TsBinaryOp::{Add, And};
        // `-(a - b)`: the source's explicit `Paren` is kept as a node.
        let sub = t_paren(node(bin(Add, "a", "b")));
        let neg = t_unary_op(UnaryOp::Neg, sub);
        assert!(is_node(&neg));
        assert_eq!(neg.into_text(), "-(a + b)");
        // `!(x && y)`.
        let not = t_unary_op(UnaryOp::Not, t_paren(node(bin(And, "x", "y"))));
        assert_eq!(not.into_text(), "!(x && y)");
        // A bare binary operand (no source `Paren`) is bracketed by the printer.
        let bare = t_unary_op(UnaryOp::Not, node(bin(And, "x", "y")));
        assert_eq!(bare.into_text(), "!(x && y)");
        let bare = t_unary_op(UnaryOp::Neg, node(bin(Add, "a", "b")));
        assert_eq!(bare.into_text(), "-(a + b)");
        // Nested unaries: `!!x`, `-!x`, and `-(-x)`, never the decrement `--x`.
        let not_not = t_unary_op(UnaryOp::Not, t_unary_op(UnaryOp::Not, node(id("x"))));
        assert_eq!(not_not.into_text(), "!!x");
        let neg_not = t_unary_op(UnaryOp::Neg, t_unary_op(UnaryOp::Not, node(id("x"))));
        assert_eq!(neg_not.into_text(), "-!x");
        let neg_neg = t_unary_op(UnaryOp::Neg, t_unary_op(UnaryOp::Neg, node(id("x"))));
        assert_eq!(neg_neg.into_text(), "-(-x)");
    }

    #[test]
    fn a_negated_decimal_is_a_call_over_a_genuine_identifier() {
        let arg = node(decimal_lit("120", 2));
        let neg = t_runtime_ctor_call(None, "__bynkDecimalNeg", arg);
        match &neg {
            LoweredExpr::Node(bynk_ts::TsExpr::Call { callee, .. }) => {
                assert!(matches!(&**callee, bynk_ts::TsExpr::Ident(n) if n == "__bynkDecimalNeg"));
            }
            _ => panic!("a node child gives a call node"),
        }
        assert_eq!(
            neg.into_text(),
            "__bynkDecimalNeg({ coefficient: 120n, scale: 2 })"
        );
    }

    #[test]
    fn constructor_calls_and_lists_are_nodes_over_node_children() {
        let ok = t_runtime_ctor_call(None, "Ok", node(num_lit("1".into())));
        assert!(is_node(&ok));
        assert_eq!(ok.into_text(), "Ok(1)");
        let http = t_runtime_ctor_call(Some("HttpResult"), "Ok", node(id("v")));
        match &http {
            LoweredExpr::Node(bynk_ts::TsExpr::Call { callee, .. }) => {
                assert!(matches!(**callee, bynk_ts::TsExpr::Member { .. }))
            }
            _ => panic!("expected a call node"),
        }
        assert_eq!(http.into_text(), "HttpResult.Ok(v)");
        assert_eq!(
            t_runtime_ctor_call(None, "Err", node(id("e"))).into_text(),
            "Err(e)"
        );
        assert_eq!(
            t_runtime_ctor_call(None, "Some", node(id("s"))).into_text(),
            "Some(s)"
        );
        let list = t_node_or_text(
            vec![node(id("a")), node(num_lit("2".into()))],
            |items| bynk_ts::TsExpr::Array {
                items,
                multiline: false,
            },
            |items| format!("[{}]", items.join(", ")),
        );
        assert!(is_node(&list));
        assert_eq!(list.into_text(), "[a, 2]");
    }

    #[test]
    fn a_text_child_makes_every_arm_fall_back_to_todays_text() {
        let t = || text("f(x) + 1");
        for (got, want) in [
            (t_unary_op(UnaryOp::Neg, t()), "-f(x) + 1"),
            (t_unary_op(UnaryOp::Not, t()), "!f(x) + 1"),
            (t_paren(t()), "(f(x) + 1)"),
            (t_runtime_ctor_call(None, "Ok", t()), "Ok(f(x) + 1)"),
            (t_runtime_ctor_call(None, "Err", t()), "Err(f(x) + 1)"),
            (t_runtime_ctor_call(None, "Some", t()), "Some(f(x) + 1)"),
            (
                t_runtime_ctor_call(Some("HttpResult"), "Ok", t()),
                "HttpResult.Ok(f(x) + 1)",
            ),
            (
                t_runtime_ctor_call(None, "__bynkDecimalNeg", t()),
                "__bynkDecimalNeg(f(x) + 1)",
            ),
        ] {
            assert!(!is_node(&got));
            assert_eq!(got.into_text(), want);
        }
        // A list with one text element among nodes renders every element as text.
        let list = t_node_or_text(
            vec![node(id("a")), t()],
            |items| bynk_ts::TsExpr::Array {
                items,
                multiline: false,
            },
            |items| format!("[{}]", items.join(", ")),
        );
        assert!(!is_node(&list));
        assert_eq!(list.into_text(), "[a, f(x) + 1]");
    }

    #[test]
    fn wrappers_over_source_lower_as_before() {
        let src = "commons wraps\n\
                   fn f(a: Int, b: Int) -> Int { -(a - b) }\n\
                   fn g(ok: Bool) -> Bool { !(ok && ok) }\n\
                   fn h() -> Result[Int, String] { Ok(1) }\n\
                   fn i() -> List[Int] { [1, 2, 3] }\n\
                   fn j() -> Option[Int] { Some(2) }\n\
                   fn k() -> Decimal { -1.5d }\n\
                   fn m() -> Int { - -1 }\n\
                   fn n(x: Int) -> Int { - -x }\n\
                   fn o(x: Int) -> Int { - - -x }\n";
        let ts = crate::testkit::emit_source(src);
        for (name, want) in [
            ("f", "-(a - b)"),
            ("g", "!(ok && ok)"),
            ("h", "Ok(1)"),
            ("i", "[1, 2, 3]"),
            ("j", "Some(2)"),
            ("k", "__bynkDecimalNeg({ coefficient: 15n, scale: 1 })"),
            // A negated negation is `-(-x)`, never the decrement `--x`, whether
            // the operand is a node (a literal) or still text (an identifier).
            ("m", "-(-1)"),
            ("n", "-(-x)"),
            ("o", "-(-(-x))"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn a_negated_negation_never_lowers_to_a_decrement() {
        // `- -x` must not splice to `--x` (a TS pre-decrement) nor `- -5` to
        // `--5` (a TS syntax error): the operand is bracketed, `-(-x)`.
        let ts = crate::testkit::emit_source(
            r#"
commons negneg

fn twice(x: Int) -> Int { - -x }
fn lit() -> Int { - -5 }
fn triple(x: Int) -> Int { - - -x }
fn paren(x: Int) -> Int { -(-x) }
fn minus(x: Int) -> Int { 5 - - -x }
"#,
        );
        for (name, want) in [
            ("twice", "-(-x)"),
            ("lit", "-(-5)"),
            ("triple", "-(-(-x))"),
            ("paren", "-(-x)"),
            ("minus", "5 - -(-x)"),
        ] {
            let got = returned(&ts, name);
            assert_eq!(got, want, "{name}");
            assert!(!got.contains("--"), "{name}: `--` in {got}");
        }
    }

    /// The number of expressions that left lowering as text while emitting `src`.
    fn text_exits(src: &str) -> usize {
        TEXT_EXITS.with(|c| c.set(0));
        let _ = crate::testkit::emit_source(src);
        TEXT_EXITS.with(|c| c.get())
    }

    #[test]
    fn identifiers_and_wrappers_over_them_lower_to_nodes() {
        // 1A.4 (#3701): an identifier is a node, so `Some(x)`, `-x` and
        // `[x, y]` over identifiers are nodes too (before, each was text
        // because its child was). No expression below leaves as text.
        let src = "commons idnodes\n\
                   fn a(x: Int) -> Option[Int] { Some(x) }\n\
                   fn b(x: Int) -> Int { -x }\n\
                   fn c(x: Int, y: Int) -> List[Int] { [x, y] }\n\
                   fn d(x: Int) -> Int { - -x }\n\
                   fn e(ok: Bool) -> Bool { !ok }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        for (name, want) in [
            ("a", "Some(x)"),
            ("b", "-x"),
            ("c", "[x, y]"),
            // Now on the node path, a negated negation still brackets.
            ("d", "-(-x)"),
            ("e", "!ok"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn field_access_and_constant_reads_over_identifiers_lower_to_nodes() {
        let head = "commons fieldnodes\n\
                    type P = { name: String, age: Int }\n\
                    const K: Int = 4\n";
        let fields = "fn a(p: P) -> String { p.name }\n\
                      fn c(p: P) -> Int { p.age }\n";
        // The constant's own initialiser still leaves as text (not this
        // slice), so a constant read is measured against that baseline: adding
        // `fn b() -> Int { K }` adds no text exit.
        let base = text_exits(head);
        assert_eq!(text_exits(&format!("{head}{fields}")), base);
        let src = format!("{head}{fields}fn b() -> Int {{ K }}\n");
        assert_eq!(text_exits(&src), base);
        let ts = crate::testkit::emit_source(&src);
        for (name, want) in [("a", "p.name"), ("b", "K()"), ("c", "p.age")] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn a_nullary_variant_is_a_node_over_genuine_names() {
        let src = "commons variants\n\
                   type Colour = | Red | Green\n\
                   fn a() -> Colour { Red }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "a"), "Colour.Red");
    }

    #[test]
    fn the_retained_text_branches_produce_their_old_text() {
        // A generic function used as a value keeps its text closure.
        let src = "commons closures\n\
                   fn pick[T: Ord](a: T, b: T) -> T { if a < b { a } else { b } }\n\
                   fn f() -> (Int, Int) -> Int { pick }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(
            returned(&ts, "f").starts_with("((__a0: number, __a1: number) => pick("),
            "{}",
            returned(&ts, "f")
        );
        // ... and it leaves lowering as text: one more text exit than the
        // same module without the reference.
        let without = "commons closures\n\
                       fn pick[T: Ord](a: T, b: T) -> T { if a < b { a } else { b } }\n";
        assert!(text_exits(src) > text_exits(without));
    }

    #[test]
    fn an_http_result_variant_read_is_a_node() {
        // 1A.4's `HttpResult.X` read leaves no text exit of its own.
        let src = "commons hr\n\
                   fn f() -> HttpResult[Int] { HttpResult.NotFound }\n";
        assert_eq!(text_exits(src), 0);
        assert_eq!(
            returned(&crate::testkit::emit_source(src), "f"),
            "HttpResult.NotFound"
        );
    }

    // ---- 1A.5 (#3703): value binary operators -------------------------------

    fn t_plain_binary(op: BinOp, l: LoweredExpr, r: LoweredExpr) -> LoweredExpr {
        bridge(super::plain_binary(op, vec![l, r]))
    }

    fn t_helper_call(helper: &'static str, l: LoweredExpr, r: LoweredExpr) -> LoweredExpr {
        bridge(super::helper_call(helper, vec![l, r]))
    }

    #[test]
    fn each_value_operator_shape_is_a_node_over_node_children() {
        let n = |s: &str| node(id(s));
        // Plain operator and `Float /`: `Binary`.
        for (op, want) in [
            (BinOp::Add, "a + b"),
            (BinOp::Sub, "a - b"),
            (BinOp::Mul, "a * b"),
            (BinOp::Div, "a / b"),
            (BinOp::Eq, "a === b"),
            (BinOp::NotEq, "a !== b"),
            (BinOp::Lt, "a < b"),
            (BinOp::LtEq, "a <= b"),
            (BinOp::Gt, "a > b"),
            (BinOp::GtEq, "a >= b"),
        ] {
            let got = t_plain_binary(op, n("a"), n("b"));
            assert!(
                matches!(&got, LoweredExpr::Node(bynk_ts::TsExpr::Binary { .. })),
                "{want}"
            );
            assert_eq!(got.into_text(), want);
        }
        // Decimal, `Int /` and structural `==`: a call over a genuine identifier.
        let call = t_helper_call("__bynkDecimalAdd", n("a"), n("b"));
        match &call {
            LoweredExpr::Node(bynk_ts::TsExpr::Call { callee, args }) => {
                assert!(matches!(&**callee, bynk_ts::TsExpr::Ident(h) if h == "__bynkDecimalAdd"));
                assert_eq!(args.len(), 2);
            }
            _ => panic!("a node pair gives a call node"),
        }
        assert_eq!(call.into_text(), "__bynkDecimalAdd(a, b)");
        // Structural `!=`: `Unary(Not, Call)`.
        let ne = bridge(super::negated(super::helper_call(
            "__bynkEq",
            vec![n("a"), n("b")],
        )));
        assert!(matches!(
            &ne,
            LoweredExpr::Node(bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                ..
            })
        ));
        assert_eq!(ne.into_text(), "!__bynkEq(a, b)");
        // `Ord`: `cmp(l, r) < 0`.
        let ord = bridge(super::ordered_comparison(
            BinOp::Lt,
            id("__cmp_P"),
            vec![n("a"), n("b")],
        ));
        assert!(matches!(
            &ord,
            LoweredExpr::Node(bynk_ts::TsExpr::Binary { .. })
        ));
        assert_eq!(ord.into_text(), "__cmp_P(a, b) < 0");
    }

    #[test]
    fn a_text_operand_makes_each_value_operator_fall_back_to_todays_text() {
        let t = || text("f(x)");
        let n = || node(id("b"));
        for (got, want) in [
            (t_plain_binary(BinOp::Sub, t(), n()), "f(x) - b"),
            (t_plain_binary(BinOp::Div, n(), t()), "b / f(x)"),
            (
                t_helper_call("__bynkDecimalMul", t(), n()),
                "__bynkDecimalMul(f(x), b)",
            ),
            (
                t_helper_call("__bynkIntDiv", n(), t()),
                "__bynkIntDiv(b, f(x))",
            ),
            (
                bridge(super::negated(super::helper_call(
                    "__bynkEq",
                    vec![t(), n()],
                ))),
                "!__bynkEq(f(x), b)",
            ),
            (
                bridge(super::ordered_comparison(
                    BinOp::GtEq,
                    id("__cmp_P"),
                    vec![t(), n()],
                )),
                "__cmp_P(f(x), b) >= 0",
            ),
        ] {
            assert!(!is_node(&got), "{want}");
            assert_eq!(got.into_text(), want);
        }
    }

    #[test]
    fn a_composite_comparator_is_a_node_over_nodes() {
        // 1A.16: the comparator is itself a node, so a composite one is an
        // ordinary callee (the 1A.5 text path is closed).
        let cmp = bynk_ts::TsExpr::Call {
            callee: Box::new(id("__bynkCmpList")),
            args: vec![id("__bynkCmpInt")],
        };
        let got = bridge(super::ordered_comparison(
            BinOp::Lt,
            cmp,
            vec![node(id("a")), node(id("b"))],
        ));
        assert!(is_node(&got));
        assert_eq!(got.into_text(), "__bynkCmpList(__bynkCmpInt)(a, b) < 0");
    }

    #[test]
    fn comparison_over_a_derived_ord_record_lowers_with_zero_text_exits() {
        // 1A.5's composite-comparator fallback, closed by 1A.16.
        let src = "commons ordrec\n\
            type P = { a: Int, b: String } derives Ord\n\
            fn lt(x: P, y: P) -> Bool { x < y }\n\
            fn lts(x: List[Int], y: List[Int]) -> Bool { x < y }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        assert!(
            returned(&ts, "lt").contains("(x, y) < 0"),
            "{}",
            returned(&ts, "lt")
        );
        assert_eq!(
            returned(&ts, "lts"),
            "__bynkCmpList(__bynkCmpInt)(x, y) < 0"
        );
    }

    #[test]
    fn binary_nodes_keep_their_brackets_inside_text_parents() {
        // Source brackets are `Paren` nodes, so a printed operator node spliced
        // into an unconverted text parent (a method-call receiver, a unary
        // operand) keeps them end to end.
        let ts = crate::testkit::emit_source(
            "commons parents\n\
             fn f(a: Int, b: Int) -> String { (a + b).toString() }\n\
             fn g(a: Int, b: Int, c: Int) -> Int { (a - b) * c }\n\
             fn h(a: Int, b: Int) -> Int { -(a + b) }\n",
        );
        assert_eq!(returned(&ts, "g"), "(a - b) * c");
        assert_eq!(returned(&ts, "h"), "-(a + b)");
        assert!(
            returned(&ts, "f").contains("(a + b)"),
            "{}",
            returned(&ts, "f")
        );
    }

    #[test]
    fn value_operators_over_identifiers_print_by_the_precedence_table() {
        let src = "commons prec\n\
                   fn a(a: Int, b: Int, c: Int) -> Int { a - b - c }\n\
                   fn b(a: Int, b: Int, c: Int) -> Int { a - (b - c) }\n\
                   fn c(a: Float, b: Float, c: Float) -> Float { a / b * c }\n\
                   fn d(a: Int, b: Int, c: Int) -> Int { a * (b + c) }\n\
                   fn e(a: Int, b: Int, c: Int, d: Int) -> Bool { a < b == c < d }\n\
                   fn f(a: Int, b: Int) -> Bool { !(a == b) }\n\
                   fn g(a: Int, b: Int) -> Int { -(a * b) }\n\
                   fn h(a: Int, b: Int, c: Int) -> Int { a + b * c }\n\
                   fn i(a: Int, b: Int, c: Int) -> Int { a - b + c }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        for (name, want) in [
            ("a", "a - b - c"),
            ("b", "a - (b - c)"),
            ("c", "a / b * c"),
            ("d", "a * (b + c)"),
            ("e", "a < b === c < d"),
            ("f", "!(a === b)"),
            ("g", "-(a * b)"),
            ("h", "a + b * c"),
            ("i", "a - b + c"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
    }

    #[test]
    fn helper_branches_lower_to_nodes_and_still_note_their_imports() {
        let src = "commons helpers\n\
                   type P = { a: Int }\n\
                   fn dec(x: Decimal, y: Decimal) -> Decimal { x + y }\n\
                   fn decs(x: Decimal, y: Decimal) -> Decimal { x - y }\n\
                   fn decm(x: Decimal, y: Decimal) -> Decimal { x * y }\n\
                   fn idiv(x: Int, y: Int) -> Int { x / y }\n\
                   fn fdiv(x: Float, y: Float) -> Float { x / y }\n\
                   fn eq(x: P, y: P) -> Bool { x == y }\n\
                   fn ne(x: P, y: P) -> Bool { x != y }\n\
                   fn ord[T: Ord](x: T, y: T) -> Bool { x < y }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        for (name, want) in [
            ("dec", "__bynkDecimalAdd(x, y)"),
            ("decs", "__bynkDecimalSub(x, y)"),
            ("decm", "__bynkDecimalMul(x, y)"),
            ("idiv", "__bynkIntDiv(x, y)"),
            ("fdiv", "x / y"),
            ("eq", "__bynkEq(x, y)"),
            ("ne", "!__bynkEq(x, y)"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
        assert!(
            ts.contains("export function ord<T>(x: T, y: T, __cmp_T: (a: T, b: T) => number): boolean {\n  return __cmp_T(x, y) < 0;\n}"),
            "{ts}"
        );
        for helper in [
            "__bynkDecimalAdd",
            "__bynkDecimalSub",
            "__bynkDecimalMul",
            "__bynkIntDiv",
            "__bynkEq",
        ] {
            assert!(
                ts.contains(&format!("{helper},")) || ts.contains(&format!("{helper} }}")),
                "{helper} is imported:\n{ts}"
            );
        }
        // The text path (a call operand) imports the same helpers.
        let text_src = "commons helpers2\n\
                        fn d() -> Decimal { 1.5d }\n\
                        fn dec(x: Decimal) -> Decimal { x + d() }\n\
                        fn idiv(x: Int) -> Int { x / 2 }\n";
        let ts = crate::testkit::emit_source(text_src);
        assert!(ts.contains("__bynkDecimalAdd"), "{ts}");
        assert!(ts.contains("__bynkIntDiv"), "{ts}");
    }

    #[test]
    fn short_circuit_operators_over_nodes_lower_to_nodes() {
        // 1A.14: `&&`, `||` and `implies` over node operands are nodes; `implies`
        // keeps its explicit brackets.
        let src = "commons shortc\n\
                   fn a(x: Bool, y: Bool) -> Bool { x && y }\n\
                   fn b(x: Bool, y: Bool) -> Bool { x || y }\n\
                   fn c(x: Bool, y: Bool) -> Bool { x implies y }\n\
                   fn d(x: Bool, y: Bool, z: Bool) -> Bool { (x || y) && z }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "a"), "x && y");
        assert_eq!(returned(&ts, "b"), "x || y");
        assert_eq!(returned(&ts, "c"), "(!(x) || y)");
        assert_eq!(returned(&ts, "d"), "(x || y) && z");
    }

    #[test]
    fn short_circuit_operators_over_a_text_operand_keep_their_text() {
        // A string `contains` is still text (a method-call family), so the
        // operator over it is text, byte for byte.
        let src = "commons shortt\n\
                   fn a(s: String, t: String, y: Bool) -> Bool { s.contains(t) && y }\n\
                   fn b(s: String, t: String, y: Bool) -> Bool { y || s.contains(t) }\n\
                   fn c(s: String, t: String, y: Bool) -> Bool { s.contains(t) implies y }\n";
        assert!(text_exits(src) >= 3);
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "a"), "s.includes(t) && y");
        assert_eq!(returned(&ts, "b"), "y || s.includes(t)");
        assert_eq!(returned(&ts, "c"), "(!(s.includes(t)) || y)");
    }

    #[test]
    fn identifier_node_helpers_build_genuine_names() {
        let m = member_node("deps", "who");
        assert!(matches!(&m, bynk_ts::TsExpr::Member { object, property }
            if property == "who" && matches!(&**object, bynk_ts::TsExpr::Ident(n) if n == "deps")));
        assert_eq!(bynk_ts::print_expr(&m), "deps.who");
        assert_eq!(
            bynk_ts::print_expr(&ident_node("__bynkRunId")),
            "__bynkRunId"
        );
    }

    /// An `if (flag) { done(); }` statement node: multi-line when printed.
    fn if_node() -> bynk_ts::TsStmt {
        bynk_ts::TsStmt::if_stmt(
            bynk_ts::TsExpr::Ident("flag".to_string()),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::expr_stmt(
                    bynk_ts::TsExpr::Ident("done".to_string()),
                    None,
                )],
                None,
            ),
            None,
        )
    }

    /// The same statement as three text hoists, one per line, the shape a text
    /// producer gives a nested statement (nesting travels inside the string).
    fn if_text(pre: &mut Pre) {
        pre.push_text("if (flag) {".to_string());
        pre.push_text("  done;".to_string());
        pre.push_text("}".to_string());
    }

    #[test]
    fn a_node_statement_indents_every_line_like_its_text_equivalent() {
        let mut as_node = Pre::new();
        as_node.0.push(LoweredStmt::Node(Box::new(if_node())));
        let mut as_text = Pre::new();
        if_text(&mut as_text);
        assert_eq!(as_node.stmts(), as_text.stmts());
        for indent in [0, 2, 6] {
            let (mut n, mut t) = (String::new(), String::new());
            for s in &as_node.stmts() {
                write_line(&mut n, indent, s);
            }
            for s in &as_text.stmts() {
                write_line(&mut t, indent, s);
            }
            assert_eq!(n, t, "indent {indent}");
            assert!(
                n.lines().all(|l| l.starts_with(&" ".repeat(indent))),
                "every line carries the caller's indentation:\n{n}"
            );
        }
        // `Lowered::pre_texts` takes the same path.
        let lowered = as_node.finish(LoweredExpr::Node(num_lit("1".to_string())));
        assert_eq!(lowered.pre_texts(), ["if (flag) {", "  done;", "}"]);
    }

    #[test]
    fn a_node_statement_sits_at_the_depth_of_its_raw_text_siblings() {
        // `ts_stmts` feeds a body the printer nests: raw text carries its own
        // indent, a node is nested by the printer at the same depth.
        let mut pre = Pre::new();
        pre.push_text("const a = 1;".to_string());
        pre.0.push(LoweredStmt::Node(Box::new(if_node())));
        pre.push_text("const b = 2;".to_string());
        let body = bynk_ts::TsStmt::block(pre.ts_stmts("  "), None);
        let printed = bynk_ts::print_stmt(&body, 0);
        let want = "{\n  const a = 1;\n  if (flag) {\n    done;\n  }\n  const b = 2;\n}\n";
        assert_eq!(printed, want);
    }

    // -- 1A.6 (#3705): calls --

    const CALLS: &str = "commons calls\n\
        type S = | A(n: Int) | B\n\
        type Box2 = { v: Int }\n\
        fn Box2.size(self) -> Int { self.v }\n\
        fn g(x: Int, y: Int) -> Int { x }\n\
        fn plain(x: Int, y: Int) -> Int { g(x, y) }\n\
        fn ctor(x: Int) -> S { A(x) }\n\
        fn ufcs(b: Box2) -> Int { b.size() }\n\
        fn fault() -> Int { unreachable(\"no\\n\\\"way\\\"\") }\n";

    #[test]
    fn calls_over_identifiers_lower_to_nodes_and_still_note_their_imports() {
        assert_eq!(text_exits(CALLS), 0);
        let ts = crate::testkit::emit_source(CALLS);
        for (name, want) in [
            ("plain", "g(x, y)"),
            ("ctor", "S.A(x)"),
            ("ufcs", "Box2.size(b)"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
        // The fault call is a node (kind and location are string nodes) and
        // still imports `__fault`.
        let fault = returned(&ts, "fault");
        assert!(
            fault.starts_with("__fault(\"Unreachable\", \"no\\n\\\"way\\\"\", \""),
            "{fault}"
        );
        assert!(ts.contains("__fault"), "{ts}");
    }

    #[test]
    fn a_call_with_a_text_argument_falls_back_to_today_s_text() {
        // A string `contains` operand is still text (a method-call family), so
        // each call over it is text, byte for byte.
        let src = "commons calltext\n\
            type S = | A(f: Bool) | B\n\
            type Box2 = { v: Int }\n\
            fn Box2.take(self, b: Bool) -> Int { self.v }\n\
            fn g(b: Bool) -> Bool { b }\n\
            fn plain(a: String, c: String) -> Bool { g(a.contains(c)) }\n\
            fn ctor(a: String, c: String) -> S { A(a.contains(c)) }\n\
            fn ufcs(b: Box2, a: String, c: String) -> Int { b.take(a.contains(c)) }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(text_exits(src) > 0);
        assert_eq!(returned(&ts, "plain"), "g(a.includes(c))");
        assert_eq!(returned(&ts, "ctor"), "S.A(a.includes(c))");
        assert_eq!(returned(&ts, "ufcs"), "Box2.take(b, a.includes(c))");
    }

    #[test]
    fn call_arguments_keep_their_precedence() {
        let src = "commons callprec\n\
            fn g(x: Int, y: Int) -> Int { x }\n\
            fn sum(a: Int, b: Int) -> Int { g(a + b, a * b) }\n\
            fn neg(a: Int, b: Int) -> Int { g(-a, b - a) }\n\
            fn tern(a: Int, b: Int, c: Bool) -> Int { g(if c { a } else { b }, a) }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "sum"), "g(a + b, a * b)");
        assert_eq!(returned(&ts, "neg"), "g(-a, b - a)");
        let tern = returned(&ts, "tern");
        assert!(tern.starts_with("g("), "{tern}");
        assert!(tern.ends_with(", a)"), "{tern}");
    }

    #[test]
    fn witness_calls_are_nodes_and_explicit_type_argument_calls_stay_text() {
        // An `Ord`-bounded callee takes comparator witnesses, which are nodes
        // (1A.16 closes 1A.6's witness fallback).
        let witness = "commons witnesses\n\
            fn lt[T: Ord](a: T, b: T) -> Bool { a < b }\n\
            fn use(a: Int, b: Int) -> Bool { lt(a, b) }\n";
        let before = text_exits("commons witnesses\nfn use(a: Int, b: Int) -> Bool { a < b }\n");
        assert_eq!(text_exits(witness), before);
        let ts = crate::testkit::emit_source(witness);
        assert_eq!(returned(&ts, "use"), "lt(a, b, __bynkCmpInt)");
        // Explicit type arguments on a generic instance method (#2795): `Call`
        // has no `type_args`, so the call stays text.
        let explicit = "commons explicit\n\
            type Box2[A] = { v: A }\n\
            fn Box2.cast[U](self) -> Option[U] { None }\n\
            fn use(b: Box2[Int]) -> Option[Int] { b.cast[Int]() }\n";
        let ts = crate::testkit::emit_source(explicit);
        assert_eq!(returned(&ts, "use"), "Box2.cast<number, number>(b)");
    }

    // -- 1A.7 (#3706): data literals --

    const DATA: &str = "commons data\n\
        type P = { name: String, age: Int }\n\
        fn mk(n: String, a: Int) -> P { P { name: n, age: a } }\n\
        fn short(name: String, age: Int) -> P { P { name, age } }\n\
        fn spread(p: P, a: Int) -> P { P { ...p, age: a } }\n\
        fn greet(n: String) -> String { \"hi \\(n)!\" }\n\
        fn adjacent(a: Int, b: Int) -> String { \"\\(a)\\(b)\" }\n\
        fn money(d: Decimal) -> String { \"cost \\(d)\" }\n";

    #[test]
    fn data_literals_over_nodes_lower_to_nodes() {
        assert_eq!(text_exits(DATA), 0);
        let ts = crate::testkit::emit_source(DATA);
        for (name, want) in [
            ("mk", "{ name: n, age: a }"),
            ("short", "{ name, age }"),
            ("spread", "{ ...p, age: a }"),
            ("greet", "`hi ${String(n)}!`"),
            ("adjacent", "`${String(a)}${String(b)}`"),
            ("money", "`cost ${__bynkDecimalToString(d)}`"),
        ] {
            assert_eq!(returned(&ts, name), want, "{name}");
        }
        // `note_decimal` fires on the node path.
        assert!(ts.contains("__bynkDecimalToString"), "{ts}");
    }

    #[test]
    fn an_interpolated_string_escapes_its_chunks_exactly() {
        let src = "commons esc\n\
            fn f(n: String) -> String { \"a`b\\(n)c\\\\d $ ${n}\" }\n\
            fn g(n: String) -> String { \"\\(n)\" }\n\
            fn h(n: String) -> String { \"x\\(n)\\n\" }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "f"), "`a\\`b${String(n)}c\\\\d \\$ \\${n}`");
        assert_eq!(returned(&ts, "g"), "`${String(n)}`");
        assert_eq!(returned(&ts, "h"), "`x${String(n)}\\n`");
    }

    #[test]
    fn a_data_literal_with_a_text_child_falls_back_byte_for_byte() {
        // An explicit-type-argument method call is still text, so a record /
        // string around one stays text, printed exactly as before.
        let src = "commons textchild\n\
            type P = { name: String, age: Option[Int] }\n\
            type Box2[A] = { v: A }\n\
            fn Box2.cast[U](self) -> Option[U] { None }\n\
            fn rec(b: Box2[Int]) -> P { P { name: \"x\", age: b.cast[Int]() } }\n\
            fn s(b: Box2[Int]) -> String { \"v \\(b.cast[Int]().getOrElse(0))\" }\n";
        assert!(text_exits(src) > 0);
        let ts = crate::testkit::emit_source(src);
        assert!(ts.contains("{ name: \"x\", age: "), "{ts}");
        assert!(ts.contains("`v ${String("), "{ts}");
    }

    #[test]
    fn a_renamed_shorthand_field_keeps_its_wire_key() {
        // `default` and `class` are TS reserved words: the binding is renamed,
        // the key is not, and it prints unquoted (legal in an object literal).
        let src = "commons renamed\n\
            type R = { default: Int, class: Int }\n\
            fn mk(default: Int, class: Int) -> R { R { default, class } }\n\
            fn sp(r: R, default: Int) -> R { R { ...r, default } }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        assert_eq!(
            returned(&ts, "mk"),
            "{ default: __id_default, class: __id_class }"
        );
        assert_eq!(returned(&ts, "sp"), "{ ...r, default: __id_default }");
    }

    #[test]
    fn a_record_that_takes_a_declared_default_stays_text() {
        let src = "commons defaults\n\
            type C = { n: Int, label: String = \"none\" }\n\
            fn mk(n: Int) -> C { C { n } }\n\
            fn full(n: Int) -> C { C { n, label: \"x\" } }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "mk"), "{ n, label: \"none\" }");
        assert_eq!(returned(&ts, "full"), "{ n, label: \"x\" }");
        // Only the defaulted construction leaves as text.
        let only_full = "commons defaults\n\
            type C = { n: Int, label: String = \"none\" }\n\
            fn full(n: Int) -> C { C { n, label: \"x\" } }\n";
        assert!(text_exits(src) > text_exits(only_full));
        assert_eq!(text_exits(only_full), 0);
    }

    #[test]
    fn a_zero_field_record_prints_as_empty_braces() {
        // Layout-only: the old text was `{  }`, the printer's form is `{}`.
        // No fixture reaches a zero-field record.
        let src = "commons empty\n\
            type E = {}\n\
            fn mk() -> E { E {} }\n";
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "mk"), "{}");
    }

    #[test]
    fn a_record_literal_as_a_lambda_body_keeps_its_brackets() {
        let src = "commons lam\n\
            type P = { v: Int }\n\
            fn f(xs: List[Int]) -> List[P] { xs.map((x) => P { v: x }) }\n";
        let ts = crate::testkit::emit_source(src);
        assert!(ts.contains("=> ({ v: x })"), "{ts}");
    }

    #[test]
    fn an_expect_expression_is_a_node_with_the_same_bytes() {
        // `_ => expect <pred>` is the expression form (the statement form has
        // its own structural report). Quotes and non-ASCII in the predicate's
        // source text are escaped exactly as `escape_ts_string` does.
        let out = crate::testkit::emit_project(
            &[
                ("a/m.bynk", "commons a.m\n\nfn one() -> Int { 1 }\n"),
                (
                    "tests/m.bynk",
                    "suite a.m\n\ncase \"c\" {\n  let r = match one() {\n    _ => expect \"q\\\"\u{e9}\" == \"x\"\n  }\n}\n",
                ),
            ],
            bynk_check::project_model::BuildTarget::Bundle,
        );
        let text: String = out.docs.values().cloned().collect();
        let at = text
            .find("__bynkExpect((")
            .unwrap_or_else(|| panic!("no expression-form expect in:\n{text}"));
        let line = text[at..].lines().next().unwrap_or_default();
        let want_src = crate::emitter::escape_ts_string("expect \"q\\\"\u{e9}\" == \"x\"");
        assert!(line.contains(&want_src), "{line}");
        assert!(line.starts_with("__bynkExpect((\""), "{line}");
    }

    #[test]
    fn expect_location_and_source_print_as_their_escaped_text() {
        // The `Str` nodes carry the raw strings; the printer's escape is the
        // one `escape_ts_string` applies to the text form.
        for raw in [
            "tests/a.test.bynk:3:7",
            "expect a == \"\u{e9}\\n\" `x` ${y}",
        ] {
            let node = str_node(raw);
            assert_eq!(
                bynk_ts::print_expr(&node),
                format!("\"{}\"", crate::emitter::escape_ts_string(raw))
            );
        }
    }

    #[test]
    fn http_and_queue_result_variant_calls_are_nodes() {
        // 1A.6 review nit: direct node-path assertions for the `HttpResult.V(…)`
        // and `QueueResult.V(…)` call forms, with no text exit.
        let src = "commons results\n\
            fn created(x: Int) -> HttpResult[Int] { Created(x) }\n\
            fn raw(b: Bytes) -> HttpResult[Int] { RawError(Forbidden, b, \"text/plain\") }\n";
        assert_eq!(text_exits(src), 0);
        let ts = crate::testkit::emit_source(src);
        assert_eq!(returned(&ts, "created"), "HttpResult.Created(x)");
        assert_eq!(
            returned(&ts, "raw"),
            "HttpResult.RawError(403, b, \"text/plain\")"
        );
    }

    /// 1A.8: `(text expressions, text statements)` hoisted while emitting
    /// `src`, with the emitted TypeScript.
    fn exits_and_stmts(src: &str) -> (usize, usize, String) {
        TEXT_EXITS.with(|c| c.set(0));
        TEXT_STMTS.with(|c| c.set(0));
        let ts = crate::testkit::emit_source(src);
        (
            TEXT_EXITS.with(|c| c.get()),
            TEXT_STMTS.with(|c| c.get()),
            ts,
        )
    }

    #[test]
    fn a_question_over_a_node_operand_hoists_statement_nodes() {
        let src = "commons qnodes\n\
            fn f(x: Result[Int, String]) -> Result[Int, String] { Ok(x? + 1) }\n\
            fn g(x: Result[Int, String]) -> Result[Int, String] { Ok(x?) }\n";
        let (exits, stmts, ts) = exits_and_stmts(src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert!(
            ts.contains(
                "  const __r0 = x;\n  if (__r0.tag === \"Err\") return __r0;\n  return Ok(__r0.value + 1);\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_over_an_option_in_an_http_result_function_exits_with_not_found() {
        // 1A.8 review nit: the `NotFound` exit, driven from source. Both
        // statements are nodes, and the guard prints as today's text does.
        let src = "commons nf\n\
            fn lookup(i: Int) -> Option[Int] { if i > 0 { Some(i) } else { None } }\n\
            fn f(i: Int) -> HttpResult[Int] { Ok(lookup(i)?) }\n";
        let (exits, stmts, ts) = exits_and_stmts(src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert!(
            ts.contains(
                "  const __r0 = lookup(i);\n  if (__r0.tag === \"None\") return HttpResult.NotFound;\n  return HttpResult.Ok(__r0.value);\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_that_embeds_its_error_wraps_it_in_the_declared_variant() {
        // 1A.8 review nit: the `Embed` exit (`embeds E as V`), driven from source.
        let src = "commons emb\n\
            type IoError = enum { Disk }\n\
            type AppError =\n  | Io(reason: IoError)\n  | Other\n  embeds IoError as Io\n\
            fn g(r: Result[Int, IoError]) -> Result[Int, AppError] { Ok(r? + 1) }\n";
        let (exits, stmts, ts) = exits_and_stmts(src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert!(
            ts.contains(
                "  const __r0 = r;\n  if (__r0.tag === \"Err\") return Err(AppError.Io(__r0.error));\n  return Ok(__r0.value + 1);\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_over_a_value_position_match_is_a_node() {
        // 1A.15: a `match` in value position is a node (its IIFE) when its arm
        // tails are, so a `?` over one is a node as well: no statement stays a
        // text line, and the guard is the same bytes.
        let src = "commons qtext\n\
            fn f(x: Result[Int, String], n: Int) -> Result[Int, String] {\n\
              Ok((match n { 0 => x, _ => x })?)\n\
            }\n";
        let (exits, stmts, ts) = exits_and_stmts(src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert!(
            ts.contains("  const __r0 = (((__d) => {")
                && ts.contains(
                    "\n  if (__r0.tag === \"Err\") return __r0;\n  return Ok(__r0.value);\n"
                ),
            "{ts}"
        );
    }

    #[test]
    fn every_question_guard_prints_as_its_text_twin() {
        // The node guard and the text guard are one shape; print each exit
        // both ways and require the same bytes (including the case form's
        // escaped location, source and `\n` in the detail).
        for exit in [
            QuestionExit::Plain,
            QuestionExit::NotFound,
            QuestionExit::Embed {
                ty_name: "AppError".to_string(),
                variant: "Db".to_string(),
            },
            QuestionExit::Case {
                location: "tests/a \"b\".test.bynk:3:7".to_string(),
                start: 10,
                end: 22,
                src: "f(\"\u{e9}\\n\")?\n`x` ${y}".to_string(),
            },
        ] {
            let node = bynk_ts::print_stmt(&question_guard_node("__r4", &exit), 0);
            assert_eq!(node, format!("{}\n", question_guard_text("__r4", &exit)));
        }
    }

    #[test]
    fn statement_nodes_re_indent_at_depth_through_both_consumers() {
        // 1A.8, Decision C: a multi-line node (the hoisted `if`) printed at
        // depth 0 by `lines()` and re-indented by the caller must equal the
        // flat text lines it replaces; `ts_stmts` keeps it as a node.
        let node = bynk_ts::TsStmt::if_else_same_line_stmt(
            ident_node("c"),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::assign(
                    ident_node("__r1"),
                    ident_node("a"),
                    None,
                )],
                None,
            ),
            bynk_ts::TsStmt::block(
                vec![bynk_ts::TsStmt::assign(
                    ident_node("__r1"),
                    ident_node("b"),
                    None,
                )],
                None,
            ),
            None,
        );
        let mut pre = Pre::new();
        pre.push_node(node);
        let text_lines = [
            "if (c) {".to_string(),
            "  __r1 = a;".to_string(),
            "} else {".to_string(),
            "  __r1 = b;".to_string(),
            "}".to_string(),
        ];
        assert_eq!(pre.stmts(), text_lines);
        // Written two levels deep, as `write_line` indents each entry.
        let mut out = String::new();
        for l in pre.stmts() {
            write_line(&mut out, INDENT_STEP * 2, &l);
        }
        assert_eq!(
            out,
            "    if (c) {\n      __r1 = a;\n    } else {\n      __r1 = b;\n    }\n"
        );
        // `ts_stmts` keeps the node; the printer at depth 2 prints the same bytes.
        let stmts = pre.ts_stmts("    ");
        assert_eq!(stmts.len(), 1);
        assert_eq!(bynk_ts::print_stmt(&stmts[0], 2), out);
    }

    #[test]
    fn a_ternary_over_a_text_branch_falls_back_to_text() {
        let src = "commons ternt\n\
            fn f(c: Bool, x: String, y: String) -> Bool { if c { x.contains(y) } else { false } }\n";
        let (exits, _, ts) = exits_and_stmts(src);
        assert!(exits > 0, "{ts}");
        assert_eq!(returned(&ts, "f"), "(c ? x.includes(y) : false)");
    }

    #[test]
    fn a_hoisted_if_over_nodes_is_built_from_nodes() {
        let src = "commons ifnodes\n\
            fn f(c: Bool, x: Result[Int, String]) -> Result[Int, String] {\n\
              let v = if c { x? } else { 0 }\n\
              Ok(v)\n\
            }\n";
        let (exits, stmts, ts) = exits_and_stmts(src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert!(
            ts.contains(
                "  let __r1: number;\n  if (c) {\n    const __r0 = x;\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value;\n  } else {\n    __r1 = 0;\n  }\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_question_nested_two_deep_in_hoisted_ifs_indents_per_level() {
        // #1750 / T2.1: the multi-line node is re-indented by `lines()` into a
        // handler body at depth, one printer level per nested `if`.
        let src = "commons ifnest\n\
            fn f(a: Bool, b: Bool, x: Result[Int, String]) -> Result[Int, String] {\n\
              let v = if a { if b { x? } else { 1 } } else { 2 }\n\
              Ok(v)\n\
            }\n";
        let (exits, stmts, ts) = exits_and_stmts(src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert!(
            ts.contains("  let __r2: number;\n  if (a) {\n    let __r1: number;\n    if (b) {\n"),
            "{ts}"
        );
        assert!(
            ts.contains(
                "    if (b) {\n      const __r0 = x;\n      if (__r0.tag === \"Err\") return __r0;\n      __r1 = __r0.value;\n    } else {\n      __r1 = 1;\n    }\n    __r2 = __r1;\n  } else {\n    __r2 = 2;\n  }\n"
            ),
            "{ts}"
        );
    }

    #[test]
    fn a_ternary_over_nodes_is_a_paren_conditional() {
        let src = "commons tern\n\
            fn f(c: Bool, x: Int, y: Int) -> Int { if c { x } else { y } }\n";
        let (exits, stmts, ts) = exits_and_stmts(src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert_eq!(returned(&ts, "f"), "(c ? x : y)");
    }

    /// The emitted suite module for a one-case suite over `src`. It lowers
    /// through the driver, so it returns no text-exit counts: `TEXT_EXITS` is
    /// compiled only into this crate's own test build, and the driver runs a
    /// separate copy of the emitter (see `text_counters`).
    fn suite_ts(src: &str, suite: &str) -> String {
        let out = crate::testkit::emit_project(
            &[("demo/day.bynk", src), ("tests/day.bynk", suite)],
            bynk_check::project_model::BuildTarget::Bundle,
        );
        out.text("tests/demo_day.test.ts").to_string()
    }

    const DAY: &str = "commons demo.day\n\ntype Day = String where NonEmpty\n\nfn len(d: Day) -> Int { d.length() }\n";

    #[test]
    fn a_question_in_a_test_case_fails_the_case_from_nodes() {
        // #2857: the case form is an `InlineBlock` holding the `throw`; it
        // prints on the one line the text form has, escapes included.
        let ts = suite_ts(
            DAY,
            "suite demo.day\n\ncase \"q\" {\n  let d = Day.of(\"x\")?\n  expect len(d) == 1\n}\n",
        );
        let at = ts.find("if (__r").expect("a `?` guard");
        let line = ts[at..].lines().next().unwrap();
        assert!(
            line.starts_with(
                "if (__r0.tag === \"Err\") { throw __bynkExpectFailure(\"tests/day.bynk:4:11\", "
            ) && line.ends_with("\\n  the operand was Err: \" + __bynkShow(__r0.error)); }"),
            "{line}"
        );
    }

    #[test]
    fn a_question_in_a_hoisted_if_branch_in_a_test_case_is_nodes() {
        let ts = suite_ts(
            DAY,
            "suite demo.day\n\ncase \"q\" {\n  let c = true\n  let n = if c { len(Day.of(\"x\")?) } else { 0 }\n  expect n == 1\n}\n",
        );
        // The nested statements keep the printer's one-level indent per `if`
        // inside the case body.
        assert!(ts.contains("if (c) {\n"), "{ts}");
        let at = ts.find("let __r").expect("a hoisted slot");
        let slot_block: Vec<&str> = ts[at..].lines().take(8).collect();
        let indent = |l: &str| l.len() - l.trim_start().len();
        let base = indent(slot_block[1]);
        assert_eq!(slot_block[1].trim_start(), "if (c) {");
        assert_eq!(indent(slot_block[2]), base + 2, "{slot_block:?}");
        assert!(
            slot_block[3].trim_start().starts_with("if (__r0.tag"),
            "{slot_block:?}"
        );
        assert_eq!(indent(slot_block[3]), base + 2, "{slot_block:?}");
        assert_eq!(indent(slot_block[5]), base, "{slot_block:?}");
    }

    #[test]
    fn a_hoisted_if_with_a_text_branch_keeps_its_text_lines() {
        // A branch tail that is still text (a string `contains`) keeps the whole
        // hoist as text lines, byte for byte, with `assign_branch`'s padding.
        let src = "commons iftext\n\
            fn f(c: Bool, x: String, y: String, r: Result[Bool, String]) -> Result[Bool, String] {\n\
              let v = if c { r? } else { x.contains(y) }\n\
              Ok(v)\n\
            }\n";
        let (exits, stmts, ts) = exits_and_stmts(src);
        assert!(exits > 0 && stmts > 0, "{ts}");
        assert!(
            ts.contains("  let __r1: boolean;\n  if (c) {\n    const __r0 = r;\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value;\n  } else {\n    __r1 = x.includes(y);\n  }\n"),
            "{ts}"
        );
    }

    /// The function `name` as emitted, from its header to its closing brace.
    fn func(ts: &str, name: &str) -> String {
        let head = format!("export function {name}(");
        let at = ts.find(&head).unwrap_or_else(|| panic!("no {name}:\n{ts}"));
        let rest = &ts[at..];
        rest[..rest.find("\n}\n").expect("a closing brace") + 3].to_string()
    }

    const IS_HDR: &str = "commons is1\n\ntype Shape = | Circle(r: Int) | Square(w: Int) | Dot\nfn mk(s: Shape) -> Shape { s }\nfn bump(n: Int) -> Int { n }\nfn risky(n: Int) -> Result[Int, String] { Ok(n) }\n";

    /// 1A.14: `is` over every pattern kind is a node, with the identity-arrow
    /// wrapper (#2836) on a tag test and none on a wildcard.
    #[test]
    fn is_over_every_pattern_kind_lowers_to_nodes() {
        let src = format!(
            "{IS_HDR}fn v(s: Shape) -> Bool {{ s is Circle(_) }}\n\
             fn n(s: Option[Shape]) -> Bool {{ s is Some(Circle(_)) }}\n\
             fn o(s: Shape) -> Bool {{ s is Circle(_) | Square(_) }}\n\
             fn w(s: Shape) -> Bool {{ s is _ }}\n\
             fn l(xs: List[Int]) -> Bool {{ xs is [1, ..] }}\n"
        );
        let (exits, stmts, ts) = exits_and_stmts(&src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert_eq!(
            returned(&ts, "v"),
            "((__c: boolean) => __c)(s.tag === \"Circle\")"
        );
        assert_eq!(
            returned(&ts, "n"),
            "((__c: boolean) => __c)(s.tag === \"Some\" && s.value.tag === \"Circle\")"
        );
        assert_eq!(
            returned(&ts, "o"),
            "((__c: boolean) => __c)((s.tag === \"Circle\" || s.tag === \"Square\"))"
        );
        assert_eq!(returned(&ts, "w"), "true");
        assert_eq!(returned(&ts, "l"), "xs.length >= 1 && xs[0] === 1");
    }

    /// A complex receiver is evaluated once into a temp, a `const` statement
    /// node; inside a hoisting right operand it is a hoisted declaration and an
    /// in-place assignment (#1654), the assignment a node.
    #[test]
    fn an_is_receiver_temp_is_a_const_node_or_an_in_place_assignment() {
        let src = format!(
            "{IS_HDR}fn a(s: Shape, k: Int) -> Bool {{ mk(s) is Dot && k > 0 }}\n\
             fn b(s: Shape, k: Int) -> Bool {{ k > 0 && mk(s) is Dot }}\n"
        );
        let (exits, stmts, ts) = exits_and_stmts(&src);
        // The hoisted `let __r0!: Shape;` declaration is the one text statement.
        assert_eq!((exits, stmts), (0, 1), "{ts}");
        assert_eq!(
            func(&ts, "a"),
            "export function a(s: Shape, k: number): boolean {\n  const __r0 = mk(s);\n  return ((__c: boolean) => __c)(__r0.tag === \"Dot\") && k > 0;\n}\n"
        );
        assert_eq!(
            func(&ts, "b"),
            "export function b(s: Shape, k: number): boolean {\n  let __r0!: Shape;\n  return k > 0 && (() => { __r0 = mk(s); return ((__c: boolean) => __c)(__r0.tag === \"Dot\"); })();\n}\n"
        );
    }

    /// `&&` and `implies` with an `is` antecedent, the three cases: no
    /// bindings, the binding IIFE, and a right operand holding a `?` (hoisted
    /// as an `if`, the antecedent kept in the returned expression for `tsc`'s
    /// narrowing, #1044).
    #[test]
    fn and_and_implies_with_is_bindings_cover_their_three_cases() {
        let src = format!(
            "{IS_HDR}fn none(s: Shape, k: Bool) -> Bool {{ s is Dot && k }}\n\
             fn iife(xs: List[Int], k: Int) -> Bool {{ xs is [x] && x > k }}\n\
             fn imp(xs: List[Int], k: Int) -> Bool {{ xs is [x, ..rest] implies x > k }}\n\
             fn ret(xs: List[Int], k: Int) -> Result[Bool, String] {{ Ok(xs is [x, ..] && risky(x)? > k) }}\n\
             fn retimp(xs: List[Int], k: Int) -> Result[Bool, String] {{ Ok(xs is [x, ..] implies risky(x)? > k) }}\n"
        );
        let (exits, stmts, ts) = exits_and_stmts(&src);
        assert_eq!((exits, stmts), (0, 0), "{ts}");
        assert_eq!(
            returned(&ts, "none"),
            "((__c: boolean) => __c)(s.tag === \"Dot\") && k"
        );
        assert_eq!(
            returned(&ts, "iife"),
            "xs.length === 1 && ((() => { const x = xs[0]; return x > k; })())"
        );
        assert_eq!(
            returned(&ts, "imp"),
            "(!(xs.length >= 1) || ((() => { const x = xs[0]; const rest = xs.slice(1); return x > k; })()))"
        );
        assert_eq!(
            func(&ts, "ret"),
            "export function ret(xs: readonly number[], k: number): Result<boolean, string> {\n  let __r1: boolean;\n  if (xs.length >= 1) {\n    const x = xs[0];\n    const __r0 = risky(x);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > k;\n  } else {\n    __r1 = false;\n  }\n  return Ok(xs.length >= 1 && __r1);\n}\n"
        );
        assert_eq!(
            func(&ts, "retimp"),
            "export function retimp(xs: readonly number[], k: number): Result<boolean, string> {\n  let __r1: boolean;\n  if (xs.length >= 1) {\n    const x = xs[0];\n    const __r0 = risky(x);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > k;\n  } else {\n    __r1 = true;\n  }\n  return Ok((!(xs.length >= 1) || __r1));\n}\n"
        );
    }

    /// A variant binding reads its payload through the `Extract` cast, which has
    /// no type node, so the whole IIFE keeps its text, byte for byte; the
    /// antecedent stays in the hoisted form for narrowing (#1044).
    #[test]
    fn a_variant_payload_binding_keeps_its_iife_text() {
        let src = format!(
            "{IS_HDR}fn g(s: Shape, k: Int) -> Bool {{ s is Circle(r) && r > k }}\n\
             fn h(s: Shape, k: Int) -> Result[Bool, String] {{ Ok(s is Circle(r) && risky(r)? > k) }}\n"
        );
        let (exits, stmts, ts) = exits_and_stmts(&src);
        assert!(exits > 0 && stmts > 0, "{ts}");
        assert_eq!(
            returned(&ts, "g"),
            "((__c: boolean) => __c)(s.tag === \"Circle\") && ((() => { const r = (s as Extract<typeof s, { tag: \"Circle\" }>).r; return r > k; })())"
        );
        assert_eq!(
            func(&ts, "h"),
            "export function h(s: Shape, k: number): Result<boolean, string> {\n  let __r1: boolean;\n  if (((__c: boolean) => __c)(s.tag === \"Circle\")) {\n    const r = (s as Extract<typeof s, { tag: \"Circle\" }>).r;\n    const __r0 = risky(r);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > k;\n  } else {\n    __r1 = false;\n  }\n  return Ok(((__c: boolean) => __c)(s.tag === \"Circle\") && __r1);\n}\n"
        );
    }

    /// Plain `&&`, `||` and `implies` with a hoisting right operand: without an
    /// early return the right operand sits in a compact arrow so its hoist runs
    /// only when reached; with one it is a hoisted `if`, `false` / `true` in the
    /// other arm.
    #[test]
    fn plain_short_circuit_with_a_hoisting_rhs_is_an_arrow_or_a_hoisted_if() {
        let src = format!(
            "{IS_HDR}fn a(x: Int, s: Shape) -> Bool {{ x > 0 && mk(s) is Dot }}\n\
             fn b(x: Int, s: Shape) -> Bool {{ x > 0 || mk(s) is Dot }}\n\
             fn c(x: Int, s: Shape) -> Bool {{ x > 0 implies mk(s) is Dot }}\n\
             fn d(x: Int) -> Result[Bool, String] {{ Ok(x > 0 && risky(x)? > 0) }}\n\
             fn e(x: Int) -> Result[Bool, String] {{ Ok(x > 0 || risky(x)? > 0) }}\n\
             fn f(x: Int) -> Result[Bool, String] {{ Ok(x > 0 implies risky(x)? > 0) }}\n"
        );
        let (exits, _, ts) = exits_and_stmts(&src);
        assert_eq!(exits, 0, "{ts}");
        let arrow =
            "(() => { __r0 = mk(s); return ((__c: boolean) => __c)(__r0.tag === \"Dot\"); })()";
        assert_eq!(returned(&ts, "a"), format!("x > 0 && {arrow}"));
        assert_eq!(returned(&ts, "b"), format!("x > 0 || {arrow}"));
        assert_eq!(returned(&ts, "c"), format!("(!(x > 0) || {arrow})"));
        let hoisted = |cond: &str, then: &str, other: &str| {
            format!(
                "  let __r1: boolean;\n  if ({cond}) {{\n{then}  }} else {{\n{other}  }}\n  return Ok(__r1);\n}}\n"
            )
        };
        let reached = "    const __r0 = risky(x);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > 0;\n";
        let skipped_true = "    __r1 = true;\n";
        let skipped_false = "    __r1 = false;\n";
        assert!(
            func(&ts, "d").ends_with(&hoisted("x > 0", reached, skipped_false)),
            "{ts}"
        );
        assert!(
            func(&ts, "e").ends_with(&hoisted("x > 0", skipped_true, reached)),
            "{ts}"
        );
        assert!(
            func(&ts, "f").ends_with(&hoisted("!(x > 0)", skipped_true, reached)),
            "{ts}"
        );
    }

    /// A text operand or hoisted statement (a string `contains`) keeps the text
    /// forms byte for byte: the binding arrow, the hoisted `if`, the plain arrow.
    #[test]
    fn short_circuit_text_fallbacks_keep_their_text() {
        let src = "commons sct\n\
             fn risky(n: Int) -> Result[Int, String] { Ok(n) }\n\
             fn a(s: String, k: Bool) -> Bool { k && s.contains(s) }\n\
             fn b(s: String, x: Int) -> Result[Bool, String] { Ok(s.contains(s) && risky(x)? > 0) }\n";
        let (exits, _, ts) = exits_and_stmts(src);
        assert!(exits > 0, "{ts}");
        assert_eq!(returned(&ts, "a"), "k && s.includes(s)");
        assert_eq!(
            func(&ts, "b"),
            "export function b(s: string, x: number): Result<boolean, string> {\n  let __r1: boolean;\n  if (s.includes(s)) {\n    const __r0 = risky(x);\n    if (__r0.tag === \"Err\") return __r0;\n    __r1 = __r0.value > 0;\n  } else {\n    __r1 = false;\n  }\n  return Ok(__r1);\n}\n"
        );
    }
}

/// 1A.18a (#3719): the `List` kernel's expression-bodied arms build nodes.
#[cfg(test)]
mod list_kernel_tests {
    use crate::testkit::emit_source;

    /// `(text exits, emitted TypeScript)` for a one-function commons.
    fn exits(body: &str) -> (usize, String) {
        super::TEXT_EXITS.with(|c| c.set(0));
        let ts = emit_source(&format!("commons lk\n{body}\n"));
        (super::TEXT_EXITS.with(|c| c.get()), ts)
    }

    /// The returned expression of `export function f`.
    fn returned(ts: &str) -> String {
        let at = ts.find("export function f(").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    /// Each arm over node operands prints exactly today's text and leaves no
    /// expression as text.
    #[test]
    fn expression_arms_over_nodes_print_as_before_with_no_text_exit() {
        for (body, want) in [
            ("fn f(xs: List[Int]) -> Int { xs.length() }", "(xs).length"),
            ("fn f(xs: List[Int]) -> Int { xs.count() }", "(xs).length"),
            (
                "fn f(xs: List[Int], i: Int) -> Option[Int] { xs.get(i) }",
                "((__xs: readonly number[], __i: number) => __i >= 0 && __i < __xs.length ? Some(__xs[__i] as number) : None)(xs, i)",
            ),
            (
                "fn f(xs: List[Int], y: List[Int]) -> List[Int] { xs.concat(y).append(1).prepend(0) }",
                "[0, ...[...[...xs, ...y], 1]]",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.map((x) => x + 1).filter((x) => x > 1).take(2).skip(1) }",
                "((((xs).map((__x: number) => ((x: number) => x + 1)(__x))).filter((__x: number) => ((x: number) => x > 1)(__x))).slice(0, globalThis.Math.max(0, 2))).slice(globalThis.Math.max(0, 1))",
            ),
            (
                "fn f(xs: List[Int]) -> Bool { xs.any((x) => x > 1) && xs.all((x) => x > 0) }",
                "(xs).some((__x: number) => ((x: number) => x > 1)(__x)) && (xs).every((__x: number) => ((x: number) => x > 0)(__x))",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.flatMap((x) => [x, x]) }",
                "(xs).flatMap((__x: number) => ((x: number) => [x, x])(__x))",
            ),
            (
                "fn f(xs: List[Int]) -> Option[Int] { xs.first() }",
                "((__xs: readonly number[]) => __xs.length > 0 ? Some(__xs[0]) : None)(xs)",
            ),
            (
                "fn f(xs: List[Int]) -> Int { xs.firstOrElse(0) }",
                "((__xs: readonly number[], __d: number) => __xs.length > 0 ? __xs[0] : __d)(xs, 0)",
            ),
            (
                "fn f(xs: List[Int]) -> Int { xs.sum((x) => x) }",
                "(xs).reduce((__s: number, __x: number) => __s + ((x: number) => x)(__x), 0)",
            ),
            (
                "fn f(xs: List[Int]) -> List[Int] { xs.distinct() }",
                "[...new globalThis.Set(xs)]",
            ),
            (
                "fn f(xs: List[Bytes]) -> List[Bytes] { xs.distinct() }",
                "__bynkDistinct(xs)",
            ),
            (
                "fn f(xs: List[Float]) -> List[Float] { xs.sortBy((x) => x) }",
                "[...xs].sort((__a: number, __b: number) => __bynkCmpFloat(((x: number) => x)(__a), ((x: number) => x)(__b)))",
            ),
            (
                "fn f(xs: List[Float]) -> List[Float] { xs.sortByDescending((x) => x) }",
                "[...xs].sort((__a: number, __b: number) => __bynkCmpFloat(((x: number) => x)(__b), ((x: number) => x)(__a)))",
            ),
        ] {
            let (n, ts) = exits(body);
            assert_eq!(returned(&ts), want, "{body}");
            assert_eq!(n, 0, "{body}: text exits\n{ts}");
        }
    }

    /// The `Decimal` sum starts from the zero decimal and notes its helper.
    #[test]
    fn a_decimal_sum_is_a_node_and_imports_its_helper() {
        let (n, ts) = exits("fn f(xs: List[Decimal]) -> Decimal { xs.sum((x) => x) }");
        assert_eq!(n, 0, "{ts}");
        assert!(ts.contains("__bynkDecimalAdd(__s, ((x: { readonly coefficient: bigint; readonly scale: number }) => x)(__x)), { coefficient: 0n, scale: 0 })"), "{ts}");
        assert!(ts.contains("import { __bynkDecimalAdd }"), "{ts}");
    }

    /// A receiver that is still text (`Map.values()`) makes the arm fall back to
    /// today's one-line text, byte for byte: the mixed-operand path of
    /// `node_or_text`, which the node tests above never reach.
    #[test]
    fn a_text_receiver_keeps_the_text_form() {
        let values = "[...(m).values()]";
        for (elem, ret, expr, want) in [
            (
                "Int",
                "List[Int]",
                "m.values().map((x) => x)",
                format!("({values}).map((__x: number) => ((x: number) => x)(__x))"),
            ),
            (
                "Int",
                "List[Int]",
                "m.values().sortBy((x) => x)",
                format!(
                    "[...{values}].sort((__a: number, __b: number) => {{ const __ka = ((x: number) => x)(__a), __kb = ((x: number) => x)(__b); return __ka < __kb ? -1 : __ka > __kb ? 1 : 0; }})"
                ),
            ),
            (
                "Float",
                "List[Float]",
                "m.values().sortBy((x) => x)",
                format!(
                    "[...{values}].sort((__a: number, __b: number) => __bynkCmpFloat(((x: number) => x)(__a), ((x: number) => x)(__b)))"
                ),
            ),
            (
                "Decimal",
                "Decimal",
                "m.values().sum((x) => x)",
                format!(
                    "({values}).reduce((__s: {d}, __x: {d}) => __bynkDecimalAdd(__s, ((x: {d}) => x)(__x)), {{ coefficient: 0n, scale: 0 }})",
                    d = "{ readonly coefficient: bigint; readonly scale: number }"
                ),
            ),
            (
                "Int",
                "Int",
                "m.values().fold(0, (a, x) => a + x)",
                format!(
                    "((__xs: readonly number[], __acc: number, __f: (acc: number, x: number) => number) => {{ for (const __x of __xs) __acc = __f(__acc, __x); return __acc; }})({values}, 0, (a: number, x: number) => a + x)"
                ),
            ),
        ] {
            let (n, ts) = exits(&format!(
                "fn f(m: Map[String, {elem}]) -> {ret} {{ {expr} }}"
            ));
            assert!(n > 0, "{expr}\n{ts}");
            assert_eq!(returned_any(&ts), want, "{expr}");
        }
    }

    /// The statement-bodied arms (1A.18b, #3719), each over a list parameter
    /// (nodes) and over `Map.values()`, which is still text.
    /// `(element type, return type, extra parameters, expression over {R})`.
    const STATEMENT_ARMS: &[(&str, &str, &str, &str)] = &[
        ("Int", "Int", "", "{R}.fold(0, (a, x) => a + x)"),
        (
            "Int",
            "Effect[Int]",
            ", g: (Int, Int) -> Effect[Int]",
            "{R}.foldEff(0, g)",
        ),
        (
            "Int",
            "Effect[()]",
            ", g: (Int) -> Effect[()]",
            "{R}.forEach(g)",
        ),
        (
            "Int",
            "Effect[()]",
            ", g: (Int) -> Effect[()]",
            "{R}.parTraverse(g)",
        ),
        (
            "Int",
            "Effect[List[Result[Int, String]]]",
            ", g: (Int) -> Effect[Result[Int, String]]",
            "{R}.traverseAll(g)",
        ),
        (
            "Int",
            "Effect[Result[List[Int], String]]",
            ", g: (Int) -> Effect[Result[Int, String]]",
            "{R}.traverseTry(g)",
        ),
        (
            "Int",
            "Effect[Result[List[Int], String]]",
            ", g: (Int) -> Effect[Result[Int, String]]",
            "{R}.parTraverseTry(g)",
        ),
        (
            "Result[Int, String]",
            "Result[List[Int], List[String]]",
            "",
            "{R}.sequenceAll()",
        ),
        (
            "Result[Int, String]",
            "Result[List[Int], String]",
            "",
            "{R}.sequenceTry()",
        ),
        ("Int", "List[Int]", "", "{R}.sortBy((x) => x)"),
        ("Int", "List[Int]", "", "{R}.sortByDescending((x) => x)"),
        ("Int", "List[Int]", "", "{R}.distinctBy((x) => x)"),
        ("Int", "Option[Int]", "", "{R}.min((x) => x)"),
        ("Int", "Option[Int]", "", "{R}.max((x) => x)"),
        ("Float", "Option[Float]", "", "{R}.min((x) => x)"),
        ("Int", "Option[Float]", "", "{R}.average((x) => x)"),
        (
            "Int",
            "List[Int]",
            ", ys: List[Int]",
            "{R}.joinOn(ys, (x) => x, (y) => y, (x, y) => x + y)",
        ),
        (
            "Int",
            "List[Option[Int]]",
            ", ys: List[Int]",
            "{R}.leftJoin(ys, (x) => x, (y) => y, (x, y) => y)",
        ),
        (
            "Int",
            "List[Int]",
            ", ys: List[Int]",
            "{R}.join(ys, (x, y) => x == y, (x, y) => x + y)",
        ),
        (
            "Int",
            "List[Int]",
            "",
            "{R}.groupBy((x) => x, (k, rows) => rows.length())",
        ),
    ];

    /// The one returned expression of `f`, async or not.
    fn returned_any(ts: &str) -> String {
        let at = ts.find("function f(").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    fn arm_source(elem: &str, ret: &str, extra: &str, expr: &str, text_receiver: bool) -> String {
        let (param, recv) = if text_receiver {
            (format!("m: Map[String, {elem}]"), "m.values()")
        } else {
            (format!("xs: List[{elem}]"), "xs")
        };
        let body = expr.replace("{R}", recv);
        format!("fn f({param}{extra}) -> {ret} {{ {body} }}")
    }

    /// Every statement-bodied arm over node operands builds a node (no text
    /// exit), prints on several lines, and says exactly what the text fallback
    /// says apart from layout: the two forms agree once whitespace is removed.
    /// The one exception is the plain-key sorts, whose `const a = …, b = …;`
    /// splits into two statements.
    #[test]
    fn statement_arms_over_nodes_match_their_text_forms_modulo_layout() {
        let squash = |s: &str| s.chars().filter(|c| !c.is_whitespace()).collect::<String>();
        for (elem, ret, extra, expr) in STATEMENT_ARMS {
            let (n_node, node_ts) = exits(&arm_source(elem, ret, extra, expr, false));
            let (n_text, text_ts) = exits(&arm_source(elem, ret, extra, expr, true));
            assert_eq!(n_node, 0, "{expr}\n{node_ts}");
            assert!(n_text > 0, "{expr}\n{text_ts}");
            let node = returned_any(&node_ts);
            let text = returned_any(&text_ts).replace("[...(m).values()]", "xs");
            assert!(
                node.contains('\n'),
                "{expr} should print on several lines\n{node}"
            );
            assert!(
                !text.contains('\n'),
                "{expr} text fallback is one line\n{text}"
            );
            let node = squash(&node);
            let node = if expr.contains("sortBy") {
                node.replace(";const__kb=", ",__kb=")
            } else {
                node
            };
            assert_eq!(node, squash(&text), "{expr}");
        }
    }

    /// The layout of the statement arms: a block-bodied arrow, one statement per
    /// line, an unbraced loop body inline, `} else {` on one line, and the
    /// accumulator's function type keeps its parameter names.
    #[test]
    fn statement_arms_print_as_multi_line_arrows() {
        for (elem, ret, extra, expr, want) in [
            (
                "Int",
                "Effect[Int]",
                ", g: (Int, Int) -> Effect[Int]",
                "{R}.foldEff(0, g)",
                "(async (__xs: readonly number[], __acc: number, __f: (acc: number, x: number) => globalThis.Promise<number>) => {
    for (const __x of __xs) __acc = await __f(__acc, __x);
    return __acc;
  })(xs, 0, g)",
            ),
            (
                "Result[Int, String]",
                "Result[List[Int], List[String]]",
                "",
                "{R}.sequenceAll()",
                "((__xs: readonly Result<number, string>[]) => {
    const __ok: number[] = [];
    const __errs: string[] = [];
    for (const __r of __xs) {
      if (__r.tag === \"Err\") {
        __errs.push(__r.error);
      } else {
        __ok.push(__r.value);
      }
    }
    return __errs.length > 0 ? Err(__errs) : Ok(__ok);
  })(xs)",
            ),
            (
                "Int",
                "List[Int]",
                "",
                "{R}.sortByDescending((x) => x)",
                "[...xs].sort((__a: number, __b: number) => {
    const __ka = ((x: number) => x)(__a);
    const __kb = ((x: number) => x)(__b);
    return __ka < __kb ? 1 : __ka > __kb ? -1 : 0;
  })",
            ),
            (
                "Int",
                "List[Int]",
                ", ys: List[Int]",
                "{R}.joinOn(ys, (x) => x, (y) => y, (x, y) => x + y)",
                "(() => {
    const __h: globalThis.Record<string, number[]> = globalThis.Object.create(null);
    for (const __u of ys) {
      const __k = String(((y: number) => y)(__u));
      (__h[__k] = __h[__k] ?? []).push(__u);
    }
    return (xs).flatMap((__t: number) => {
      const __m = __h[String(((x: number) => x)(__t))] ?? [];
      return __m.map((__u: number) => ((x: number, y: number) => x + y)(__t, __u));
    });
  })()",
            ),
        ] {
            let (n, ts) = exits(&arm_source(elem, ret, extra, expr, false));
            assert_eq!(n, 0, "{ts}");
            assert_eq!(returned_any(&ts), want, "{expr}");
        }
    }

    /// The async arms stay async by construction and still import the settle
    /// helper that `par*` calls: `note_settle_all` fires as it did.
    #[test]
    fn par_arms_import_their_settle_helper() {
        let (n, ts) = exits(&arm_source(
            "Int",
            "Effect[()]",
            ", g: (Int) -> Effect[()]",
            "{R}.parTraverse(g)",
            false,
        ));
        assert_eq!(n, 0, "{ts}");
        assert!(ts.contains("__bynkSettleAll"), "{ts}");
        assert!(
            ts.contains("(async (__xs: readonly number[]) => {\n"),
            "{ts}"
        );
    }

    /// Hoisted effects keep source order: the receiver's `?` lowers before the
    /// argument's for `append`/`concat`, after it for `prepend`.
    #[test]
    fn hoisted_operands_keep_source_order() {
        let src = "commons lk\n\
            fn p(x: Int) -> Result[Int, String] { Ok(x) }\n\
            fn f(xs: List[Int]) -> Result[List[Int], String] { Ok(xs.append(p(1)?).concat([p(2)?])) }\n\
            fn g(xs: List[Int]) -> Result[List[Int], String] { Ok(xs.prepend(p(3)?)) }\n";
        let ts = emit_source(src);
        let at = |n: &str| ts.find(n).unwrap_or_else(|| panic!("{n}\n{ts}"));
        let f = at("export function f(");
        let one = ts[f..].find("p(1)").unwrap();
        let two = ts[f..].find("p(2)").unwrap();
        assert!(one < two, "{ts}");
        assert!(
            ts.contains("return Ok([...[...xs, __r0.value], ...[__r1.value]]);"),
            "{ts}"
        );
        assert!(ts.contains("return Ok([__r0.value, ...xs]);"), "{ts}");
    }

    /// Decision G: the imports derive from the printed output, so a module using
    /// `get`, `first`, `sequenceAll` and `traverseTry` imports what it did before.
    #[test]
    fn imports_follow_the_printed_kernel_output() {
        let src = "commons lk\n\
            fn a(xs: List[Int]) -> Option[Int] { xs.get(0) }\n\
            fn a2(xs: List[Int]) -> Option[Int] { xs.first() }\n\
            fn b(xs: List[Result[Int, String]]) -> Result[List[Int], List[String]] { xs.sequenceAll() }\n\
            fn c(xs: List[Int]) -> Effect[Result[List[Int], String]] {\n\
              xs.traverseTry((x) => { let r: Result[Int, String] = Ok(x)\n  Effect.pure(r) })\n\
            }\n";
        let ts = emit_source(src);
        assert!(
            ts.contains(
                "import { Ok, Err, Some, None, type Result, type Option } from \"./__bynk_runtime/result.js\";"
            ),
            "{ts}"
        );
        assert_eq!(ts.matches("import ").count(), 2, "{ts}");
    }
}
