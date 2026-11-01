//! Expression and statement lowering — the `LowerCtx`-driven engine that
//! turns Bynk expressions, statements, matches, and mocks into TypeScript
//! source. Split out of `emitter.rs` (ADR 0060); `LowerCtx` and the `ts_*`
//! type renderers stay in the parent and are reached via `use super::*`.

use std::collections::HashSet;
use std::sync::Arc;

use bynk_check::checker::{Callee, NamedKind, Ty, TyId, TypedCommons};
use bynk_syntax::ast::{
    BaseType, BinOp, Block, Expr, ExprKind, FaultsExpr, FieldInit, Ident, InterpPart, LambdaExpr,
    LiteralValue, MatchArm, MatchBody, ObservationExpr, ObservationMatcher, Pattern,
    PatternBindingKind, PredKind, Refinement, Statement, TypeBody, TypeDecl, TypeRef, UnaryOp,
};

use bynk_ir::{ConstVal, EventPatternIr, EventPatternValueIr, IrHttpMethod};
use bynk_lower::is_effectful_return;

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
            ModuleCtx::new(typed, cross_context, runtime_use),
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
    test_service_handlers: HashMap<String, Vec<bynk_ir::IrHandlerKind>>,
    sum_handlers: HashSet<(String, String)>,
    test_agents: HashSet<String>,
    settle_sends: bool,
    // #2024: the names the runner scaffold binds in the body's scope.
    harness_bound: HashSet<String>,
    source: &str,
    rel_path: &str,
    runtime_use: &RuntimeUse,
) -> (String, SourceMapBuilder) {
    let mut out = String::new();
    let smb = RefCell::new(SourceMapBuilder::new());
    {
        let mut cx = LowerCtx::new(
            ModuleCtx::new(typed, cross_context, runtime_use),
            BodyMode::TestCase {
                test: TestShared {
                    test_scaffold: true,
                    assert_loc: Some(crate::emitter::AssertLoc {
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
        let tail = pre.lower(&block.tail, &mut cx);
        for s in pre.stmts() {
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
    system_http_claims: crate::emitter::SystemHttpClaims,
    // #2136: the target's cron and queue services, driven through the Worker.
    system_jobs: crate::emitter::SystemJobServices,
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
        let mut module = ModuleCtx::new(typed, cross_context, runtime_use);
        module.target = BuildTarget::Workers;
        let mut cx = LowerCtx::new(
            module,
            BodyMode::IntegrationCase {
                test: TestShared {
                    test_scaffold: true,
                    assert_loc: Some(crate::emitter::AssertLoc {
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
        for s in pre.stmts() {
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
/// effectful statement (`<-`, `do`, `~>`, `:=`, `advance`) is `Safe` only
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
            bynk_ir::walk_exprs(e, &mut |x: &bynk_syntax::ast::Expr| {
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

pub(crate) fn emit_block_as_function_body(
    out: &mut String,
    block: &Block,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    // v0.154 (ADR 0178): **preserve** the enclosing `return_ty` rather than
    // resetting it. Nested control-flow blocks (an `if`/`else` or `match` arm
    // *within* a function body) reach here, and a `return` from them returns
    // from the whole function — so an embedding `?` inside them must still see
    // the function's return type. Only a genuine function/handler boundary
    // (`_with_return`) rebinds it. A top-level body with no embedding context
    // (a mock/test that never called `_with_return`) simply inherits the
    // ambient `None`.
    emit_block_inner(out, block, cx, indent, async_tail);
}

/// As [`emit_block_as_function_body`], but records the enclosing return type on
/// the `LowerCtx` for the duration of the body so the `?` lowering can apply a
/// declared error embedding (v0.154, ADR 0178). The return type is set and then
/// restored, so a *sibling* body (a lambda, a mock) does not inherit the wrong
/// one; nested control-flow blocks reached via the shim above keep it.
pub(crate) fn emit_block_as_function_body_with_return(
    out: &mut String,
    block: &Block,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
    return_type: Option<&TypeRef>,
) {
    let tys = cx.commons().tys();
    let prev = cx.return_ty.take();
    cx.return_ty = return_type
        .and_then(|rt| bynk_check::checker::resolve_type_ref(rt, &cx.commons().types, tys));
    // #1750: a function body's tails `return`, whatever sink encloses it.
    let prev_slot = cx.tail_slot.take();
    emit_block_inner(out, block, cx, indent, async_tail);
    cx.tail_slot = prev_slot;
    cx.return_ty = prev;
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
    let tys = cx.commons().tys();
    use bynk_check::checker::Ty;
    let operand_node = operand_ty.map(|t| tys.get(t));
    let Some(Ty::Result(_, e)) = operand_node.as_deref() else {
        return None;
    };
    let f_err = peel_result_err(cx.return_ty?, tys)?;
    // An exact/compatible match propagates as-is; only a genuine mismatch that a
    // declared embedding resolves gets wrapped.
    if bynk_check::checker::compatible(*e, f_err, tys) {
        return None;
    }
    bynk_check::checker::embedding_for(f_err, *e, &cx.commons().types, tys)
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

fn emit_block_inner(
    out: &mut String,
    block: &Block,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    // #908: every nested block (if/else branch, match arm, lambda body) as
    // well as a function/handler's own body reaches here, so this is the one
    // place that needs to push/pop a shadow-rename frame to match the
    // language's actual block-scoping — a re-`let` frame must not survive
    // past the block it was declared in.
    cx.shadow_scopes.push(HashMap::new());
    for stmt in &block.statements {
        emit_statement(out, stmt, cx, indent);
    }
    // Tail position: match → inline switch, if → inline if, otherwise return expr.
    // Anchor the tail's generated lines to the tail expression's span (slice 1).
    cx.record_span(out.len(), block.tail.span);
    match &block.tail.kind {
        ExprKind::Match { discriminant, arms } => {
            emit_match_tail(out, discriminant, arms, cx, indent, async_tail);
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } if ternary_shaped(cond, then_block, else_block, cx) => {
            // T2.1 (R6.2): the shape *could* be a ternary. Whether it actually
            // is depends on whether either branch hoists — which is now
            // answered by lowering it, not by predicting it. Both outcomes are
            // built from the same `Lowered` values, so nothing is lowered twice
            // and no temp number is burned on a discarded attempt.
            let mut pre = Pre::new();
            let cond_expr = pre.lower(cond, cx);
            let then_tail = lower_tail_expr(&then_block.tail, cx, async_tail);
            let else_tail = lower_tail_expr(&else_block.tail, cx, async_tail);
            for s in pre.stmts() {
                write_line(out, indent, s);
            }
            if then_tail.pre.is_empty() && else_tail.pre.is_empty() {
                write_line(
                    out,
                    indent,
                    &tail_stmt(
                        cx,
                        &format!(
                            "({cond_expr} ? {t} : {e})",
                            t = then_tail.expr,
                            e = else_tail.expr
                        ),
                    ),
                );
            } else {
                // A branch hoisted after all — the case `simple_expr` used to
                // mispredict (#945, and the `ListLit`/`InterpStr` fallthrough
                // this slice closes). A statement `if` is where a hoisted `?`'s
                // early return belongs, so emit that instead of a ternary that
                // has nowhere to put it. Byte-identical to what `emit_if_tail`
                // produces for this shape (both branches are statement-free and
                // their tails lower as plain expressions), rebuilt from the
                // values already in hand.
                write_line(out, indent, &format!("if ({cond_expr}) {{"));
                emit_pure_tail_branch(out, then_tail, cx, indent + INDENT_STEP);
                write_line(out, indent, "} else {");
                emit_pure_tail_branch(out, else_tail, cx, indent + INDENT_STEP);
                write_line(out, indent, "}");
            }
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => {
            emit_if_tail(out, cond, then_block, else_block, cx, indent, async_tail);
        }
        _ => {
            let tail = lower_tail_expr(&block.tail, cx, async_tail);
            for s in &tail.pre {
                write_line(out, indent, s);
            }
            write_line(out, indent, &tail_stmt(cx, &tail.expr));
        }
    }
    cx.shadow_scopes.pop();
}

/// #1750: the statement that hands a block's tail value to its sink: `return
/// value;` in a function, lambda or arrow body, or, inside a statement-form
/// value expression ([`hoist_value_as_statement`]), the assignment to its slot
/// followed by a `break` out of its labelled block.
fn tail_stmt(cx: &LowerCtx, value: &str) -> String {
    match &cx.tail_slot {
        None => format!("return {value};"),
        Some(TailSlot { slot, label }) => format!("{slot} = {value}; break {label};"),
    }
}

/// Write one branch of the statement `if` above: the tail's hoisted statements
/// followed by its `return` (or, #1750, its sink's [`tail_stmt`]). Exactly the
/// body [`emit_block_inner`]'s own `_` arm writes for a statement-free block,
/// which is what `emit_if_tail` would have recursed into.
fn emit_pure_tail_branch(out: &mut String, tail: Lowered, cx: &LowerCtx, indent: usize) {
    for s in &tail.pre {
        write_line(out, indent, s);
    }
    write_line(out, indent, &tail_stmt(cx, &tail.expr));
}

/// Lower an expression that's in the tail position of a returning context.
///
/// In async-tail position (v0.7.1), an `async function` wraps its return value
/// as a Promise automatically, so `Effect.pure(...)` is redundant and should
/// emit as a bare value. Recurse through control-flow forms whose result is
/// the surrounding function's return value:
/// - `Effect.pure(x)` → lower `x` directly.
/// - A ternary-form `if`/`else` (simple branches) where each branch's tail is
///   itself an async-tail position.
/// - A pure-tail block (no statements) where the inner tail is the actual
///   returned expression.
/// - Parens (transparent).
///
/// In non-async-tail position, defer to [`lower_expr`] unchanged.
fn lower_tail_expr(e: &Expr, cx: &mut LowerCtx, async_tail: bool) -> Lowered {
    if !async_tail {
        return lower_expr(e, cx);
    }
    match &e.kind {
        ExprKind::EffectPure(inner) => lower_expr(inner, cx),
        ExprKind::Paren(inner) => lower_tail_expr(inner, cx, true),
        ExprKind::Block(b) if b.statements.is_empty() => lower_tail_expr(&b.tail, cx, true),
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } if ternary_shaped(cond, then_block, else_block, cx) => {
            let mut pre = Pre::new();
            let cond_expr = pre.lower(cond, cx);
            let then_tail = lower_tail_expr(&then_block.tail, cx, true);
            let else_tail = lower_tail_expr(&else_block.tail, cx, true);
            if then_tail.pre.is_empty() && else_tail.pre.is_empty() {
                return pre.finish(format!(
                    "({cond_expr} ? {t} : {e})",
                    t = then_tail.expr,
                    e = else_tail.expr
                ));
            }
            let slot_ty = async_tail_slot_ty(e, cx);
            let value =
                hoist_if_as_statement(&mut pre, cond_expr, then_tail, else_tail, slot_ty, cx);
            pre.finish(value)
        }
        _ => lower_expr(e, cx),
    }
}

/// The slot annotation for a hoisted `if` in **async-tail** position, where
/// `checked_ty_ts` alone is wrong.
///
/// #1029 review: this function exists because an `async function` wraps its
/// return value, so `Effect.pure(x)` in tail position emits a bare `x` rather
/// than a promise. A branch therefore assigns `T` where the `if`'s recorded type
/// is `Effect[T]` — and `ts_ty` would render that `Promise<T>`, which `tsc
/// --strict` rejects (`Type 'boolean' is not assignable to type
/// 'Promise<boolean>'`). The ternary form never had to care: `return (c ? x : p)`
/// in an `async function` legally accepts either.
///
/// Both shapes are genuinely reachable — an `Effect.pure(x)` branch yields `T`
/// (the `EffectPure` arm above unwraps it) while a branch that is merely
/// `Effect`-typed falls through to `lower_expr` and yields `Promise<T>` — so the
/// annotation admits both rather than picking one.
fn async_tail_slot_ty(e: &Expr, cx: &LowerCtx) -> Option<String> {
    let tys = cx.commons().tys();
    match cx.commons().expr_ty(e.id).as_deref() {
        Some(Ty::Effect(inner)) => {
            let inner_ts = match &*tys.get(*inner) {
                Ty::Unit => "void".to_string(),
                _ => ts_ty(*inner, tys),
            };
            Some(format!("{inner_ts} | globalThis.Promise<{inner_ts}>"))
        }
        _ => checked_ty_ts(e, cx),
    }
}

/// T2.1: a branch of a would-be ternary hoisted, so the ternary has nowhere to
/// put its statements. Hoist the whole `if` as a real statement assigning to a
/// fresh slot, and hand back the slot as the expression.
///
/// The reason this matters is the one R6.6 names: an arrow wrapper
/// (`(() => { … })()`, the only shape available before `pre` could travel with
/// the expression) makes a hoisted `?`'s `return` exit the *arrow* rather than
/// the enclosing function — the `lower_match_as_iife` miscompile (#946) in a
/// different costume, which `tsc --strict` rejects outright because the arrow's
/// type becomes `T | { tag: "Err" }`. Hoisting keeps the `return` where the
/// source put it.
///
/// Decision B of #1017 asked whether threading closes this for free. For the
/// `if`-expression it does, and this is where. It does **not** close
/// `lower_bin_op`'s short-circuit `rhs`, whose hoist must be *skipped* when the
/// operator doesn't reach it — a statement cannot be conditionally skipped, so
/// the arrow there is load-bearing rather than incidental. That one stays open.
fn hoist_if_as_statement(
    pre: &mut Pre,
    cond_expr: String,
    then_tail: Lowered,
    else_tail: Lowered,
    slot_ty: Option<String>,
    cx: &mut LowerCtx,
) -> String {
    let slot = cx.fresh();
    // The slot needs its type spelled out. TypeScript's "evolving `let`"
    // inference gives up once a branch also `return`s (TS7034/TS7005 under
    // `--strict`), which is exactly the shape a hoisted `?` produces — so take
    // the type the checker already recorded for the `if` expression.
    match &slot_ty {
        Some(ty) => pre.push(format!("let {slot}: {ty};")),
        None => pre.push(format!("let {slot};")),
    }
    pre.push(format!("if ({cond_expr}) {{"));
    pre.extend(assign_branch(then_tail, &slot));
    pre.push("} else {".to_string());
    pre.extend(assign_branch(else_tail, &slot));
    pre.push("}".to_string());
    slot
}

/// #1750: emit a value-position `if`, `match` or block (`e`) as a real
/// statement, for when its branches hold a `?`. The arrow (IIFE) these forms
/// otherwise lower to would capture the `?`'s `return`, so its `Err` became the
/// expression's value instead of leaving the function: the same miscompile
/// [`hoist_if_as_statement`] closes for a ternary-shaped `if`, which has only
/// branch tails to place. Here `emit` writes the form through its ordinary
/// statement-position emitter (`emit_if_tail`, `emit_match_tail`,
/// `emit_block_as_function_body`) with [`LowerCtx::tail_slot`] set, so every
/// tail, however deeply nested, assigns the slot and breaks out of the
/// labelled block:
///
/// ```text
/// let __r3: T;
/// __r3_out: {
///   if (cond) { …; __r3 = a; break __r3_out; } else { __r3 = b; break __r3_out; }
/// }
/// ```
///
/// The label is what makes the `break` reach past a `switch` (whose cases
/// would otherwise fall through to its trailing `throw`) and past nested
/// statement forms. `return_ty` is deliberately left set: the `?` now returns
/// from the enclosing function, so ADR 0178's embedding conversion applies
/// (#1750 corrects that ADR's "an IIFE clears it", which described the
/// miscompile). An `await` inside lands in the enclosing function's own body,
/// which is `async` whenever its body can `await`.
fn hoist_value_as_statement(
    e: &Expr,
    cx: &mut LowerCtx,
    emit: impl FnOnce(&mut String, &mut LowerCtx),
) -> Lowered {
    let slot = cx.fresh();
    let label = slot.clone() + "_out";
    let mut pre = Pre::new();
    // Typed for the same reason as `hoist_if_as_statement`'s slot: inference
    // gives up on a `let` whose assignments sit beside a `return`.
    let annotation = checked_ty_ts(e, cx)
        .map(|ty| ": ".to_string() + &ty)
        .unwrap_or_default();
    pre.push(format!("let {slot}{annotation};"));
    pre.push(label.clone() + ": {");
    let saved = cx.tail_slot.replace(TailSlot {
        slot: slot.clone(),
        label,
    });
    // The body is a local buffer spliced into `pre` — see `without_source_map`.
    let mut body = String::new();
    cx.without_source_map(|cx| emit(&mut body, cx));
    cx.tail_slot = saved;
    pre.extend(body.lines().map(str::to_string));
    pre.push("}".to_string());
    pre.finish(slot)
}

/// #1750: whether a `?` in `b` would lower to a `return` inside `b`'s own
/// emitted body. Structural, like [`ternary_shaped`], so the route is chosen
/// before anything is lowered. A lambda is its own return scope, so a `?`
/// inside one doesn't count.
fn block_contains_question(b: &Block) -> bool {
    let mut stmt_exprs = Vec::new();
    for s in &b.statements {
        bynk_syntax::ast::statement_exprs(s, &mut stmt_exprs);
    }
    stmt_exprs.into_iter().any(expr_contains_question) || expr_contains_question(&b.tail)
}

/// [`block_contains_question`] over a `match`'s arms, guards included: a guard
/// is evaluated inside the arrow too.
fn arms_contain_question(arms: &[MatchArm]) -> bool {
    arms.iter().any(|a| {
        a.guard.as_ref().is_some_and(expr_contains_question)
            || match &a.body {
                MatchBody::Expr(e) => expr_contains_question(e),
                MatchBody::Block(b) => block_contains_question(b),
            }
    })
}

fn expr_contains_question(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Question(_) => true,
        ExprKind::Lambda(_) => false,
        _ => bynk_syntax::ast::expr_children(e)
            .into_iter()
            .any(expr_contains_question),
    }
}

/// One branch of the hoisted statement `if` above: the branch's own hoisted
/// statements, then the assignment of its value to the shared slot. Each line
/// carries its own indent — a hoisted statement is written by whichever caller
/// owns the statement position, at one flat indent, so nesting has to travel
/// inside the string.
fn assign_branch(tail: Lowered, slot: &str) -> Vec<String> {
    let pad = " ".repeat(INDENT_STEP);
    let mut out: Vec<String> = tail.pre.iter().map(|s| format!("{pad}{s}")).collect();
    out.push(format!("{pad}{slot} = {};", tail.expr));
    out
}

/// The TypeScript rendering of the type the checker recorded for `e`, if any.
/// Used to annotate the slot [`hoist_if_as_statement`] declares.
fn checked_ty_ts(e: &Expr, cx: &LowerCtx) -> Option<String> {
    cx.commons()
        .expr_ty(e.id)
        .map(|_| ts_ty(cx.commons().expr_types[&e.id].ty, cx.commons().tys()))
}

/// T2.1 (R6.2): the ternary form's **structural** precondition — everything
/// about the shape that must hold before any lowering is attempted. Both
/// branches must be pure tails (a ternary has nowhere to host a statement)
/// whose own tails lower as plain expressions, and the condition must introduce
/// no `is` bindings (likewise nowhere to declare them).
///
/// The question this deliberately does *not* ask is the one `simple_expr` used
/// to guess: "will lowering this branch hoist a statement?" That is now
/// answered after the fact by [`Lowered::pre`], which cannot be wrong. The
/// predictive classifier — and the `debug_assert!(stmts.is_empty())` pair that
/// stood in for the guarantee it could not make — is deleted; the five defects
/// it produced (#945–#948 plus the `ListLit`/`InterpStr` fallthrough this slice
/// found live) are unrepresentable rather than merely fixed.
///
/// What remains here is only *routing*: which emitter shape a branch would take
/// if it were emitted as a block. That is a structural property of the AST, not
/// a prediction about lowering, and it never guessed wrong — every one of the
/// defects was hoisting, none was routing.
fn ternary_shaped(cond: &Expr, then_block: &Block, else_block: &Block, cx: &LowerCtx) -> bool {
    then_block.statements.is_empty()
        && else_block.statements.is_empty()
        && tail_lowers_as_expression(&then_block.tail, cx)
        && tail_lowers_as_expression(&else_block.tail, cx)
        && !cond_has_is_bindings(cond, cx)
}

/// True when [`emit_block_inner`] would lower this tail through its plain-
/// expression arm rather than routing it to a statement shape of its own. A
/// ternary branch — and the branch rebuild both hoisting fallbacks perform —
/// can host an expression and its hoisted statements, but not an emitted
/// `switch` or `if`/`else`.
fn tail_lowers_as_expression(e: &Expr, cx: &LowerCtx) -> bool {
    match &e.kind {
        ExprKind::Match { .. } => false,
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => ternary_shaped(cond, then_block, else_block, cx),
        _ => true,
    }
}

/// Lower the awaited expression of a `let x <- e`, with the call-site
/// `by <Actor>(<identity>)` clause it carries folded into the addressed call.
fn lower_effect_let_value(
    l: &bynk_syntax::ast::LetStmt,
    pre: &mut Pre,
    cx: &mut LowerCtx,
) -> String {
    // v0.182 (#664): a call-site `by <Actor>(<identity>)` supplies the
    // identity the addressed handler reads as `deps.identity`. Lower it
    // (a test-only brand cast, like an agent key) and stash it for the
    // address-call lowering to fold into that call's deps.
    let saved_identity = cx.call_site_identity.take();
    let saved_claims = cx.call_site_claims.take();
    let saved_actor = cx.call_site_actor.take();
    let saved_no_credential = cx.call_site_no_credential;
    cx.call_site_no_credential = false;
    let saved_bad_signature = cx.call_site_bad_signature.take();
    if let Some(principal) = &l.principal {
        // #706: `by Nobody` drives the route with no credential (the 401
        // path); it names no identity. Any other principal supplies its
        // identity — the *raw* lowered value (`"bob"`), which unit deps
        // folds with an `as any` brand cast and the system driver passes
        // as the JWT `sub` verbatim.
        if principal.actor.name == "Nobody" {
            cx.call_site_no_credential = true;
        } else if let Some(variant) = bad_signature_driver(&principal.actor.name) {
            // #2135: `by Forged` / `by Stale` name no identity either.
            cx.call_site_bad_signature = Some(variant);
        } else {
            cx.call_site_identity = principal.identity.as_ref().map(|id| pre.lower(id, cx));
            cx.call_site_claims = principal.claims.as_ref().map(|c| pre.lower(c, cx));
            cx.call_site_actor = Some(principal.actor.name.clone());
        }
    }
    let value = pre.lower(&l.value, cx);
    cx.call_site_identity = saved_identity;
    cx.call_site_claims = saved_claims;
    cx.call_site_actor = saved_actor;
    cx.call_site_no_credential = saved_no_credential;
    cx.call_site_bad_signature = saved_bad_signature;
    value
}

/// #2640: `par { let a <- e1  let b <- e2 }` →
/// `const [a, b] = await __bynkSettleAll([e1, e2]);`.
///
/// The array literal evaluates `e1` then `e2`, so each effect is issued, in
/// source order, before either is awaited; the join waits for all of them and
/// raises the first rejection in source order (see `settle.ts`). A `do e`
/// binding takes a hole in the pattern. Should a binding's value hoist
/// statements, those run before the array and so ahead of the earlier bindings'
/// effects; to keep issue order, every effect is then started into a temporary
/// in turn and the temporaries are joined. Every effect is still awaited, so
/// ADR 0426's guarantee holds.
fn emit_par(out: &mut String, p: &bynk_syntax::ast::ParStmt, cx: &mut LowerCtx, indent: usize) {
    let mut values: Vec<(Pre, String)> = Vec::new();
    // `None` for a `do`, which binds nothing; the name (or `_`) for a `let`.
    let mut names: Vec<Option<&bynk_syntax::ast::Ident>> = Vec::new();
    for st in &p.statements {
        let mut pre = Pre::new();
        match st {
            Statement::EffectLet(l) => {
                let value = lower_effect_let_value(l, &mut pre, cx);
                names.push(Some(&l.name));
                values.push((pre, value));
            }
            Statement::Do(d) => {
                let value = pre.lower(&d.value, cx);
                names.push(None);
                values.push((pre, value));
            }
            // The checker refuses every other statement in a `par` block.
            _ => {}
        }
    }
    let hoisted = values.iter().any(|(pre, _)| !pre.stmts().is_empty());
    let mut items: Vec<String> = Vec::new();
    for (pre, value) in values {
        for s in pre.stmts() {
            write_line(out, indent, s);
        }
        if hoisted {
            let started = cx.fresh();
            write_line(out, indent, &format!("const {started} = {value};"));
            items.push(started);
        } else {
            items.push(value);
        }
    }
    // The names are bound only after every value is lowered, so a binding that
    // reads an outer name its own `let` re-binds still sees the outer one.
    let mut pattern: Vec<String> = names
        .iter()
        .map(|n| n.map(|id| cx.bind_local_name(&id.name)).unwrap_or_default())
        .collect();
    while pattern.last().is_some_and(String::is_empty) {
        pattern.pop();
    }
    cx.note_settle_all();
    // T2.2 (R6.4): see the matching note at `EffectLet`.
    cx.emitted_await = true;
    let join = format!("await __bynkSettleAll([{}]);", items.join(", "));
    if pattern.is_empty() {
        write_line(out, indent, &join);
    } else {
        write_line(
            out,
            indent,
            &format!("const [{}] = {join}", pattern.join(", ")),
        );
    }
}

fn emit_statement(out: &mut String, stmt: &Statement, cx: &mut LowerCtx, indent: usize) {
    // Slice 1 (ADR 0103 D2): every generated line this statement emits — including
    // the multi-line `?` expansion (temp / Err-guard / unwrap) — anchors to the
    // statement's source span, so a source-map-aware stepper coalesces them into
    // one source step (the slice-0 spike confirmed this).
    cx.record_span(out.len(), stmt.span());
    match stmt {
        Statement::Let(l) => {
            // Track `let x = AgentName(key)` so subsequent `x.method(args)`
            // calls can dispatch through the agent class.
            if l.name.name != "_"
                && let ExprKind::Call {
                    name,
                    args: ctor_args,
                    ..
                } = &l.value.kind
                && cx.local_agents.contains(&name.name)
                && ctor_args.len() == 1
            {
                cx.local_agent_vars
                    .insert(l.name.name.clone(), name.name.clone());
            }
            let mut pre = Pre::new();
            let value = pre.lower(&l.value, cx);
            for s in pre.stmts() {
                write_line(out, indent, s);
            }
            // #908: also covers `_` (a fresh throwaway local; TS allows
            // `const _ = ...` only once per scope) and a re-`let` of a name
            // already bound in this or an enclosing block (which otherwise
            // either collides with the first `const` of that name — TS2451 —
            // or, one block deeper, shadows it into a TDZ read).
            let bind_name = cx.bind_local_name(&l.name.name);
            match &l.type_annot {
                Some(annot) => write_line(
                    out,
                    indent,
                    &format!(
                        "const {bind_name}: {ty} = {value};",
                        ty = ts_type_ref(annot),
                    ),
                ),
                None => write_line(out, indent, &format!("const {bind_name} = {value};")),
            }
        }
        Statement::EffectLet(l) => {
            // `let x <- expr` → `const x = await expr;`
            let mut pre = Pre::new();
            let value = lower_effect_let_value(l, &mut pre, cx);
            for s in pre.stmts() {
                write_line(out, indent, s);
            }
            let bind_name = cx.bind_local_name(&l.name.name);
            // T2.2 (R6.4): the literal `await` below is one of the two real
            // sources of effectfulness a synchronous-looking IIFE further out
            // needs to know about — see `emitted_await`.
            cx.emitted_await = true;
            match &l.type_annot {
                Some(annot) => write_line(
                    out,
                    indent,
                    &format!(
                        "const {bind_name}: {ty} = await {value};",
                        ty = ts_type_ref(annot),
                    ),
                ),
                None => write_line(out, indent, &format!("const {bind_name} = await {value};")),
            }
        }
        Statement::Expect(a) => {
            // Inside a test case body, `expect <pred>` lowers to a runtime check
            // that throws an ExpectationError so the surrounding test-case runner
            // catches it and records the failure (v0.112, renamed from `assert`).
            let span_start = a.value.span.start;
            let span_end = a.value.span.end;
            let location = expect_location(cx, span_start);
            let src = expect_source_text(cx, a.value.span);
            let mut pre = Pre::new();
            let cond = pre.lower(&a.value, cx);
            // Structural expected-vs-actual for a top-level comparison. The
            // predicate is pure (ADR 0144), so re-evaluating the operands for the
            // failure message is observationally identical to the condition.
            let detail = if let ExprKind::BinOp(op, l, r) = &a.value.kind
                && let Some(sym) = comparison_op_symbol(*op)
            {
                let lv = pre.lower(l, cx);
                let rv = pre.lower(r, cx);
                format!(
                    "\"expect {src}\\n  expected: {src}\\n  actual:   \" + __bynkShow(({lv})) + \" {sym} \" + __bynkShow(({rv}))"
                )
            } else if matches!(a.value.kind, ExprKind::Faults(_)) {
                // #1706: the claim's only failure is a call that returned.
                format!("\"expect {src}\\n  the call returned without faulting\"")
            } else {
                format!("\"expect {src}\"")
            };
            for s in pre.stmts() {
                write_line(out, indent, s);
            }
            write_line(
                out,
                indent,
                &format!(
                    "if (!({cond})) {{ throw __bynkExpectFailure(\"{location}\", {span_start}, {span_end}, {detail}); }}",
                ),
            );
        }
        Statement::Advance(a) => {
            // #2137: move the case's harness clock (`emitter/test_runtime/clock.ts`),
            // which every participant's `Clock` reads at `system` and the
            // `Clock` double reads below it. A negative amount faults there.
            let mut pre = Pre::new();
            let amount = pre.lower(&a.amount, cx);
            for s in pre.stmts() {
                write_line(out, indent, s);
            }
            write_line(out, indent, &format!("__bynkAdvanceClock({amount});"));
        }
        Statement::Send(s) => {
            // v0.79: `~> expr` — fire-and-forget. The reply is `Effect[()]` and is
            // never awaited. On the Workers target the immediate tier hands the
            // promise to the execution context's `waitUntil`, so it settles after
            // the handler returns rather than being killed with the response. The
            // execution context rides in `deps.__exec` (threaded by `compose`).
            let mut pre = Pre::new();
            let value = pre.lower(&s.value, cx);
            for st in pre.stmts() {
                write_line(out, indent, st);
            }
            write_line(
                out,
                indent,
                &format!(
                    "{deps}.__exec.waitUntil({value});",
                    deps = cx.cap_deps_expr()
                ),
            );
        }
        Statement::Do(d) => {
            // v0.146 (ADR 0170): `do expr` → `await expr;`. The binder-free
            // `let _ <- expr` for a unit effect — the effect runs and joins the
            // handler, its `()` result discarded (no `const`).
            let mut pre = Pre::new();
            let value = pre.lower(&d.value, cx);
            for st in pre.stmts() {
                write_line(out, indent, st);
            }
            // T2.2 (R6.4): see the matching note at `EffectLet`.
            cx.emitted_await = true;
            write_line(out, indent, &format!("await {value};"));
        }
        Statement::Par(p) => emit_par(out, p, cx, indent),
        Statement::Assign(a) => {
            // v0.81 (storage track, ADR 0109): `cell := expr` writes the mutable
            // working state in place (`__state.cell = <expr>`). It is staged in
            // memory — read-your-writes within the handler — and flushed once at
            // handler end via `commitState` (which runs the invariant gate before
            // the durable write). A fault before that flush persists nothing.
            let mut pre = Pre::new();
            let value = pre.lower(&a.value, cx);
            for st in pre.stmts() {
                write_line(out, indent, st);
            }
            let lhs = match cx.agent_store_cells() {
                Some((var, _)) => format!("{var}.{}", a.target.name),
                // Defensive: the checker resolves `:=` to a store cell, so a
                // write outside a store-agent handler does not reach emission.
                None => a.target.name.clone(),
            };
            write_line(out, indent, &format!("{lhs} = {value};"));
        }
    }
}

/// v0.59: the `location` string an `expect` failure carries. With a test-body
/// [`AssertLoc`](crate::emitter::AssertLoc) in scope it is a real, escaped
/// `path:line:col` (so `--format json` consumers can link to the source);
/// otherwise it falls back to the bare byte offset (`expect`s only appear in test
/// bodies, so the fallback is defensive).
fn expect_location(cx: &LowerCtx, offset: usize) -> String {
    match cx.assert_loc() {
        Some(loc) => {
            let (line, col) = bynk_syntax::span::line_col(&loc.source, offset);
            // Normalise to forward slashes so the location is identical on
            // Windows (where `PathBuf` joins with `\`) — matching the
            // diagnostic path rendering and the committed goldens.
            let path = loc.rel_path.replace('\\', "/");
            crate::emitter::escape_ts_string(&format!("{path}:{line}:{col}"))
        }
        None => format!("offset {offset}"),
    }
}

/// v0.112: the trimmed, TS-escaped source text of an `expect` predicate, embedded
/// in the structural failure report (`expect <src>`). Escaped for a
/// double-quoted TS string literal (no surrounding quotes); empty when no
/// test-body source is in scope (defensive — `expect`s only appear in test bodies).
fn expect_source_text(cx: &LowerCtx, span: bynk_syntax::span::Span) -> String {
    match cx.assert_loc() {
        Some(loc) => {
            let raw = loc.source.get(span.start..span.end).unwrap_or("").trim();
            crate::emitter::escape_ts_string(raw)
        }
        None => String::new(),
    }
}

/// v0.112: the source operator symbol for a comparison `BinOp`, or `None` for
/// non-comparison operators (which get the source-text-only failure report).
fn comparison_op_symbol(op: BinOp) -> Option<&'static str> {
    match op {
        BinOp::Eq => Some("=="),
        BinOp::NotEq => Some("!="),
        BinOp::Lt => Some("<"),
        BinOp::LtEq => Some("<="),
        BinOp::Gt => Some(">"),
        BinOp::GtEq => Some(">="),
        _ => None,
    }
}

fn write_line(out: &mut String, indent: usize, line: &str) {
    for _ in 0..indent {
        out.push(' ');
    }
    out.push_str(line);
    out.push('\n');
}

/// The raw TypeScript form of a compile-time literal that v0.9.4 may admit as a
/// refined type (int or string). `None` for anything else.
fn lower_const_literal_raw(e: &Expr) -> Option<String> {
    match &e.kind {
        ExprKind::IntLit { value: n, .. } => Some(n.to_string()),
        // v0.21: the stored lexeme verbatim.
        ExprKind::FloatLit { lexeme, .. } => Some(lexeme.clone()),
        ExprKind::StrLit(s) => Some(format!("\"{}\"", escape_ts_string(s))),
        // A negated numeric literal — admissible at compile time (the
        // checker folds the sign in `const_literal`), so the lowering must
        // route it through `unsafe` like any other admitted literal.
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => match &inner.kind {
            ExprKind::IntLit { value: n, .. } => Some(format!("-{n}")),
            ExprKind::FloatLit { lexeme, .. } => Some(format!("-{lexeme}")),
            _ => None,
        },
        _ => None,
    }
}

/// Lower an interpolated string (v0.43, ADR 0075) to a TS template literal.
/// Chunks become escaped literal text; each hole becomes `${String(<expr>)}`.
/// `String(…)` is identity for a `String` hole and the display form for
/// `Int`/`Float`/`Bool` — and the checker guarantees only base scalars reach
/// here, so no `[object Object]` can be emitted.
fn lower_interp_str(parts: &[InterpPart], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    let mut out = String::from("`");
    for part in parts {
        match part {
            InterpPart::Chunk(text) => out.push_str(&escape_ts_template(text)),
            InterpPart::Hole(hole) => {
                let lowered = pre.lower(hole, cx);
                out.push_str(&format!("${{String({lowered})}}"));
            }
        }
    }
    out.push('`');
    pre.finish(out)
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
    pub pre: Vec<String>,
    pub expr: String,
}

impl Lowered {
    /// An expression that hoists nothing — the common leaf case.
    pub(crate) fn bare(expr: impl Into<String>) -> Self {
        Lowered {
            pre: Vec::new(),
            expr: expr.into(),
        }
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
pub(crate) struct Pre(Vec<String>);

impl Pre {
    pub(crate) fn new() -> Self {
        Pre(Vec::new())
    }

    /// Lower `e`, absorbing whatever it hoists, and hand back its text. The
    /// replacement for `lower_expr_into(e, stmts, cx)`: identical ordering —
    /// the callee's statements land here, in order, at the point of the call —
    /// but the callee never saw the buffer.
    pub(crate) fn lower(&mut self, e: &Expr, cx: &mut LowerCtx) -> String {
        let lowered = lower_expr(e, cx);
        self.absorb(lowered)
    }

    /// Absorb an already-built [`Lowered`] and hand back its text.
    pub(crate) fn absorb(&mut self, lowered: Lowered) -> String {
        self.0.extend(lowered.pre);
        lowered.expr
    }

    /// Absorb an optional [`Lowered`] — a kernel lowerer that may decline the
    /// shape. Declining hoists nothing, so `None` leaves `self` untouched.
    pub(crate) fn absorb_opt(&mut self, lowered: Option<Lowered>) -> Option<String> {
        lowered.map(|l| self.absorb(l))
    }

    /// Hoist a statement this function generated itself (a `?` temp, an
    /// `is`-receiver binding, a match IIFE's scaffolding).
    pub(crate) fn push(&mut self, stmt: String) {
        self.0.push(stmt);
    }

    pub(crate) fn extend(&mut self, stmts: impl IntoIterator<Item = String>) {
        self.0.extend(stmts);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The accumulated statements, for a caller that is writing them into a
    /// real statement position rather than hoisting them further.
    pub(crate) fn stmts(&self) -> &[String] {
        &self.0
    }

    /// Pair the accumulated statements with the expression they precede.
    pub(crate) fn finish(self, expr: impl Into<String>) -> Lowered {
        Lowered {
            pre: self.0,
            expr: expr.into(),
        }
    }
}

/// The canonical entry point: lower `e`, returning its text and whatever
/// statements must run first.
pub(crate) fn lower_expr(e: &Expr, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.commons().tys();
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
        ExprKind::IntLit { value: n, .. } => n.to_string(),
        // Slice C: `Wire(<String>)` in a generic position lowers to its raw inner
        // string. The system-http driver site intercepts `Wire` args before this
        // to route them raw (no serialisation) and switch to the outcome decoder.
        ExprKind::Wire(inner) => pre.lower(inner, cx),
        // v0.21: the stored lexeme verbatim — `1e10` must not normalise.
        ExprKind::FloatLit { lexeme, .. } => lexeme.clone(),
        // v0.86 (ADR 0112): a `Duration` literal lowers to its constant
        // milliseconds (the value `Duration` erases to).
        ExprKind::DurationLit { millis, .. } => millis.to_string(),
        ExprKind::StrLit(s) => format!("\"{}\"", escape_ts_string(s)),
        // v0.43 (ADR 0075): an interpolated string lowers to a TS template
        // literal — chunks as escaped literal text, holes as `${String(…)}`.
        ExprKind::InterpStr(parts) => pre.absorb(lower_interp_str(parts, cx)),
        ExprKind::BoolLit(b) => b.to_string(),
        // v0.20b: a list literal lowers to a TS array literal; `readonly` is
        // a type-level property and the checker owns the element typing.
        ExprKind::ListLit(elems) => {
            let lowered: Vec<String> = elems.iter().map(|el| pre.lower(el, cx)).collect();
            format!("[{}]", lowered.join(", "))
        }
        ExprKind::Ident(id) => lower_ident(e, id, cx),
        ExprKind::Call { name, args, .. } => pre.absorb(lower_call(e, name, args, cx)),
        ExprKind::UnaryOp(op, inner) => {
            let inner = pre.lower(inner, cx);
            let sym = match op {
                UnaryOp::Neg => "-",
                UnaryOp::Not => "!",
            };
            format!("{sym}{inner}")
        }
        ExprKind::BinOp(op, lhs, rhs) => pre.absorb(lower_bin_op(*op, lhs, rhs, cx)),
        ExprKind::Paren(inner) => {
            let s = pre.lower(inner, cx);
            format!("({s})")
        }
        ExprKind::Ok(inner) => {
            let s = pre.lower(inner, cx);
            // v0.9: `Ok` is overloaded — use the checker's recorded type to
            // decide between `Result.Ok` and `HttpResult.Ok`.
            if matches!(
                cx.commons().expr_ty(e.id).as_deref(),
                Some(Ty::HttpResult(_))
            ) {
                cx.note_http_result();
                format!("HttpResult.Ok({s})")
            } else {
                format!("Ok({s})")
            }
        }
        ExprKind::Err(inner) => {
            let s = pre.lower(inner, cx);
            format!("Err({s})")
        }
        ExprKind::Some(inner) => {
            let s = pre.lower(inner, cx);
            format!("Some({s})")
        }
        ExprKind::None => "None".to_string(),
        ExprKind::Question(inner) => {
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
            let embed = if is_option {
                None
            } else {
                embed_conversion(operand_ty, cx)
            };
            let inner_expr = pre.lower(inner, cx);
            let tmp = cx.fresh();
            pre.push(format!("const {tmp} = {inner_expr};"));
            if is_option {
                pre.push(format!(
                    "if ({tmp}.tag === \"None\") return HttpResult.NotFound;"
                ));
            } else if let Some((ty_name, variant)) = embed {
                pre.push(format!(
                    "if ({tmp}.tag === \"Err\") return Err({ty_name}.{variant}({tmp}.error));"
                ));
            } else {
                pre.push(format!("if ({tmp}.tag === \"Err\") return {tmp};"));
            }
            // T2.3 (R6.3): every branch above just pushed a `return` that must
            // exit the enclosing function, not whatever scope this `Lowered`
            // eventually lands in — see `LowerCtx::emitted_early_return`.
            cx.emitted_early_return = true;
            format!("{tmp}.value")
        }
        ExprKind::RecordConstruction { type_name, fields } => {
            pre.absorb(lower_record_construction(type_name, fields, cx))
        }
        ExprKind::FieldAccess { .. }
            if matches!(
                cx.commons().callees.get(&e.id),
                Some(bynk_check::checker::Callee::Intrinsic { ns, .. })
                    if *ns == bynk_check::checker::RUN_ID_NS
            ) =>
        {
            // #2138: `run.id` — the module's `__bynkRunId`.
            "__bynkRunId".to_string()
        }
        ExprKind::FieldAccess { receiver, field } => {
            pre.absorb(lower_field_access(e, receiver, field, cx))
        }
        ExprKind::MethodCall {
            receiver,
            method,
            type_args,
            args,
        } => pre.absorb(lower_method_call(e, receiver, method, type_args, args, cx)),
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
            pre.absorb(hoist_value_as_statement(e, cx, |out, cx| {
                emit_if_tail(out, cond, then_block, else_block, cx, INDENT_STEP, false)
            }))
        }
        ExprKind::If {
            cond,
            then_block,
            else_block,
        } => pre.absorb(lower_if(e, cond, then_block, else_block, cx)),
        // v0.20a: a lambda lowers to a TS arrow; `async` iff its checked type
        // is an effectful function. A lambda is deliberately its own return
        // scope (ADR 0178) — its body is lowered with no access to the outer
        // `pre`, so anything it hoists (match-as-IIFE etc.) stays local to
        // the arrow, correctly, because a `?`'s `return` there is supposed to
        // exit the lambda.
        ExprKind::Lambda(lambda) => lower_lambda(e, lambda, cx),
        // Defensive: the parser builds `ExprKind::Block` only for a lambda body,
        // which `lower_lambda` emits itself, so neither `Block` arm is reached
        // today. Routed like `if`/`match` so a future value-position block
        // with a `?` would not reintroduce the arrow miscompile.
        ExprKind::Block(b) if block_contains_question(b) => {
            pre.absorb(hoist_value_as_statement(e, cx, |out, cx| {
                emit_block_as_function_body(out, b, cx, INDENT_STEP, false)
            }))
        }
        ExprKind::Block(b) => lower_block_as_expr(b, cx),
        ExprKind::Match { discriminant, arms } if arms_contain_question(arms) => {
            pre.absorb(hoist_value_as_statement(e, cx, |out, cx| {
                emit_match_tail(out, discriminant, arms, cx, INDENT_STEP, false)
            }))
        }
        ExprKind::Match { discriminant, arms } => {
            pre.absorb(lower_match_as_iife(discriminant, arms, cx))
        }
        ExprKind::Is { value, pattern } => pre.absorb(lower_is(value, pattern, cx)),
        ExprKind::UnitLit => "undefined".to_string(),
        ExprKind::EffectPure(inner) => {
            let inner_expr = pre.lower(inner, cx);
            format!("globalThis.Promise.resolve({inner_expr})")
        }
        ExprKind::RecordSpread {
            type_name: _,
            base,
            overrides,
        } => pre.absorb(lower_record_spread(base, overrides, cx)),
        ExprKind::Expect(inner) => {
            // v0.9.1: expect as an expression (v0.112, renamed from `assert`).
            // Emit a runtime helper call that returns void (i.e., evaluates to
            // `undefined` at runtime and is treated as the unit value `()` in Bynk
            // terms). The expression form reports the predicate source only — the
            // statement form carries the structural expected-vs-actual report.
            let value = pre.lower(inner, cx);
            let span_start = inner.span.start;
            let span_end = inner.span.end;
            let location = expect_location(cx, span_start);
            let src = expect_source_text(cx, inner.span);
            format!(
                "__bynkExpect(({value}), \"{location}\", {span_start}, {span_end}, \"expect {src}\")"
            )
        }
        ExprKind::Val { type_ref, args } => pre.absorb(lower_val(type_ref, args, cx)),
        ExprKind::Observation(o) => lower_observation(o, cx),
        ExprKind::Faults(f) => lower_faults(f, cx),
        ExprKind::Trace { cap, op } => {
            // `trace(Cap.op)` → the recorded calls mapped to per-call records
            // whose fields are the operation's parameters (positionally).
            let obs = cx
                .observation_trace()
                .map(str::to_string)
                .unwrap_or_else(|| "__obs".to_string());
            let key = format!("{}.{}", cap.name, op.name);
            let names = cap_op_param_names(cx, &cap.name, &op.name);
            let fields = names
                .iter()
                .enumerate()
                .map(|(i, n)| format!("{n}: __c.args[{i}]"))
                .collect::<Vec<_>>()
                .join(", ");
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
            let record_ty = bynk_check::checker::call_record_type_name(&cap.name, &op.name);
            format!(
                "((({obs}.log[{key:?}] ?? []).map((__c: {{ args: unknown[] }}) => ({{ {fields} }}))) as {record_ty}[])"
            )
        }
    };
    pre.finish(expr)
}

/// The parameter names of a capability operation, looked up from the capability
/// declarations in scope (v0.117). Used to destructure a recorded call's
/// arguments for a `with` predicate and to build `trace(Cap.op)` records.
/// P6.29 (design/tracks/the-ir.md §6a): the declaration lookup itself now reads
/// `bynk_lower::capability_op_sig_from_commons` — still a by-name walk
/// under the hood (nothing IR-native replaces "find the op named `op` on the
/// capability named `cap`"), but relocated to `ir/lower.rs` (excluded from
/// `ast_importers` — the `Ast → Ir` lowering pass's own job), so this file no
/// longer spells `bynk_syntax::ast::CommonsItem` itself.
fn cap_op_param_names(cx: &LowerCtx, cap: &str, op: &str) -> Vec<String> {
    bynk_lower::capability_op_sig_from_commons(cx.commons(), cap, op)
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
/// #2135: the driver name a reserved bad-signature principal drives: `by
/// Forged` (a wrong secret) or `by Stale` (a timestamp past the tolerance).
fn bad_signature_driver(actor: &str) -> Option<&'static str> {
    match actor {
        "Forged" => Some("forged"),
        "Stale" => Some("stale"),
        _ => None,
    }
}

fn lower_faults(faults: &FaultsExpr, cx: &mut LowerCtx) -> String {
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
    let value = inner.lower(call, cx);
    cx.call_site_identity = saved_identity;
    cx.call_site_actor = saved_actor;
    cx.call_site_no_credential = saved_no_credential;
    cx.call_site_bad_signature = saved_bad_signature;
    // The literal `await` below is a real source of effectfulness for any
    // synchronous-looking IIFE further out — see `emitted_await`.
    cx.emitted_await = true;
    let hoisted: String = inner.stmts().iter().map(|s| format!("{s} ")).collect();
    format!(
        "(await (async () => {{ try {{ {hoisted}await ({value}); return false; }} catch (__e) {{ if (__e instanceof ExpectationError) {{ throw __e; }} return true; }} }})())"
    )
}

/// Lower an observation (v0.117) to a `Bool` JavaScript expression over the
/// recorded trace object. `total`/`matching` counts drive the sugar; `before`
/// compares call order.
fn lower_observation(o: &ObservationExpr, cx: &mut LowerCtx) -> String {
    let obs = cx
        .observation_trace()
        .map(str::to_string)
        .unwrap_or_else(|| "__obs".to_string());
    let key = format!("{}.{}", o.cap.name, o.op.name);
    let calls = format!("({obs}.log[{key:?}] ?? [])");
    match &o.matcher {
        ObservationMatcher::NeverCalled => format!("({calls}.length === 0)"),
        ObservationMatcher::Before { cap, op } => {
            let key2 = format!("{}.{}", cap.name, op.name);
            let calls2 = format!("({obs}.log[{key2:?}] ?? [])");
            format!(
                "({calls}.length > 0 && {calls2}.length > 0 && {calls}[0].order < {calls2}[0].order)"
            )
        }
        ObservationMatcher::Called { count, with_pred } => match with_pred {
            None => match count {
                None => format!("({calls}.length >= 1)"),
                Some(c) => {
                    let n = lower_expr(c, cx).expr;
                    format!("({calls}.length === ({n}))")
                }
            },
            Some(p) => {
                let names = cap_op_param_names(cx, &o.cap.name, &o.op.name);
                // #291: typed through the operation's call-record alias (as
                // `trace` is), so a predicate may read a record parameter's
                // fields (`with req.url == …`) — `__c.args` is `unknown[]`.
                let destructure = if names.is_empty() {
                    String::new()
                } else {
                    let record_ty =
                        bynk_check::checker::call_record_type_name(&o.cap.name, &o.op.name);
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
                let pred_lowered = lower_expr(p, cx);
                let pre_src = pred_lowered.pre.join(" ");
                let pred = pred_lowered.expr;
                // P7.2: same shape as `lower_observation`'s `Trace` sibling above — only
                // `.args` is destructured, so `unknown[]` is what's actually known.
                let matching = format!(
                    "{calls}.filter((__c: {{ args: unknown[] }}) => {{ {destructure}{pre_src}return ({pred}); }}).length"
                );
                match count {
                    None => format!("(({matching}) >= 1)"),
                    Some(c) => {
                        let n = lower_expr(c, cx).expr;
                        format!("(({matching}) === ({n}))")
                    }
                }
            }
        },
    }
}

/// A default base-type literal (as TypeScript source) that satisfies a refined
/// type's predicates, for bare `Val[T]`. `None` when no default can be derived
/// (a `Matches` refinement — the checker rejects bare `Val` for those).
fn refined_default(decl: &TypeDecl) -> Option<String> {
    let (base, refinement) = match &decl.body {
        TypeBody::Refined {
            base, refinement, ..
        } => (*base, refinement.as_ref()),
        _ => return None,
    };
    match base {
        BaseType::Int => {
            let mut lo: i64 = 0;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1),
                        PredKind::NonNegative => lo = lo.max(0),
                        PredKind::InRange(a, _) => lo = lo.max(a.value),
                        _ => {}
                    }
                }
            }
            Some(lo.to_string())
        }
        BaseType::String => {
            let mut len: i64 = 0;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match p.kind {
                        PredKind::NonEmpty => len = len.max(1),
                        PredKind::MinLength(k) | PredKind::Length(k) => len = len.max(k),
                        PredKind::Matches(_) => return None,
                        _ => {}
                    }
                }
            }
            if len < 1 {
                len = 1;
            }
            Some(format!("\"{}\"", "x".repeat(len as usize)))
        }
        BaseType::Bool => Some("true".to_string()),
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
            Some(lo.to_string())
        }
        // v0.86: `Duration` carries no refinement; `0` millis is its default.
        BaseType::Duration | BaseType::Instant => Some("0".to_string()),
        // v0.110: `Bytes` carries no refinement; the empty octet sequence is
        // its default.
        BaseType::Bytes => Some("new globalThis.Uint8Array()".to_string()),
    }
}

/// v0.9.4 Part B (slice 1): lower a refined-type `Val[T]` / `Val[T](lit)` to
/// the branded `unsafe` constructor. The checker has already validated this is a
/// refined type in a test body, and recorded the refined type at `span`.
/// v0.9.4 slice 2 recursion cap for bare `Val` generation (mirrors the
/// checker's `MOCK_DEPTH`).
const MOCK_DEPTH: u32 = 12;

/// A TypeScript base-literal default for an opaque type's underlying base. Not
/// distinct per call in this increment — per-call distinctness via a runtime
/// counter is a follow-up.
fn base_default_ts(base: BaseType) -> String {
    match base {
        BaseType::Int => "0".to_string(),
        BaseType::String => "\"mock\"".to_string(),
        BaseType::Bool => "true".to_string(),
        BaseType::Float => "0".to_string(),
        BaseType::Duration | BaseType::Instant => "0".to_string(),
        BaseType::Bytes => "new globalThis.Uint8Array()".to_string(),
    }
}

/// Generate a TypeScript expression for a bare `Val` of `ty` (v0.9.4 Part B,
/// slice 2). Recurses through sum payloads and record fields; refined types use
/// `refined_default`, opaque types wrap a base default, bare bases use 0/""/true.
fn mock_value(ty: TyId, cx: &LowerCtx, depth: u32) -> String {
    let tys = cx.commons().tys();
    if depth == 0 {
        return "undefined".to_string();
    }
    match &*tys.get(ty) {
        Ty::Base(BaseType::Int) => "0".to_string(),
        Ty::Base(BaseType::String) => "\"\"".to_string(),
        Ty::Base(BaseType::Bool) => "true".to_string(),
        Ty::Base(BaseType::Float) => "0".to_string(),
        Ty::Named { name, .. } => {
            let Some(decl) = cx.commons().types.get(name.as_str()) else {
                return "undefined".to_string();
            };
            match &decl.body {
                TypeBody::Refined { .. } => {
                    let d = refined_default(decl).unwrap_or_else(|| "0".to_string());
                    unchecked_construct_test(name, &d, false)
                }
                TypeBody::Opaque { base, .. } => {
                    unchecked_construct_test(name, &base_default_ts(*base), true)
                }
                TypeBody::Sum(s) => match s.variants.first() {
                    None => "undefined".to_string(),
                    Some(v) if v.payload.is_empty() => format!("{name}.{}", v.name.name),
                    Some(v) => {
                        let args: Vec<String> = v
                            .payload
                            .iter()
                            .map(|f| {
                                bynk_check::checker::resolve_type_ref(
                                    &f.type_ref,
                                    &cx.commons().types,
                                    tys,
                                )
                                .map(|t| mock_value(t, cx, depth - 1))
                                .unwrap_or_else(|| "undefined".to_string())
                            })
                            .collect();
                        format!("{name}.{}({})", v.name.name, args.join(", "))
                    }
                },
                TypeBody::Record(r) => {
                    let parts: Vec<String> = r
                        .fields
                        .iter()
                        .map(|f| {
                            let fv = bynk_check::checker::resolve_type_ref(
                                &f.type_ref,
                                &cx.commons().types,
                                tys,
                            )
                            .map(|t| mock_value(t, cx, depth - 1))
                            .unwrap_or_else(|| "undefined".to_string());
                            format!("{}: {}", f.name.name, fv)
                        })
                        .collect();
                    format!("{{ {} }}", parts.join(", "))
                }
            }
        }
        _ => "undefined".to_string(),
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
fn capability_call_type_args_ts(type_args: &[TypeRef]) -> String {
    if type_args.is_empty() {
        return String::new();
    }
    format!(
        "<{}>",
        type_args
            .iter()
            .map(ts_type_ref)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// #934 (call-site key scoping): `Idempotency.dedup[T](key)` and
/// `Idempotency.remember[T](key, value, expiresAfter)` both take the
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
    if !is_first_party || !matches!(method, "dedup" | "remember") {
        return;
    }
    let scope = cx
        .handler_scope()
        .unwrap_or_else(|| panic!("Idempotency.{method} lowered with no handler_scope set"));
    args_lowered[0] = format!("`{scope}::${{{}}}`", args_lowered[0]);
}

fn lower_method_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Lowered {
    let tys = cx.commons().tys();
    let mut pre = Pre::new();
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
    // mirroring the ordinary (non-held) Map branch's own guard just below.
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
        return pre.finish(match method.name.as_str() {
            "put" => format!(
                "({}, undefined)",
                own_set(&m, &format!("String({})", a[0]), &format!("__connIdOf({})", a[1]))
            ),
            "remove" => format!(
                "(async () => {{ const __k = String({0}); const __cid = globalThis.Object.hasOwn({m}, __k) ? {m}[__k] : undefined; if (__cid !== undefined) {{ const __c = __resolveConnection<{f_ts}>(this.state, __cid); if (__c.tag === \"Some\") {{ await __c.value.close(); }} delete {m}[__k]; }} return undefined; }})()",
                a[0]
            ),
            "contains" => format!("globalThis.Object.hasOwn({m}, String({}))", a[0]),
            "size" => format!("globalThis.Object.keys({m}).length"),
            "get" => format!(
                "(() => {{ const __k = String({0}); return globalThis.Object.hasOwn({m}, __k) ? __resolveConnection<{f_ts}>(this.state, {m}[__k]) : None; }})()",
                a[0]
            ),
            // Any non-entry op is a lazy query lifting the map into a scan over its
            // **resolved** connections (the present ones — a connId whose socket has
            // closed drops out).
            _ => {
                let other_elem_ts = args
                    .first()
                    .map(|_| join_other_elem_ts(args, cx))
                    .unwrap_or_default();
                lower_query_method(
                    format!(
                        "globalThis.Object.values({m}).flatMap((__cid) => {{ const __c = __resolveConnection<{f_ts}>(this.state, __cid); return __c.tag === \"Some\" ? [__c.value] : []; }})"
                    ),
                    method,
                    &a,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &f_ts,
                    &other_elem_ts,
                    tys,
                )
            }
            .unwrap_or_else(|| {
                format!("(/* unsupported held Map op {} */ undefined)", method.name)
            }),
        });
    }
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
        // (`<map>__idx_<f>[v]`) instead of a full `Object.values` scan. The lookup
        // reads the staged map, so it stays read-your-writes (the index is
        // maintained in the same commit).
        if method.name == "filter"
            && let [arg] = args
            && let ExprKind::Lambda(lam) = &arg.kind
            && let Some(routed) = pre.absorb_opt(route_indexed_filter(
                &m,
                &var,
                &id.name,
                &idx_fields,
                lam,
                cx,
            ))
        {
            return pre.finish(routed);
        }
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        return pre.finish(match method.name.as_str() {
            // v0.93: an indexed map's mutators keep the sibling posting-lists exact
            // inside the same staged commit (re-index on last-write-wins).
            "put" if !idx_fields.is_empty() => idx_map_put(&m, &var, &id.name, &idx_fields, &a),
            "remove" if !idx_fields.is_empty() => {
                idx_map_remove(&m, &var, &id.name, &idx_fields, &a)
            }
            "update" if !idx_fields.is_empty() => {
                idx_map_update(&m, &var, &id.name, &idx_fields, &a)
            }
            "upsert" if !idx_fields.is_empty() => {
                idx_map_upsert(&m, &var, &id.name, &idx_fields, &a)
            }
            "put" => format!("({}, undefined)", own_set(&m, &a[0], &a[1])),
            "remove" => format!("((delete {m}[{}]), undefined)", a[0]),
            "contains" => format!("globalThis.Object.hasOwn({m}, {})", a[0]),
            "size" => format!("globalThis.Object.keys({m}).length"),
            "get" => format!(
                "(() => {{ const __k = {0}; return globalThis.Object.hasOwn({m}, __k) ? Some({m}[__k]) : None; }})()",
                a[0]
            ),
            "update" => format!(
                "(() => {{ const __k = {0}; if (!globalThis.Object.hasOwn({m}, __k)) {{ throw new globalThis.Error(\"Map.update: key absent\"); }} {m}[__k] = ({1})({m}[__k]); return undefined; }})()",
                a[0], a[1]
            ),
            "upsert" => format!(
                "(() => {{ const __k = {0}; {set}; return undefined; }})()",
                a[0],
                set = own_set(
                    &m,
                    "__k",
                    &format!("({})(globalThis.Object.hasOwn({m}, __k) ? {m}[__k] : ({}))", a[2], a[1])
                ),
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
                let other_elem_ts = args
                    .first()
                    .map(|_| join_other_elem_ts(args, cx))
                    .unwrap_or_default();
                lower_query_method(
                    format!("globalThis.Object.values({m})"),
                    method,
                    &a,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &elem_ts,
                    &other_elem_ts,
                    tys,
                )
                .unwrap_or_else(|| format!("(/* unsupported Map op {} */ undefined)", method.name))
            }
        });
    }
    // v0.83: a storage-`Set` operation — `<set>.<op>(…)` on a `store Set[T]` field.
    // Lowers to an entry op over `__state.<set>` (a `Record<string, boolean>`):
    // `add`/`remove` mutate the working record, `contains`/`size` read it.
    //
    // P6.21 (partial, continued): reads `Callee::Store` (P6.0) instead of
    // `!cx.is_local(&id.name)` — see the storage-`Map` branch above for the
    // full reasoning. `Set` never gets `Callee::Query` (checker.rs's own
    // `StoreField::Set` arm records only `Store`), unlike `Map`/`Log`.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Store { .. }))
        && cx.is_agent_store_set(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let s = format!("{var}.{}", id.name);
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        return pre.finish(match method.name.as_str() {
            "add" => format!("({}, undefined)", own_set(&s, &a[0], "true")),
            "remove" => format!("((delete {s}[{}]), undefined)", a[0]),
            "contains" => format!("globalThis.Object.hasOwn({s}, {})", a[0]),
            "size" => format!("globalThis.Object.keys({s}).length"),
            other => format!("(/* unsupported Set op {other} */ undefined)"),
        });
    }
    // v0.87 (ADR 0113): a storage-`Cache` operation — `<cache>.<op>(…)` on a
    // `store Cache[K, V]` field. Lowers to an entry op over `__state.<cache>` (a
    // `Record<string, { v, exp }>`) that applies TTL expiry against the injected
    // `Clock`: every op but `remove` reads `now()` (an awaited `Effect`), so the
    // op is an async IIFE. `put`/`update`/`upsert` stamp `exp = now() + ttl`;
    // `get`/`contains`/`size` treat an entry past `exp` as absent.
    //
    // P6.21 (partial, continued): reads `Callee::Store` instead of
    // `!cx.is_local(&id.name)` — see the storage-`Map` branch above. `Cache`
    // never gets `Callee::Query`, same as `Set`.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Store { .. }))
        && let Some(ttl) = cx.agent_store_cache_ttl(&id.name)
    {
        let var = cx.agent_store_var().to_string();
        let c = format!("{var}.{}", id.name);
        let now = format!("await {}.Clock.now()", cx.cap_deps_expr());
        let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
        return pre.finish(match method.name.as_str() {
            "remove" => format!("((delete {c}[{}]), undefined)", a[0]),
            "put" => format!(
                "(async () => {{ const __now = {now}; {set}; return undefined; }})()",
                set = own_set(&c, &a[0], &format!("{{ v: {}, exp: __now + {ttl} }}", a[1])),
            ),
            "get" => format!(
                "(async () => {{ const __now = {now}; const __k = {0}; return (globalThis.Object.hasOwn({c}, __k) && {c}[__k].exp > __now) ? Some({c}[__k].v) : None; }})()",
                a[0]
            ),
            "contains" => format!(
                "(async () => {{ const __now = {now}; const __k = {0}; return globalThis.Object.hasOwn({c}, __k) && {c}[__k].exp > __now; }})()",
                a[0]
            ),
            "size" => format!(
                "(async () => {{ const __now = {now}; return globalThis.Object.values({c}).filter((__e) => __e.exp > __now).length; }})()"
            ),
            "update" => format!(
                "(async () => {{ const __now = {now}; const __k = {0}; if (!(globalThis.Object.hasOwn({c}, __k) && {c}[__k].exp > __now)) {{ throw new globalThis.Error(\"Cache.update: key absent\"); }} {c}[__k] = {{ v: ({1})({c}[__k].v), exp: __now + {ttl} }}; return undefined; }})()",
                a[0], a[1]
            ),
            "upsert" => format!(
                "(async () => {{ const __now = {now}; const __k = {0}; const __cur = (globalThis.Object.hasOwn({c}, __k) && {c}[__k].exp > __now) ? {c}[__k].v : ({1}); {set}; return undefined; }})()",
                a[0],
                a[1],
                set = own_set(&c, "__k", &format!("{{ v: ({})(__cur), exp: __now + {ttl} }}", a[2])),
            ),
            other => format!("(/* unsupported Cache op {other} */ undefined)"),
        });
    }
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
                Some(ms) => format!(
                    " for (let __i = {g}.length - 1; __i >= 0; __i--) {{ if ({g}[__i].t < __now - {ms}) {g}.splice(__i, 1); }}"
                ),
                None => String::new(),
            };
            return pre.finish(format!(
                "(async () => {{ const __now = {now}; {g}.push({{ t: __now, v: {0} }});{prune} return undefined; }})()",
                a[0]
            ));
        }
        // The values array feeding the general query pipeline.
        let values = format!("{g}.map((__e) => __e.v)");
        let thunk = |body: String| format!("(() => {body})");
        return pre.finish(match method.name.as_str() {
            // Time-window roots → a `Query` thunk over the windowed values.
            "since" => thunk(format!(
                "{g}.filter((__e) => __e.t >= ({0})).map((__e) => __e.v)",
                a[0]
            )),
            "before" => thunk(format!(
                "{g}.filter((__e) => __e.t < ({0})).map((__e) => __e.v)",
                a[0]
            )),
            "between" => thunk(format!(
                "{g}.filter((__e) => __e.t >= ({0}) && __e.t <= ({1})).map((__e) => __e.v)",
                a[0], a[1]
            )),
            "recent" => thunk(format!(
                "{g}.slice(globalThis.Math.max(0, {g}.length - globalThis.Math.max(0, {0}))).reverse().map((__e) => __e.v)",
                a[0]
            )),
            "reversed" => thunk(format!("[...{g}].reverse().map((__e) => __e.v)")),
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
                let other_elem_ts = args
                    .first()
                    .map(|_| join_other_elem_ts(args, cx))
                    .unwrap_or_default();
                lower_query_method(
                    values,
                    method,
                    &a,
                    cx.commons().expr_types.get(&e.id).map(|te| te.ty),
                    &elem_ts,
                    &other_elem_ts,
                    tys,
                )
                .unwrap_or_else(|| format!("(/* unsupported Log op {} */ undefined)", method.name))
            }
        });
    }
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
        return pre.finish(match method.name.as_str() {
            "update" => format!("(() => {{ {n} = ({0})({n}); return undefined; }})()", a[0]),
            other => format!("(/* unsupported Cell op {other} */ undefined)"),
        });
    }
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
        let args_lowered = lower_http_variant_args(&method.name, args, &mut pre, cx);
        return pre.finish(format!(
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
                return pre.finish(format!("new globalThis.Set<{}>()", ts_ty(*t, tys)));
            }
            Some(Ty::List(t)) => {
                return pre.finish(format!(
                    "([] as readonly {}[])",
                    ts_array_elem(&ts_ty(*t, tys))
                ));
            }
            Some(Ty::Map(k, v)) => {
                return pre.finish(format!(
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
        return pre.finish(format!("new globalThis.Set<{}>({xs})", ts_ty(t, tys)));
    }
    // v0.22b: the typed JSON codec (ADR 0045).
    if let Some(s) = pre.absorb_opt(lower_json_codec_call(e, receiver, method, args, cx)) {
        return pre.finish(s);
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
        return pre.finish(format!("globalThis.Math.{constant}"));
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
        return pre.finish(format!(
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
        return pre.finish(inner);
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
        return pre.finish(format!("__bynkInstantFromIso({s})"));
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
        return pre.finish(inner);
    }
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
                return pre.finish(format!("new globalThis.TextEncoder().encode({s})"));
            }
            ("fromBase64", 1) => {
                let s = pre.lower(&args[0], cx);
                cx.note_bytes();
                return pre.finish(format!("__bynkBytesFromBase64({s})"));
            }
            // #2016: the guarded hex and base64url decodes, `None` on bad input.
            ("fromHex", 1) => {
                let s = pre.lower(&args[0], cx);
                cx.note_bytes_ops();
                return pre.finish(format!("__bynkBytesFromHex({s})"));
            }
            ("fromBase64Url", 1) => {
                let s = pre.lower(&args[0], cx);
                cx.note_bytes_ops();
                return pre.finish(format!("__bynkBytesFromBase64Url({s})"));
            }
            ("empty", 0) => {
                return pre.finish("new globalThis.Uint8Array()".to_string());
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
        return pre.finish(format!("__tokenClaims({xs})"));
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
                return pre.finish(format!("__requestHeaders({xs})"));
            }
            ("empty", 0) => {
                cx.note_request_headers();
                return pre.finish("__requestHeaders([])".to_string());
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
        return pre.finish(format!(
            "(async function* () {{ for (const __e of {xs}) {{ yield __e; }} }})()"
        ));
    }
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
        return pre.finish(format!(
            "{}.{}.{}{}({})",
            cx.cap_deps_expr(),
            cap,
            method.name,
            capability_call_type_args_ts(type_args),
            args_lowered.join(", ")
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
        return pre.finish(s);
    }
    // Events track, slice 0 (spine #936): `Events.emit[E](event)` never
    // calls through a provider like an ordinary capability op — release-
    // at-commit (events.md §3.0) buffers it into the handler's own
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
            Some(Callee::Capability { cap, op }) if cap == "Events" && op == "emit"
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
        return pre.finish(format!(
            "(async () => {{ __events.push({{ type: \"{event_name}\", payload: {payload}, envelope: {{ eventId: globalThis.crypto.randomUUID(), publisherId: \"{publisher_id}\", emittedAt: globalThis.Date.now(), schemaVersion: {schema_version} }} }}); }})()"
        ));
    }
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
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        return pre.finish(format!(
            "deps.{}.{}({})",
            id.name,
            method.name,
            args_lowered.join(", ")
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
        return pre.finish(format!(
            "{}.{}.{}{}({})",
            cx.cap_deps_expr(),
            id.name,
            method.name,
            capability_call_type_args_ts(type_args),
            args_lowered.join(", ")
        ));
    }
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
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        return pre.finish(format!(
            "{}.{}({})",
            id.name,
            method.name,
            args_lowered.join(", ")
        ));
    }
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
        return pre.finish(format!("__bynkAssetGet(({asset}).url)"));
    }
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
            return pre.finish(format!("{driver}({})", rest.join(", ")));
        }
    }
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
            return pre.finish(format!(
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
        let rest: Vec<String> = args[1..]
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let lowered = pre.lower(a, cx);
                if !is_raw || matches!(&a.kind, ExprKind::Wire(_)) {
                    return lowered;
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
        return pre.finish(format!(
            "{}_{}_{}({})",
            driver,
            id.name,
            key,
            all.join(", ")
        ));
    }
    // #2024: a case `let` may shadow a service of the target; the local wins.
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.is_test_service(&id.name)
        && !cx.is_local(&id.name)
    {
        // #1994: a target whose handlers send with `~>` settles each addressed
        // call — awaits the promises the handler handed `deps.__exec.waitUntil`
        // — so the case's expectations see the sends' effects.
        let settles = cx.test_settles_sends();
        let settle = |call: String| {
            if settles {
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
            return pre.finish(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
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
                    if let bynk_ir::IrHandlerKind::Cron { expr: e } = h {
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
                return pre.finish(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
            }
            if method.name == "message"
                && handlers
                    .iter()
                    .any(|h| matches!(h, bynk_ir::IrHandlerKind::Message))
            {
                // A `from queue` service binds exactly one queue and declares one
                // `on message` handler, so the position index is 0.
                let key = crate::emitter::queue_handler_method_name(&id.name, 0);
                let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
                let mut all = args_lowered;
                all.push(deps_expr);
                return pre.finish(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
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
                    .any(|h| matches!(h, bynk_ir::IrHandlerKind::Event))
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
                return pre.finish(settle(format!(
                    "__bynkEvent_{}({payload}, {envelope}, {deps_expr})",
                    id.name
                )));
            }
        }
        // `svc.call(args)` and other (non-http) forms: pass args through with deps.
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let mut all = args_lowered;
        all.push(deps_expr);
        return pre.finish(settle(format!(
            "{}.{}({})",
            id.name,
            method.name,
            all.join(", ")
        )));
    }
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
        // instance's own key — so dropping it is sound.
        if cx.ws_self_agent() == Some(name.name.as_str()) {
            cx.record_agent_call(&name.name, &method.name);
            let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
            let mut all = args_lowered;
            all.push("deps".to_string());
            return pre.finish(format!(
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
        return pre.finish(format!(
            "{instance}.{method}({args})",
            method = method.name,
            args = all.join(", ")
        ));
    }
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
        return pre.finish(format!("{}.{}({})", recv, method.name, all.join(", ")));
    }
    // v0.20b: built-in kernel methods on the collection types,
    // dispatched on the receiver's checked type. Emitted inline
    // (typed IIFEs / spreads) — no runtime imports, so files that
    // never touch collections emit byte-identically to v0.20a.
    if let Some(recv_ty) = cx.commons().expr_ty(receiver.id).as_deref().cloned() {
        match &recv_ty {
            Ty::List(elem) => {
                if let Some(s) =
                    pre.absorb_opt(lower_list_kernel(e, receiver, method, args, *elem, cx))
                {
                    return pre.finish(s);
                }
            }
            // v0.91 (ADR 0119): a chained op on a lazy `Query` — the source is
            // the receiver thunk, invoked (`(recv)()`).
            Ty::Query(elem) => {
                let elem_ts = ts_ty(*elem, tys);
                let recv = pre.lower(receiver, cx);
                let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
                let result_ty = cx.commons().expr_types.get(&e.id).map(|te| te.ty);
                let other_elem_ts = args
                    .first()
                    .map(|_| join_other_elem_ts(args, cx))
                    .unwrap_or_default();
                if let Some(s) = lower_query_method(
                    format!("({recv})()"),
                    method,
                    &a,
                    result_ty,
                    &elem_ts,
                    &other_elem_ts,
                    tys,
                ) {
                    return pre.finish(s);
                }
            }
            // v0.100: a chained op on a `Stream` — the receiver already *is* an
            // async iterable, so it is the source directly. Emitted inline as
            // async-generator IIFEs (builders) / an async drain (`collect`).
            Ty::Stream(_) => {
                let recv = pre.lower(receiver, cx);
                let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
                if let Some(s) = lower_stream_method(recv, method, &a) {
                    return pre.finish(s);
                }
            }
            // v0.102: the held-resource operations on a `Connection[F]` lower to
            // method calls on the runtime `Connection` object — `send(frame)` and
            // `close()`. The linearity pass has already verified ownership.
            Ty::Connection(_) => {
                let recv = pre.lower(receiver, cx);
                let a: Vec<String> = args.iter().map(|x| pre.lower(x, cx)).collect();
                return pre.finish(format!("({recv}).{}({})", method.name, a.join(", ")));
            }
            Ty::Map(key, val) => {
                if let Some(s) =
                    pre.absorb_opt(lower_map_kernel(receiver, method, args, *key, *val, cx))
                {
                    return pre.finish(s);
                }
            }
            // K6a (#2596): the `Set` kernel.
            Ty::Set(elem) => {
                if let Some(s) = pre.absorb_opt(lower_set_kernel(receiver, method, args, *elem, cx))
                {
                    return pre.finish(s);
                }
            }
            // v0.21: the numeric kernel. `toFloat` is the identity
            // at runtime (the Int/Float distinction is erased);
            // the four `Float -> Int` roundings map onto `Math.*`.
            // v0.22a extends it (abs/min/max/clamp, isNaN/isFinite).
            Ty::Base(base @ (BaseType::Int | BaseType::Float)) => {
                let base = *base;
                if let Some(s) =
                    pre.absorb_opt(lower_numeric_kernel(receiver, method, args, base, cx))
                {
                    return pre.finish(s);
                }
            }
            // v0.86 (ADR 0112): the `Duration` kernel. `toMillis` is the identity
            // at runtime (a `Duration` already *is* its milliseconds); `toString`
            // renders the number.
            Ty::Base(BaseType::Duration) => {
                if let Some(s) = pre.absorb_opt(lower_duration_kernel(receiver, method, args, cx)) {
                    return pre.finish(s);
                }
            }
            // v0.90 (ADR 0114): the `Instant` kernel. `toEpochMillis` is the
            // identity (an `Instant` lowers to its epoch millis); `toString`
            // renders the number.
            Ty::Base(BaseType::Instant) => {
                if let Some(s) = pre.absorb_opt(lower_instant_kernel(receiver, method, args, cx)) {
                    return pre.finish(s);
                }
            }
            // #2341 (decision A): an HTTP handler's `headers` is a runtime
            // object with these very methods, each answering an `Option`.
            Ty::RequestHeaders | Ty::TokenClaims => {
                let recv = pre.lower(receiver, cx);
                let args: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
                return pre.finish(format!("({recv}).{}({})", method.name, args.join(", ")));
            }
            // v0.110 (ADR 0142): the `Bytes` kernel. `length` is the octet
            // count; `toBase64` encodes; `decodeUtf8` is a guarded UTF-8 decode
            // returning `Option`.
            Ty::Base(BaseType::Bytes) => {
                if let Some(s) = pre.absorb_opt(lower_bytes_kernel(receiver, method, args, cx)) {
                    return pre.finish(s);
                }
            }
            // v0.22a: the string kernel (ADR 0046).
            Ty::Base(BaseType::String) => {
                if let Some(s) = pre.absorb_opt(lower_string_kernel(receiver, method, args, cx)) {
                    return pre.finish(s);
                }
            }
            // v0.22a: Option/Result combinators (ADR 0048).
            Ty::Option(inner) => {
                if let Some(s) =
                    pre.absorb_opt(lower_option_kernel(e, receiver, method, args, *inner, cx))
                {
                    return pre.finish(s);
                }
            }
            Ty::Result(ok, err) => {
                if let Some(s) = pre.absorb_opt(lower_result_kernel(
                    e, receiver, method, args, *ok, *err, cx,
                )) {
                    return pre.finish(s);
                }
            }
            // §2.8.3: the `Effect[Result[T, E]]` combinators — `mapOk`/`mapErr`/
            // `flatMapOk`/`flatMapErr`. Lowered as an `async` IIFE that awaits the
            // receiver `Promise<Result<…>>` and rebuilds the transformed Result.
            Ty::Effect(inner) => {
                if let Ty::Result(ok, err) = &*tys.get(*inner)
                    && let Some(s) = pre.absorb_opt(lower_effect_result_kernel(
                        e, receiver, method, args, *ok, *err, cx,
                    ))
                {
                    return pre.finish(s);
                }
            }
            // #561: a refined receiver inherits its base type's read-only kernel
            // methods (DECISION D). A refined value erases to its branded base
            // representation, so the inherited call lowers through the *same*
            // base-kernel helper as a plain base receiver — the emitted call is
            // byte-identical, with no unwrap/`.raw` step. Declared methods win:
            // when the refined type declares this method it takes the UFCS tail
            // below, so route to the kernel only when it is *not* declared.
            // `Bool` has no kernel and always falls through.
            Ty::Named {
                name,
                kind: NamedKind::Refined(base),
                ..
            } if !cx
                .commons()
                .methods
                .get(name)
                .is_some_and(|t| t.instance.contains_key(&method.name)) =>
            {
                let lowered = match base {
                    BaseType::Int | BaseType::Float => {
                        let base = *base;
                        pre.absorb_opt(lower_numeric_kernel(receiver, method, args, base, cx))
                    }
                    BaseType::String => {
                        pre.absorb_opt(lower_string_kernel(receiver, method, args, cx))
                    }
                    BaseType::Duration => {
                        pre.absorb_opt(lower_duration_kernel(receiver, method, args, cx))
                    }
                    BaseType::Instant => {
                        pre.absorb_opt(lower_instant_kernel(receiver, method, args, cx))
                    }
                    BaseType::Bytes => {
                        pre.absorb_opt(lower_bytes_kernel(receiver, method, args, cx))
                    }
                    BaseType::Bool => None,
                };
                if let Some(s) = lowered {
                    return pre.finish(s);
                }
            }
            _ => {}
        }
    }
    // Instance call: UFCS lowering with the receiver as first arg. The
    // namespace comes from the receiver's checked type. Every method call the
    // checker accepts has one: a test-service call's outcome is typed by its
    // handler (#1934), where it once reached this placeholder and failed only
    // at `tsc` (#1931). The placeholder stays reachable for a program that
    // failed checking, which is still lowered and whose output is discarded,
    // so it is not a panic.
    let ns = cx
        .receiver_namespace(receiver)
        .unwrap_or_else(|| "/* unknown */".to_string());
    let recv = pre.lower(receiver, cx);
    let mut all = vec![recv];
    for a in args {
        all.push(pre.lower(a, cx));
    }
    pre.finish(format!("{ns}.{}({})", method.name, all.join(", ")))
}

/// v0.22b: the typed JSON codec (ADR 0045). `encode` dispatches to the
/// module-local `serialise_*` helpers + `JSON.stringify`; `decode[T]` to
/// `JSON.parse` + `deserialise_*`, mapping a parse failure to a `Malformed`
/// JsonError and a BoundaryError to the uniform `kind`/`path`/`message`
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
    let tys = cx.commons().tys();
    let mut pre = Pre::new();
    if matches!(
        cx.commons().callees.get(&e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == JSON
    ) && args.len() == 1
    {
        if method.name == "encode"
            && let Some(arg_ty) = cx
                .commons()
                .expr_types
                .get(&args[0].id)
                .map(|te| &te.ty)
                .cloned()
            && let Some(tref) = ty_to_type_ref(arg_ty, tys)
        {
            // #917: a test-scaffold module's target/`uses` types export no codec
            // of their own — record the root so the module that spliced this
            // body in can generate its own `serialise_*`/`deserialise_*` closure,
            // the same way a workers cross-context caller generates one for a
            // consumed context's boundary types (#661). P6.28: pushes the `TyId`
            // already in hand, not a re-derived `TypeRef` — `tref` below still
            // exists for this call site's own, separate codec-rendering use.
            if cx.in_test_scaffold() {
                cx.runtime_use().note_json_codec_root(arg_ty);
            }
            let v = pre.lower(&args[0], cx);
            // #1435 (Arc E slice 1): boundary-print, same treatment as this
            // file's own `serialise_expr_via` call site above.
            let ser =
                bynk_ts::print_expr(&serialisation::serialise_expr(&tref, &v, cx.runtime_use()));
            return Some(pre.finish(format!("globalThis.JSON.stringify({ser})")));
        }
        if matches!(method.name.as_str(), "decode" | "decodeSql")
            && let Some(Ty::Result(t, _)) = cx.commons().expr_ty(e.id).as_deref().cloned()
            && let Some(tref) = ty_to_type_ref(t, tys)
        {
            // #917: in a test-scaffold module `T` may be foreign (owned by the
            // suite target or one of its `uses`, never declared locally), so its
            // TS type positions reach through the module's namespace qualifier —
            // empty everywhere else, where this renders identically to `ts_ty`.
            let ts = if cx.in_test_scaffold() {
                // #1437 (Arc E slice 2): `serialisation::ts_type_ref_qualified`
                // deleted — folded into `serialisation::qualified_ts_type`,
                // which bridges this file's own dotted-prefix qualifier
                // convention (`json_codec_qual`'s values carry a trailing
                // `.`) onto the general `TsType`-returning renderer
                // (`emitter.rs`'s `ts_type_ref_qualified_multi_ts_type`,
                // Arc C slice 33/#1401), which expects a bare namespace and
                // appends its own separator. This call site stays
                // `String`-based (`ts` feeds a hand-formatted signature
                // string below), so it prints at the boundary via
                // `bynk_ts::print_type`.
                let qual = cx.runtime_use().json_codec_qual();
                bynk_ts::print_type(&serialisation::qualified_ts_type(&tref, &qual))
            } else {
                ts_ty(t, tys)
            };
            // P6.28: pushes the `TyId` already in hand, not a re-derived
            // `TypeRef` — `tref` above still exists for this call site's own,
            // separate codec-rendering use.
            if cx.in_test_scaffold() {
                cx.runtime_use().note_json_codec_root(t);
            }
            // #914: the wrapper below names `Result`, `JsonValue` and `JsonError`
            // in its own signature and body, whichever arm the inner deserialiser
            // takes — including the delegating ones, which record nothing. A
            // module that curates its import list (the test-scaffold modules)
            // otherwise emits all three unimported.
            cx.runtime_use().note_json_codec();
            // #1435 (Arc E slice 1): boundary-print, same treatment as above.
            let des = bynk_ts::print_expr(&serialisation::deserialise_expr(
                &tref,
                "__j",
                "$",
                cx.runtime_use(),
            ));
            let sql_bools = if method.name == "decodeSql" {
                sql_bool_pass(t, cx)
            } else {
                String::new()
            };
            let arg = pre.lower(&args[0], cx);
            return Some(pre.finish(format!(
                "((__s: string): Result<{ts}, JsonError> => {{ \
                 let __j: __JsonValue; \
                 try {{ __j = globalThis.JSON.parse(__s) as __JsonValue; }} \
                 catch (__e) {{ return Err({{ kind: \"Malformed\", path: \"$\", message: String(__e) }}); }} \
                 {sql_bools}const __r = {des}; \
                 if (__r.tag === \"Ok\") return Ok(__r.value as {ts}); \
                 const __be = __r.error; \
                 return Err({{ kind: __be.kind, \
                 path: (__be.kind === \"StructuralMismatch\" || __be.kind === \"RefinementViolation\") ? __be.path : \"$\", \
                 message: __be.kind === \"StructuralMismatch\" ? `expected ${{__be.expected}}, got ${{String(__be.actual)}}` : __be.kind === \"RefinementViolation\" ? __be.violation.message : __be.details }}); }})({arg})"
            )));
        }
    }
    let text: Option<String> = None;
    text.map(|expr| pre.finish(expr))
}

/// P4c (#2628): `Json.decodeSql[T]`'s pass over the parsed rows before the
/// ordinary codec runs. SQLite has no boolean, so a flag column arrives as
/// `0`/`1`; at each `Bool` field of the row record (the checker has confined
/// `T` to a row or a `List` of one) exactly those two numbers become
/// `false`/`true`. Anything else is left for the codec to accept or refuse,
/// so every generated codec stays the one `Json.decode` uses. Empty when the
/// row has no `Bool` field.
fn sql_bool_pass(t: bynk_check::checker::TyId, cx: &LowerCtx) -> String {
    let tys = cx.commons().tys();
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
                    Some(format!(
                        "deps.surface.{key}.{}({})",
                        method.name,
                        args_lowered.join(", ")
                    ))
                }
                BuildTarget::Workers => {
                    let _ = key;
                    let call = pre.absorb(lower_workers_cross_context_call(
                        &consumed, method, args, cx,
                    ));
                    return Some(pre.finish(call));
                }
            }
        } else {
            None
        };
    text.map(|expr| pre.finish(expr))
}

fn lower_val(type_ref: &TypeRef, args: &[Expr], cx: &mut LowerCtx) -> Lowered {
    let tys = cx.commons().tys();
    let mut pre = Pre::new();
    // Resolve the mocked type straight from the AST node rather than the
    // checker's `expr_types` side-table — the static type table is always
    // populated, whereas a test body's per-expression types may not be visible
    // to the emitter.
    let ty = match bynk_check::checker::resolve_type_ref(type_ref, &cx.commons().types, tys) {
        Some(t) => t,
        None => return pre.finish("undefined /* mock: unresolved type */".to_string()),
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
        let raw = lower_const_literal_raw(arg).unwrap_or_else(|| pre.lower(arg, cx));
        return pre.finish(unchecked_construct_test(name, &raw, false));
    }
    // Bare mock (refined / opaque / sum / record).
    pre.finish(mock_value(ty, cx, MOCK_DEPTH))
}

/// When we encounter `lhs && rhs`, see if lhs is an `is` (possibly wrapped
/// in parens or nested `&&`) and if so collect the bindings to inject into
/// rhs. Returns `(binding_const_decls, lowered_lhs, lowered_rhs)` if
/// special handling is appropriate; otherwise returns None.
/// What [`lower_and_with_is`] produces for its caller to assemble.
struct AndWithIs {
    /// The left operand's `is` bindings, then the right operand's own hoisted
    /// statements: both run, in order, inside the right operand's IIFE.
    bindings: Vec<String>,
    lhs: Lowered,
    rhs: String,
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
    bindings.extend(rhs_lowered.pre);
    cx.shadow_scopes.pop();
    Some(AndWithIs {
        bindings,
        lhs: lhs_lowered,
        rhs: rhs_lowered.expr,
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
fn gather_is_bindings_for_emit(
    e: &Expr,
    when_true: bool,
    cx: &mut LowerCtx,
    out: &mut Vec<String>,
    found: &mut bool,
) {
    // #1654: which `is` tests `e` proves is the shared rule the resolver and
    // the checker scope bindings by (`bynk_check::narrowing`).
    for test in bynk_check::narrowing::matched_is_tests(e, when_true) {
        emit_is_test_bindings(test, cx, out, found);
    }
}

/// The `const name = …;` declarations for one matched `is` test.
fn emit_is_test_bindings(e: &Expr, cx: &mut LowerCtx, out: &mut Vec<String>, found: &mut bool) {
    let tys = cx.commons().tys();
    let ExprKind::Is { value, pattern } = &e.kind else {
        return;
    };
    {
        *found = true;
        let value_text = cx.is_receiver_text(value);
        let disc_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
        if let Pattern::Variant {
            variant, bindings, ..
        } = pattern.as_ref()
        {
            // #1751/#1752: a test emitted out of line (inside a short-circuit
            // arrow) doesn't narrow its receiver here; read through the
            // variant the checker proved instead.
            let unnarrowed = cx.unnarrowed_is_tests.contains(&e.span);
            let payload = variant_payload(&value_text, &variant.name, unnarrowed);
            // v0.13: refinement narrowing re-binds the value's name to the
            // branded refined type, read from the forced receiver temp.
            if bindings.is_empty()
                && cx.is_refined_is_check(value, &variant.name)
                && let ExprKind::Ident(id) = &value.kind
            {
                out.push(format!(
                    "const {name} = {value_text} as {refined};",
                    name = ts_ident(&id.name),
                    refined = variant.name,
                ));
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
                        cx.positional_field_name(disc_ty, &variant.name, i, tys)
                    }
                };
                // #1653: renamed when it shadows (`o is Some(o)`), so the
                // declaration can't read itself in its temporal dead zone.
                let local = cx.bind_local_name(&name.name);
                out.push(format!("const {local} = {payload}.{field};"));
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
            out.push(format!("let {};", decl.join(", ")));
            let last = alts.len() - 1;
            for (i, alt) in alts.iter().enumerate() {
                let (tag, pairs) = match alt {
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
                                    cx.positional_field_name(disc_ty, &variant.name, j, tys)
                                }
                            };
                            let local = cx
                                .resolved_local_name(&name.name)
                                .unwrap_or_else(|| ts_ident(&name.name));
                            // #1765 review: out of line, the dispatch below
                            // starts from the whole union, so its last arm (a
                            // bare `else`) isn't narrowed to this variant.
                            let payload = variant_payload(
                                &value_text,
                                &variant.name,
                                cx.unnarrowed_is_tests.contains(&e.span),
                            );
                            pairs.push((local, payload + "." + &field));
                        }
                        (Some(variant.name.clone()), pairs)
                    }
                    _ => (None, Vec::new()),
                };
                if i == last {
                    out.push("} else {".to_string());
                } else {
                    let cond = tag
                        .as_ref()
                        .map(|t| format!("{value_text}.tag === \"{t}\""))
                        .unwrap_or_else(|| "true".to_string());
                    let kw = if i == 0 { "if" } else { "} else if" };
                    out.push(format!("{kw} ({cond}) {{"));
                }
                for (name, path) in &pairs {
                    out.push(format!("  {name} = {path};"));
                }
            }
            out.push("}".to_string());
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

/// v0.150 (ADR 0174): peel a `traverseTry`/`parTraverseTry` call's checked type
/// `Effect[Result[List[U], E]]` to the `U` TS — the short-circuit collect's
/// output-array element annotation. Falls back to `any` if the shape is absent.
fn list_ok_elem_ts(call_ty: Option<TyId>, tys: &Arc<Types>) -> String {
    match call_ty.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
            Ty::Result(ok, _) => match &*tys.get(*ok) {
                Ty::List(u) => ts_ty(*u, tys),
                _ => ts_ty(*ok, tys),
            },
            _ => ts_ty(*inner, tys),
        },
        _ => "any".to_string(),
    }
}

/// v0.20b: lower a built-in `List` kernel method. Returns None for a method
/// name the kernel doesn't own (the checker has already rejected it; this
/// keeps the dispatch defensive). All forms are pure expressions; `foldEff`
/// returns a Promise that the surrounding `<-` bind awaits.
fn lower_list_kernel(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    elem: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.commons().tys();
    let mut pre = Pre::new();
    let elem_ts = ts_ty(elem, tys);
    let elem_el = ts_array_elem(&elem_ts);
    let text: Option<String> = match (method.name.as_str(), args) {
        ("length", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).length"))
        }
        ("get", [index]) => {
            let recv = pre.lower(receiver, cx);
            let idx = pre.lower(index, cx);
            Some(format!(
                "((__xs: readonly {elem_el}[], __i: number) => __i >= 0 && __i < __xs.length ? Some(__xs[__i] as {elem_ts}) : None)({recv}, {idx})"
            ))
        }
        ("prepend", [head]) => {
            let head = pre.lower(head, cx);
            let recv = pre.lower(receiver, cx);
            Some(format!("[{head}, ...{recv}]"))
        }
        // #1889: the receiver lowers before the argument, as written, so any
        // hoisted effects keep source order.
        ("append", [item]) => {
            let recv = pre.lower(receiver, cx);
            let item = pre.lower(item, cx);
            Some(format!("[...{recv}, {item}]"))
        }
        ("concat", [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("[...{recv}, ...{other}]"))
        }
        ("fold", [init, f]) => {
            // The call's checked type is the accumulator type.
            let acc_ts = cx
                .commons()
                .expr_types
                .get(&e.id)
                .map(|te| ts_ty(te.ty, tys))
                .unwrap_or_else(|| {
                    panic!(
                        "bynk internal error (finding #28): emitter has no recorded type for a \
                     `fold` call at {:?} — the checker should already have typed it",
                        e.span
                    )
                });
            let recv = pre.lower(receiver, cx);
            let init = pre.lower(init, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "((__xs: readonly {elem_el}[], __acc: {acc_ts}, __f: (acc: {acc_ts}, x: {elem_ts}) => {acc_ts}) => {{ for (const __x of __xs) __acc = __f(__acc, __x); return __acc; }})({recv}, {init}, {f})"
            ))
        }
        (FOLD_EFF, [init, f]) => {
            // The call's checked type is `Effect[Acc]` — peel for the TS
            // accumulator annotation.
            let acc_ts = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Effect(acc)) => ts_ty(*acc, tys),
                Some(_) => ts_ty(cx.commons().expr_types[&e.id].ty, tys),
                None => panic!(
                    "bynk internal error (finding #28): emitter has no recorded type for a \
                     `foldEff` call at {:?} — the checker should already have typed it",
                    e.span
                ),
            };
            let recv = pre.lower(receiver, cx);
            let init = pre.lower(init, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__xs: readonly {elem_el}[], __acc: {acc_ts}, __f: (acc: {acc_ts}, x: {elem_ts}) => globalThis.Promise<{acc_ts}>) => {{ for (const __x of __xs) __acc = await __f(__acc, __x); return __acc; }})({recv}, {init}, {f})"
            ))
        }
        // v0.146 (ADR 0170): `forEach` — run an effectful step per element in
        // order, awaiting each; yields `Promise<void>`. The eager `List`
        // analogue of the `Query.forEach` terminal, emitted inline.
        (FOR_EACH, [f]) => {
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__xs: readonly {elem_el}[]) => {{ for (const __x of __xs) {{ await ({f})(__x); }} }})({recv})"
            ))
        }
        // v0.147 (ADR 0171): `parTraverse` — issue the effectful fn over every
        // element concurrently and await them together, so one slow element does
        // not head-of-line-block the rest. The eager `List` analogue of the
        // `Query.parTraverse` terminal, emitted inline.
        (PAR_TRAVERSE, [f]) => {
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__xs: readonly {elem_el}[]) => {{ await globalThis.Promise.all(__xs.map((__x: {elem_ts}) => ({f})(__x))); }})({recv})"
            ))
        }
        // v0.148 (ADR 0172): the collect-all iterators — every element's
        // `Result` outcome is gathered (an `Err` is a value, so nothing rejects).
        // `traverseAll` awaits each in order into a typed array; `parTraverseAll`
        // issues all at once and `Promise.all`s the resolved `Result`s. Both
        // yield `Promise<Result<…>[]>`.
        (TRAVERSE_ALL, [f]) => {
            // The call's checked type is `Effect[List[Result[B, E]]]` — peel to
            // the `Result[B, E]` TS for the output-array annotation.
            let res_ts = match cx.commons().expr_ty(e.id).as_deref() {
                Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
                    Ty::List(el) => ts_ty(*el, tys),
                    _ => ts_ty(*inner, tys),
                },
                other => panic!(
                    "bynk internal error (finding #28): emitter expected `Effect[List[_]]` \
                     recorded for a `traverseAll` call at {:?}, but found {other:?} — the \
                     checker should already have typed it",
                    e.span
                ),
            };
            let res_el = ts_array_elem(&res_ts);
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__xs: readonly {elem_el}[]) => {{ const __out: {res_el}[] = []; for (const __x of __xs) {{ __out.push(await ({f})(__x)); }} return __out; }})({recv})"
            ))
        }
        (PAR_TRAVERSE_ALL, [f]) => {
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__xs: readonly {elem_el}[]) => await globalThis.Promise.all(__xs.map((__x: {elem_ts}) => ({f})(__x))))({recv})"
            ))
        }
        // v0.150 (ADR 0174): the short-circuit collect iterators — stop at the
        // first `Err` and return it (`Effect[Result[List[U], E]]`). `traverseTry`
        // awaits each in order, bailing on the first `Err`; `parTraverseTry`
        // issues all at once, then scans the resolved `Result`s in input order.
        (TRAVERSE_TRY, [f]) => {
            let u_ts = list_ok_elem_ts(cx.commons().expr_types.get(&e.id).map(|te| te.ty), tys);
            let u_el = ts_array_elem(&u_ts);
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__xs: readonly {elem_el}[]) => {{ const __out: {u_el}[] = []; for (const __x of __xs) {{ const __r = await ({f})(__x); if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})({recv})"
            ))
        }
        (PAR_TRAVERSE_TRY, [f]) => {
            let u_ts = list_ok_elem_ts(cx.commons().expr_types.get(&e.id).map(|te| te.ty), tys);
            let u_el = ts_array_elem(&u_ts);
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__xs: readonly {elem_el}[]) => {{ const __rs = await globalThis.Promise.all(__xs.map((__x: {elem_ts}) => ({f})(__x))); const __out: {u_el}[] = []; for (const __r of __rs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})({recv})"
            ))
        }
        // v0.88 (ADR 0116): the eager builder/terminal vocabulary. Most lower
        // to native array methods; callbacks are wrapped in a single-arg arrow
        // so the array index/array extra args never reach a Bynk one-param fn.
        ("map", [f]) => {
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!("({recv}).map((__x: {elem_ts}) => ({f})(__x))"))
        }
        ("filter", [p]) => {
            let recv = pre.lower(receiver, cx);
            let p = pre.lower(p, cx);
            Some(format!("({recv}).filter((__x: {elem_ts}) => ({p})(__x))"))
        }
        ("flatMap", [f]) => {
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!("({recv}).flatMap((__x: {elem_ts}) => ({f})(__x))"))
        }
        ("take", [n]) => {
            let recv = pre.lower(receiver, cx);
            let n = pre.lower(n, cx);
            Some(format!("({recv}).slice(0, globalThis.Math.max(0, {n}))"))
        }
        ("skip", [n]) => {
            let recv = pre.lower(receiver, cx);
            let n = pre.lower(n, cx);
            Some(format!("({recv}).slice(globalThis.Math.max(0, {n}))"))
        }
        ("count", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).length"))
        }
        ("any", [p]) => {
            let recv = pre.lower(receiver, cx);
            let p = pre.lower(p, cx);
            Some(format!("({recv}).some((__x: {elem_ts}) => ({p})(__x))"))
        }
        ("all", [p]) => {
            let recv = pre.lower(receiver, cx);
            let p = pre.lower(p, cx);
            Some(format!("({recv}).every((__x: {elem_ts}) => ({p})(__x))"))
        }
        // v0.119 (ADR 0155, DECISION C-a): `run.upTo(step)` — the driven history
        // strictly before `step`. Steps are distinct object instances in the run
        // array, so `indexOf` is reference identity. An IIFE avoids re-evaluating
        // the receiver.
        ("upTo", [step]) => {
            let recv = pre.lower(receiver, cx);
            let step = pre.lower(step, cx);
            Some(format!(
                "((__xs: readonly {elem_el}[], __s: {elem_ts}) => __xs.slice(0, __xs.indexOf(__s)))({recv}, {step})"
            ))
        }
        ("first", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!(
                "((__xs: readonly {elem_el}[]) => __xs.length > 0 ? Some(__xs[0]) : None)({recv})"
            ))
        }
        ("firstOrElse", [default]) => {
            let recv = pre.lower(receiver, cx);
            let default = pre.lower(default, cx);
            Some(format!(
                "((__xs: readonly {elem_el}[], __d: {elem_ts}) => __xs.length > 0 ? __xs[0] : __d)({recv}, {default})"
            ))
        }
        // v0.88 (ADR 0116 D2/D3/D4): ordering + aggregates. The comparator
        // `<`/`>` works for the numeric- and string-erased orderable keys
        // alike, so no key-type branch is needed (except average's rounding).
        ("sortBy", [key]) => {
            let recv = pre.lower(receiver, cx);
            let key = pre.lower(key, cx);
            Some(format!(
                "[...{recv}].sort((__a: {elem_ts}, __b: {elem_ts}) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? -1 : __ka > __kb ? 1 : 0; }})"
            ))
        }
        ("distinct", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...new globalThis.Set({recv})]"))
        }
        ("distinctBy", [key]) => {
            let recv = pre.lower(receiver, cx);
            let key = pre.lower(key, cx);
            Some(format!(
                "((__xs: readonly {elem_el}[]) => {{ const __seen = new globalThis.Set(); const __out: {elem_el}[] = []; for (const __x of __xs) {{ const __k = ({key})(__x); if (!__seen.has(__k)) {{ __seen.add(__k); __out.push(__x); }} }} return __out; }})({recv})"
            ))
        }
        ("sum", [key]) => {
            let recv = pre.lower(receiver, cx);
            let key = pre.lower(key, cx);
            Some(format!(
                "({recv}).reduce((__s: number, __x: {elem_ts}) => __s + ({key})(__x), 0)"
            ))
        }
        ("min" | "max", [key]) => {
            let cmp = if method.name == "min" { "<" } else { ">" };
            let recv = pre.lower(receiver, cx);
            let key = pre.lower(key, cx);
            Some(format!(
                "((__xs: readonly {elem_el}[]) => {{ if (__xs.length === 0) return None; let __m = ({key})(__xs[0]); for (const __x of __xs) {{ const __k = ({key})(__x); if (__k {cmp} __m) __m = __k; }} return Some(__m); }})({recv})"
            ))
        }
        ("average", [key]) => {
            // D3: Duration averages round to integer millis; Int/Float -> Float.
            let round = matches!(
                cx.commons().expr_ty(e.id).as_deref(),
                Some(Ty::Option(inner)) if matches!(&*tys.get(*inner), Ty::Base(BaseType::Duration))
            );
            let mean = if round {
                "globalThis.Math.round(__s / __xs.length)"
            } else {
                "__s / __xs.length"
            };
            let recv = pre.lower(receiver, cx);
            let key = pre.lower(key, cx);
            Some(format!(
                "((__xs: readonly {elem_el}[]) => {{ if (__xs.length === 0) return None; let __s = 0; for (const __x of __xs) __s += ({key})(__x); return Some({mean}); }})({recv})"
            ))
        }
        // v0.94 (ADR 0116/0120): joins & grouping. Hash on a stringified key
        // (value-keyable, like the `@indexed` posting list), probe, and project
        // each result through `into` — there is no pair value. `joinOn`/`leftJoin`
        // build the hash from `other`'s key; `join` is a nested-loop predicate;
        // `groupBy` partitions in first-seen key order. Group/`into` receive the
        // **original** key (re-derived from a representative row), not the
        // stringified hash key.
        ("joinOn", [other, left, right, into]) => {
            let recv = pre.lower(receiver, cx);
            let u_ts = join_other_elem_ts(args, cx);
            let u_el = ts_array_elem(&u_ts);
            let other = pre.lower(other, cx);
            let left = pre.lower(left, cx);
            let right = pre.lower(right, cx);
            let into = pre.lower(into, cx);
            Some(format!(
                "(() => {{ const __h: globalThis.Record<string, {u_el}[]> = globalThis.Object.create(null); for (const __u of {other}) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return ({recv}).flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.map((__u: {u_ts}) => ({into})(__t, __u)); }}); }})()"
            ))
        }
        ("leftJoin", [other, left, right, into]) => {
            let recv = pre.lower(receiver, cx);
            let u_ts = join_other_elem_ts(args, cx);
            let u_el = ts_array_elem(&u_ts);
            let other = pre.lower(other, cx);
            let left = pre.lower(left, cx);
            let right = pre.lower(right, cx);
            let into = pre.lower(into, cx);
            Some(format!(
                "(() => {{ const __h: globalThis.Record<string, {u_el}[]> = globalThis.Object.create(null); for (const __u of {other}) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return ({recv}).flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.length > 0 ? __m.map((__u: {u_ts}) => ({into})(__t, Some(__u))) : [({into})(__t, None)]; }}); }})()"
            ))
        }
        ("join", [other, on, into]) => {
            let recv = pre.lower(receiver, cx);
            let u_ts = join_other_elem_ts(args, cx);
            let u_el = ts_array_elem(&u_ts);
            let other = pre.lower(other, cx);
            let on = pre.lower(on, cx);
            let into = pre.lower(into, cx);
            Some(format!(
                "(() => {{ const __b: readonly {u_el}[] = {other}; return ({recv}).flatMap((__t: {elem_ts}) => __b.filter((__u: {u_ts}) => ({on})(__t, __u)).map((__u: {u_ts}) => ({into})(__t, __u))); }})()"
            ))
        }
        ("groupBy", [key, into]) => {
            let recv = pre.lower(receiver, cx);
            let key = pre.lower(key, cx);
            let into = pre.lower(into, cx);
            Some(format!(
                "(() => {{ const __h: globalThis.Record<string, {elem_el}[]> = globalThis.Object.create(null); const __order: string[] = []; for (const __t of {recv}) {{ const __k = String(({key})(__t)); if (!(__k in __h)) {{ __h[__k] = []; __order.push(__k); }} __h[__k].push(__t); }} return __order.map((__k) => {{ const __rows = __h[__k]; return ({into})(({key})(__rows[0]), __rows); }}); }})()"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

/// v0.94 (ADR 0120): the TS element type of a join's `other` collection (its
/// `List`/`Query` element), for typing the hash-map buckets. Falls back to
/// `unknown` if the checker recorded no type (an already-diagnosed program).
fn join_other_elem_ts(args: &[Expr], cx: &LowerCtx) -> String {
    let tys = cx.commons().tys();
    match cx.commons().expr_ty(args[0].id).as_deref() {
        Some(Ty::List(u) | Ty::Query(u)) => ts_ty(*u, tys),
        _ => "unknown".to_string(),
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
fn lower_stream_method(source: String, method: &Ident, a: &[String]) -> Option<String> {
    let claim = STREAM_CLAIM;
    match (method.name.as_str(), a) {
        // -- builders (return an AsyncIterable) --
        ("map", [f]) => Some(format!(
            "(async function* (__s) {{ {claim} for await (const __e of __s) {{ yield ({f})(__e); }} }})({source})"
        )),
        ("take", [n]) => Some(format!(
            "(async function* (__s) {{ {claim} const __n = {n}; if (__n <= 0) {{ return; }} let __i = 0; for await (const __e of __s) {{ yield __e; if (++__i >= __n) {{ return; }} }} }})({source})"
        )),
        // -- terminal (drains to a list; Effect-typed) --
        ("collect", []) => Some(format!(
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
/// `elem_ts` is the receiver's own element type, already rendered — resolved
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
/// `other_elem_ts` is `joinOn`/`leftJoin`'s own `other`-side bucket type,
/// computed by every caller via the same `join_other_elem_ts` helper
/// `lower_list_kernel`'s own `joinOn`/`leftJoin`/`join` arms already call —
/// pushed to the call site (rather than this function taking `args`/`cx`
/// itself, `join_other_elem_ts`'s own `args[0]` indexing) to stay under
/// clippy's argument-count lint; every caller already has `args`/`cx` in
/// scope and guards the empty-args case (`distinct`/`collect`/etc. have no
/// `other` to derive one from — `other_elem_ts` is simply unused there).
fn lower_query_method(
    source: String,
    method: &Ident,
    a: &[String],
    result_ty: Option<TyId>,
    elem_ts: &str,
    other_elem_ts: &str,
    tys: &Arc<Types>,
) -> Option<String> {
    let elem_el = ts_array_elem(elem_ts);
    let other_elem_el = ts_array_elem(other_elem_ts);
    let thunk = |body: String| format!("(() => {body})");
    Some(match (method.name.as_str(), a) {
        // -- builders → a deferred thunk over the narrowed source --
        ("filter", [p]) => thunk(format!("{source}.filter((__x) => ({p})(__x))")),
        ("map", [f]) => thunk(format!("{source}.map((__x) => ({f})(__x))")),
        // storage flatMap: the fn returns a `Query` (a thunk) — invoke each.
        ("flatMap", [f]) => thunk(format!("{source}.flatMap((__x) => ({f})(__x)())")),
        ("sortBy", [key]) => thunk(format!(
            "[...{source}].sort((__a, __b) => {{ const __ka = ({key})(__a), __kb = ({key})(__b); return __ka < __kb ? -1 : __ka > __kb ? 1 : 0; }})"
        )),
        ("take", [n]) => thunk(format!("{source}.slice(0, globalThis.Math.max(0, {n}))")),
        ("skip", [n]) => thunk(format!("{source}.slice(globalThis.Math.max(0, {n}))")),
        ("distinct", []) => thunk(format!("[...new globalThis.Set({source})]")),
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
        ("distinctBy", [key]) => thunk(format!(
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
        ("joinOn", [other, left, right, into]) => thunk(format!(
            "{{ const __h: globalThis.Record<string, {other_elem_el}[]> = globalThis.Object.create(null); for (const __u of ({other})()) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return {source}.flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.map((__u: {other_elem_ts}) => ({into})(__t, __u)); }}); }}"
        )),
        ("leftJoin", [other, left, right, into]) => thunk(format!(
            "{{ const __h: globalThis.Record<string, {other_elem_el}[]> = globalThis.Object.create(null); for (const __u of ({other})()) {{ const __k = String(({right})(__u)); (__h[__k] = __h[__k] ?? []).push(__u); }} return {source}.flatMap((__t: {elem_ts}) => {{ const __m = __h[String(({left})(__t))] ?? []; return __m.length > 0 ? __m.map((__u: {other_elem_ts}) => ({into})(__t, Some(__u))) : [({into})(__t, None)]; }}); }}"
        )),
        ("join", [other, on, into]) => thunk(format!(
            "{{ const __b = ({other})(); return {source}.flatMap((__t) => __b.filter((__u) => ({on})(__t, __u)).map((__u) => ({into})(__t, __u))); }}"
        )),
        // Review of #1460: `__h`'s buckets hold `{source}`'s own rows
        // (`elem_ts`), the receiver's element type — same as `distinctBy`
        // above, not `other`-derived (`groupBy` has no `other` argument).
        ("groupBy", [key, into]) => thunk(format!(
            "{{ const __h: globalThis.Record<string, {elem_el}[]> = globalThis.Object.create(null); const __order: string[] = []; for (const __t of {source}) {{ const __k = String(({key})(__t)); if (!(__k in __h)) {{ __h[__k] = []; __order.push(__k); }} __h[__k].push(__t); }} return __order.map((__k) => {{ const __rows = __h[__k]; return ({into})(({key})(__rows[0]), __rows); }}); }}"
        )),
        // -- terminals → read the source array (awaited at the `<-`) --
        ("collect", []) => source,
        ("first", []) => format!(
            "(() => {{ const __a = {source}; return __a.length > 0 ? Some(__a[0]) : None; }})()"
        ),
        ("firstOrElse", [default]) => format!(
            "(() => {{ const __a = {source}; return __a.length > 0 ? __a[0] : ({default}); }})()"
        ),
        ("count", []) => format!("{source}.length"),
        ("fold", [init, f]) => format!(
            "(() => {{ let __acc = {init}; for (const __x of {source}) __acc = ({f})(__acc, __x); return __acc; }})()"
        ),
        ("any", [p]) => format!("{source}.some((__x) => ({p})(__x))"),
        ("all", [p]) => format!("{source}.every((__x) => ({p})(__x))"),
        ("sum", [key]) => format!("{source}.reduce((__s: number, __x) => __s + ({key})(__x), 0)"),
        ("min" | "max", [key]) => {
            let cmp = if method.name == "min" { "<" } else { ">" };
            format!(
                "(() => {{ const __a = {source}; if (__a.length === 0) return None; let __m = ({key})(__a[0]); for (const __x of __a) {{ const __k = ({key})(__x); if (__k {cmp} __m) __m = __k; }} return Some(__m); }})()"
            )
        }
        ("average", [key]) => {
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
            format!(
                "(() => {{ const __a = {source}; if (__a.length === 0) return None; let __s = 0; for (const __x of __a) __s += ({key})(__x); return Some({mean}); }})()"
            )
        }
        ("forEach", [f]) => {
            format!("(async () => {{ for (const __x of {source}) {{ await ({f})(__x); }} }})()")
        }
        // v0.107 (slice 4): the parallel broadcast form — issue the effectful fn over
        // every element concurrently and await them together, so one slow element
        // does not head-of-line-block the rest.
        ("parTraverse", [f]) => {
            format!(
                "(async () => {{ await globalThis.Promise.all({source}.map((__x) => ({f})(__x))); }})()"
            )
        }
        // v0.149 (ADR 0173): the collect-all terminals over a query/broadcast —
        // gather every `Result` outcome (an `Err` is a value, so nothing rejects).
        ("traverseAll", [f]) => {
            let res_ts = match result_ty.map(|t| tys.get(t)).as_deref() {
                Some(Ty::Effect(inner)) => match &*tys.get(*inner) {
                    Ty::List(el) => ts_ty(*el, tys),
                    _ => ts_ty(*inner, tys),
                },
                _ => "any".to_string(),
            };
            let res_el = ts_array_elem(&res_ts);
            format!(
                "(async () => {{ const __out: {res_el}[] = []; for (const __x of {source}) {{ __out.push(await ({f})(__x)); }} return __out; }})()"
            )
        }
        ("parTraverseAll", [f]) => {
            format!(
                "(async () => await globalThis.Promise.all({source}.map((__x) => ({f})(__x))))()"
            )
        }
        // v0.150 (ADR 0174): the short-circuit collect terminals — stop at the
        // first `Err`, returning `Result[List[U], E]`.
        ("traverseTry", [f]) => {
            let u_ts = list_ok_elem_ts(result_ty, tys);
            let u_el = ts_array_elem(&u_ts);
            format!(
                "(async () => {{ const __out: {u_el}[] = []; for (const __x of {source}) {{ const __r = await ({f})(__x); if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})()"
            )
        }
        ("parTraverseTry", [f]) => {
            let u_ts = list_ok_elem_ts(result_ty, tys);
            let u_el = ts_array_elem(&u_ts);
            format!(
                "(async () => {{ const __rs = await globalThis.Promise.all({source}.map((__x) => ({f})(__x))); const __out: {u_el}[] = []; for (const __r of __rs) {{ if (__r.tag === \"Err\") {{ return Err(__r.error); }} __out.push(__r.value); }} return Ok(__out); }})()"
            )
        }
        _ => return None,
    })
}

/// #1685: a store collection is a plain object, and its keys come from user
/// data. A property *set* of `__proto__` hits `Object.prototype`'s accessor (a
/// primitive value is dropped, an object value reassigns the prototype), so
/// every write that may create a key *defines* it instead. Reads of a key that
/// may be absent go through `Object.hasOwn`, never `in` or a bare index, so an
/// inherited name (`constructor`, `toString`) is not read as present. A write to
/// a key already known to be own (after an `Object.hasOwn` check) may stay a set.
fn own_set(obj: &str, key: &str, value: &str) -> String {
    format!(
        "globalThis.Object.defineProperty({obj}, {key}, {{ value: {value}, writable: true, enumerable: true, configurable: true }})"
    )
}

// ---- v0.93 (ADR 0118): `@indexed` secondary-index emission ----------------
//
// For a `store Map[K, V] @indexed(by: f)` field `m`, a sibling state record
// `m__idx_f: Record<string, string[]>` maps a (stringified) field value to the
// primary keys whose value carries it. The mutators maintain it inside the same
// staged commit; an equality `filter` on `f` reads it instead of scanning.

/// Fragment that *removes* `pk` from the posting-list of every indexed field of
/// the value bound to `val_local` (used before overwriting/deleting an entry).
fn idx_unindex(var: &str, map: &str, fields: &[String], val_local: &str, pk: &str) -> String {
    fields
        .iter()
        .map(|f| {
            let idx = format!("{var}.{map}__idx_{f}");
            format!(
                "{{ const __ik = String(({val_local}).{f}); const __ia = globalThis.Object.hasOwn({idx}, __ik) ? {idx}[__ik] : undefined; if (__ia) {{ const __ii = __ia.indexOf({pk}); if (__ii >= 0) __ia.splice(__ii, 1); if (__ia.length === 0) delete {idx}[__ik]; }} }}"
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Fragment that *adds* `pk` to the posting-list of every indexed field of the
/// value bound to `val_local` (used after writing an entry).
fn idx_reindex(var: &str, map: &str, fields: &[String], val_local: &str, pk: &str) -> String {
    fields
        .iter()
        .map(|f| {
            let idx = format!("{var}.{map}__idx_{f}");
            format!(
                "{{ const __ik = String(({val_local}).{f}); if (!globalThis.Object.hasOwn({idx}, __ik)) {}; {idx}[__ik].push({pk}); }}",
                own_set(&idx, "__ik", "[]")
            )
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// `put` with index maintenance: drop the prior value's postings (if any), write,
/// then post the new value. Last-write-wins re-indexing.
fn idx_map_put(m: &str, var: &str, map: &str, fields: &[String], a: &[String]) -> String {
    let un = idx_unindex(var, map, fields, "__o", "__k");
    let re = idx_reindex(var, map, fields, "__v", "__k");
    format!(
        "(() => {{ const __k = String({k}); const __v = {v}; const __o = globalThis.Object.hasOwn({m}, __k) ? {m}[__k] : undefined; if (__o !== undefined) {{ {un} }} {set}; {re} return undefined; }})()",
        k = a[0],
        v = a[1],
        set = own_set(m, "__k", "__v"),
    )
}

/// `remove` with index maintenance: drop the value's postings, then delete.
fn idx_map_remove(m: &str, var: &str, map: &str, fields: &[String], a: &[String]) -> String {
    let un = idx_unindex(var, map, fields, "__o", "__k");
    format!(
        "(() => {{ const __k = String({k}); const __o = globalThis.Object.hasOwn({m}, __k) ? {m}[__k] : undefined; if (__o !== undefined) {{ {un} delete {m}[__k]; }} return undefined; }})()",
        k = a[0],
    )
}

/// `update` with index maintenance: the key must exist (else a fault); re-index
/// from old value to new.
fn idx_map_update(m: &str, var: &str, map: &str, fields: &[String], a: &[String]) -> String {
    let un = idx_unindex(var, map, fields, "__o", "__k");
    let re = idx_reindex(var, map, fields, "__v", "__k");
    format!(
        "(() => {{ const __k = String({k}); if (!globalThis.Object.hasOwn({m}, __k)) {{ throw new globalThis.Error(\"Map.update: key absent\"); }} const __o = {m}[__k]; {un} const __v = ({f})(__o); {m}[__k] = __v; {re} return undefined; }})()",
        k = a[0],
        f = a[1],
    )
}

/// `upsert` with index maintenance: re-index from the prior value (if present)
/// to the computed new value.
fn idx_map_upsert(m: &str, var: &str, map: &str, fields: &[String], a: &[String]) -> String {
    let un = idx_unindex(var, map, fields, "__o", "__k");
    let re = idx_reindex(var, map, fields, "__v", "__k");
    format!(
        "(() => {{ const __k = String({k}); const __e = globalThis.Object.hasOwn({m}, __k); const __o = __e ? {m}[__k] : undefined; if (__e) {{ {un} }} const __v = ({f})(__e ? __o : ({d})); {set}; {re} return undefined; }})()",
        k = a[0],
        d = a[1],
        f = a[2],
        set = own_set(m, "__k", "__v"),
    )
}

/// If `lam` is `(p) => p.<field> == <value>` (either order) where `<field>` is
/// indexed and `<value>` does not mention `p`, lower it to a posting-list lookup
/// thunk `() => idx[v].map(pk => map[pk])`. Otherwise `None` (fall back to scan).
fn route_indexed_filter(
    m: &str,
    var: &str,
    map: &str,
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
    let text: Option<String> = Some(format!(
        "(() => {{ const __s = String({v}); const __ia = {var}.{map}__idx_{field}; return (globalThis.Object.hasOwn(__ia, __s) ? __ia[__s] : []).map((__pk) => {m}[__pk]); }})"
    ));
    text.map(|expr| pre.finish(expr))
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
fn lower_numeric_kernel(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    base: BaseType,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (method.name.as_str(), args) {
        ("toFloat", []) => Some(pre.lower(receiver, cx)),
        ("abs", []) => {
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
        ("round" | "floor" | "ceil" | "truncate", []) => {
            let recv = pre.lower(receiver, cx);
            let f = if method.name == "truncate" {
                "trunc"
            } else {
                method.name.as_str()
            };
            cx.note_int();
            Some(format!(
                "__bynkToInt(globalThis.Math.{f}({recv}), \"{}\")",
                method.name
            ))
        }
        ("min" | "max", [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("globalThis.Math.{}({recv}, {other})", method.name))
        }
        ("clamp", [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "globalThis.Math.min(globalThis.Math.max({recv}, {lo}), {hi})"
            ))
        }
        ("isNaN" | "isFinite", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Number.{}({recv})", method.name))
        }
        // v0.42 (ADR 0074): host number→string — `String(n)` is ECMAScript's
        // Number::toString (shortest round-trip; `1e21`/`Infinity`/`NaN` as the
        // host renders them). The normative contract is the platform's.
        ("toString", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        // K6b (#2598): the `Int` division companions and exact power, through
        // `int.ts` helpers so their fault branches stay out of coverage.
        ("div" | "rem" | "mod" | "pow", [other]) if base == BaseType::Int => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            let helper = match method.name.as_str() {
                "div" => "__bynkIntDivFloor",
                "rem" => "__bynkIntRem",
                "mod" => "__bynkIntMod",
                _ => "__bynkIntPow",
            };
            cx.note_int();
            Some(format!("{helper}({recv}, {other})"))
        }
        // K6b (#2598): the `Float` functions, straight onto JS `Math`.
        (
            "sqrt" | "exp" | "log10" | "log2" | "sin" | "cos" | "tan" | "asin" | "acos" | "atan",
            [],
        ) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.{}({recv})", method.name))
        }
        ("ln", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("globalThis.Math.log({recv})"))
        }
        ("pow", [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("globalThis.Math.pow({recv}, {other})"))
        }
        // `y.atan2(x)`: the angle of the point `(x, y)`.
        ("atan2", [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("globalThis.Math.atan2({recv}, {x})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

/// v0.86 (ADR 0112): lower a `Duration` kernel method. `toMillis` is the
/// identity (a `Duration` lowers to its milliseconds); `toString` renders it.
fn lower_duration_kernel(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (method.name.as_str(), args) {
        ("toMillis", []) => Some(pre.lower(receiver, cx)),
        ("toString", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

/// v0.90 (ADR 0114): lower an `Instant` kernel method. `toEpochMillis` is the
/// identity (an `Instant` lowers to its epoch milliseconds); `toString` renders
/// it.
fn lower_instant_kernel(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (method.name.as_str(), args) {
        ("toEpochMillis", []) => Some(pre.lower(receiver, cx)),
        ("toString", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("String({recv})"))
        }
        // K3 (#2592): RFC 3339 in UTC.
        ("toIso", []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_time_ops();
            Some(format!("__bynkInstantToIso({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

/// v0.110 (ADR 0142 D3/D4): lower a `Bytes` kernel method. `length` is the
/// `Uint8Array.length` (octet count, not any string length); `toBase64` is a
/// total encode; `decodeUtf8` is a guarded fatal decode returning `Option`
/// (`None` on an invalid UTF-8 sequence).
fn lower_bytes_kernel(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    let text: Option<String> = match (method.name.as_str(), args) {
        ("length", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).length"))
        }
        ("toBase64", []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes();
            Some(format!("__bynkBytesToBase64({recv})"))
        }
        ("decodeUtf8", []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes();
            Some(format!("__bynkBytesDecodeUtf8({recv})"))
        }
        // #2016 (standard-library K1). `slice` clamps negative indices to 0 as
        // `String.slice` does, and `Uint8Array.slice` copies, so a slice never
        // aliases a buffer a binding may still hold.
        ("slice", [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "({recv}).slice(globalThis.Math.max(0, {lo}), globalThis.Math.max(0, {hi}))"
            ))
        }
        ("concat", [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesConcat({recv}, {other})"))
        }
        ("compare", [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesCompare({recv}, {other})"))
        }
        ("toHex", []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesToHex({recv})"))
        }
        ("toBase64Url", []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_bytes_ops();
            Some(format!("__bynkBytesToBase64Url({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
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
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let mut pre = Pre::new();
    // K6c (#2600): the checker admits a regex method only with a literal
    // pattern; anything else never reaches here.
    let pattern = match args.first().map(|a| &a.kind) {
        Some(ExprKind::StrLit(literal)) if method.name.ends_with("Regex") => Some(literal.clone()),
        _ => None,
    };
    if let Some(literal) = pattern {
        let recv = pre.lower(receiver, cx);
        let groups = "(__m: globalThis.RegExpMatchArray): Option<string>[] => \
                      [...__m].map((__g: string | undefined): Option<string> => \
                      __g === undefined ? None : Some(__g))";
        let text = match (method.name.as_str(), args.len()) {
            ("matchesRegex", 1) => Some(format!(
                "{}.test({recv})",
                regex_construction(&literal, "", true)
            )),
            ("containsRegex", 1) => Some(format!(
                "{}.test({recv})",
                regex_construction(&literal, "", false)
            )),
            ("findRegex", 1) => Some(format!(
                "((__m: globalThis.RegExpExecArray | null): Option<readonly Option<string>[]> => \
                 __m === null ? None : Some(({groups})(__m)))({}.exec({recv}))",
                regex_construction(&literal, "", false)
            )),
            ("findAllRegex", 1) => Some(format!(
                "([...({recv}).matchAll({})].map({groups}) as readonly (readonly Option<string>[])[])",
                regex_construction(&literal, "g", false)
            )),
            ("replaceRegex", 2) => {
                let with = pre.lower(&args[1], cx);
                Some(format!(
                    "({recv}).replace({}, {with})",
                    regex_construction(&literal, "g", false)
                ))
            }
            // JS `split` with a regex, minus the captured groups it splices in:
            // its own algorithm over a sticky scan, an empty match at the last
            // split point advancing by one code unit.
            ("splitRegex", 1) => Some(format!(
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
        return text.map(|expr| pre.finish(expr));
    }
    let text: Option<String> = match (method.name.as_str(), args) {
        ("length", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).length"))
        }
        ("trim", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.trim()"))
        }
        ("toUpper", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.toUpperCase()"))
        }
        ("toLower", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("{recv}.toLowerCase()"))
        }
        ("chars", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...{recv}]"))
        }
        ("split", [sep]) => {
            let recv = pre.lower(receiver, cx);
            let sep = pre.lower(sep, cx);
            Some(format!("{recv}.split({sep})"))
        }
        ("contains", [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!("{recv}.includes({sub})"))
        }
        ("startsWith" | "endsWith", [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!("{recv}.{}({sub})", method.name))
        }
        ("concat", [other]) => {
            let recv = pre.lower(receiver, cx);
            let other = pre.lower(other, cx);
            Some(format!("{recv}.concat({other})"))
        }
        ("replace", [from, to]) => {
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
        ("slice", [lo, hi]) => {
            let recv = pre.lower(receiver, cx);
            let lo = pre.lower(lo, cx);
            let hi = pre.lower(hi, cx);
            Some(format!(
                "{recv}.slice(globalThis.Math.max(0, {lo}), globalThis.Math.max(0, {hi}))"
            ))
        }
        ("indexOf", [sub]) => {
            let recv = pre.lower(receiver, cx);
            let sub = pre.lower(sub, cx);
            Some(format!(
                "((__i: number) => __i < 0 ? None : Some(__i))({recv}.indexOf({sub}))"
            ))
        }
        // K2 (#2594): percent-encoding over the UTF-8 bytes.
        ("percentEncode", []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_url_ops();
            Some(format!("__bynkPercentEncode({recv})"))
        }
        ("percentDecode", []) => {
            let recv = pre.lower(receiver, cx);
            cx.note_url_ops();
            Some(format!("__bynkPercentDecode({recv})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

/// v0.22a: lower a built-in `Option[T]` kernel method (ADR 0048). Typed
/// IIFEs in the v0.20b posture — no runtime imports beyond the
/// `Some`/`None`/`Ok`/`Err` constructors every module already has.
#[allow(clippy::too_many_arguments)]
fn lower_option_kernel(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    inner: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.commons().tys();
    let mut pre = Pre::new();
    let t = ts_ty(inner, tys);
    let text: Option<String> = match (method.name.as_str(), args) {
        ("map", [f]) => {
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
        ("andThen", [f]) => {
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
        ("getOrElse", [fallback]) => {
            let recv = pre.lower(receiver, cx);
            let fallback = pre.lower(fallback, cx);
            Some(format!(
                "((__o: Option<{t}>, __d: {t}) => __o.tag === \"Some\" ? __o.value : __d)({recv}, {fallback})"
            ))
        }
        ("isSome", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"Some\")"))
        }
        ("okOr", [error]) => {
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
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

/// v0.22a: lower a built-in `Result[T, E]` kernel method (ADR 0048). The
/// miss branches return the narrowed receiver — TS's discriminated-union
/// narrowing makes the `Err` arm assignable to `Result<B, E>` directly.
#[allow(clippy::too_many_arguments)]
fn lower_result_kernel(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.commons().tys();
    let mut pre = Pre::new();
    let t = ts_ty(ok, tys);
    let et = ts_ty(err, tys);
    let text: Option<String> = match (method.name.as_str(), args) {
        ("map", [f]) => {
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
        ("andThen", [f]) => {
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
        ("mapErr", [f]) => {
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
        ("getOrElse", [fallback]) => {
            let recv = pre.lower(receiver, cx);
            let fallback = pre.lower(fallback, cx);
            Some(format!(
                "((__r: Result<{t}, {et}>, __d: {t}) => __r.tag === \"Ok\" ? __r.value : __d)({recv}, {fallback})"
            ))
        }
        ("isOk", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}.tag === \"Ok\")"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
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
    method: &Ident,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.commons().tys();
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
    let text: Option<String> = match (method.name.as_str(), args) {
        ("mapOk", [f]) => {
            // result `Effect[Result[U, E]]` — `a_ts` is `U`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (x: {t}) => {a_ts}) => {{ const __r = await __e; return __r.tag === \"Ok\" ? Ok(__f(__r.value)) : __r; }})({recv}, {f})"
            ))
        }
        ("mapErr", [f]) => {
            // result `Effect[Result[T, F]]` — `b_ts` is `F`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (e: {et}) => {b_ts}) => {{ const __r = await __e; return __r.tag === \"Err\" ? Err(__f(__r.error)) : __r; }})({recv}, {f})"
            ))
        }
        ("flatMapOk", [f]) => {
            // `f: T -> Effect[Result[U, E]]`; result `Effect[Result[U, E]]`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (x: {t}) => globalThis.Promise<Result<{a_ts}, {et}>>) => {{ const __r = await __e; return __r.tag === \"Ok\" ? await __f(__r.value) : __r; }})({recv}, {f})"
            ))
        }
        ("flatMapErr", [f]) => {
            // `f: E -> Effect[Result[T, F]]`; result `Effect[Result[T, F]]`.
            let recv = pre.lower(receiver, cx);
            let f = pre.lower(f, cx);
            Some(format!(
                "(async (__e: globalThis.Promise<Result<{t}, {et}>>, __f: (e: {et}) => globalThis.Promise<Result<{t}, {b_ts}>>) => {{ const __r = await __e; return __r.tag === \"Err\" ? await __f(__r.error) : __r; }})({recv}, {f})"
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

/// v0.20b: lower a built-in `Map` kernel method. `insert` copies — the
/// emitted `ReadonlyMap` is never mutated in place; updating an existing key
/// keeps its insertion position (JS `Map` semantics, normative in §7).
/// K6a (#2596): lower a `Set` kernel method over the `ReadonlySet`. `insert`
/// and `remove` copy, so the receiver is never mutated; a copy keeps the
/// insertion order, and inserting a present element keeps its position.
fn lower_set_kernel(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    elem: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.commons().tys();
    let mut pre = Pre::new();
    let elem_ts = ts_ty(elem, tys);
    let text: Option<String> = match (method.name.as_str(), args) {
        ("length", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).size"))
        }
        ("contains", [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!("({recv}).has({x})"))
        }
        ("insert", [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!(
                "(new globalThis.Set<{elem_ts}>({recv}).add({x}) as ReadonlySet<{elem_ts}>)"
            ))
        }
        ("remove", [x]) => {
            let recv = pre.lower(receiver, cx);
            let x = pre.lower(x, cx);
            Some(format!(
                "((__s: globalThis.Set<{elem_ts}>) => (__s.delete({x}), __s as ReadonlySet<{elem_ts}>))(new globalThis.Set<{elem_ts}>({recv}))"
            ))
        }
        ("toList", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!(
                "([...({recv})] as readonly {}[])",
                ts_array_elem(&elem_ts)
            ))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

fn lower_map_kernel(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    key: TyId,
    val: TyId,
    cx: &mut LowerCtx,
) -> Option<Lowered> {
    let tys = cx.commons().tys();
    let mut pre = Pre::new();
    let key_ts = ts_ty(key, tys);
    let val_ts = ts_ty(val, tys);
    let text: Option<String> = match (method.name.as_str(), args) {
        ("length", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("({recv}).size"))
        }
        ("keys", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...({recv}).keys()]"))
        }
        // v0.149 (ADR 0173): the values in key order — the `keys()` sibling over
        // the in-memory `ReadonlyMap`.
        ("values", []) => {
            let recv = pre.lower(receiver, cx);
            Some(format!("[...({recv}).values()]"))
        }
        ("get", [k]) => {
            let recv = pre.lower(receiver, cx);
            let k = pre.lower(k, cx);
            Some(format!(
                "((__m: ReadonlyMap<{key_ts}, {val_ts}>, __k: {key_ts}) => __m.has(__k) ? Some(__m.get(__k) as {val_ts}) : None)({recv}, {k})"
            ))
        }
        ("insert", [k, v]) => {
            let recv = pre.lower(receiver, cx);
            let k = pre.lower(k, cx);
            let v = pre.lower(v, cx);
            Some(format!("new globalThis.Map({recv}).set({k}, {v})"))
        }
        _ => None,
    };
    text.map(|expr| pre.finish(expr))
}

fn lower_if(
    e: &Expr,
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let slot_ty = checked_ty_ts(e, cx);
    let cond_expr = pre.lower(cond, cx);
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
            return pre.finish(format!(
                "({cond_expr} ? {t} : {e})",
                t = then_tail.expr,
                e = else_tail.expr
            ));
        }
        // A branch hoisted. Before T2.1 there was nowhere to put its statements
        // from here — a value position had no statement slot — and the only
        // shape available was the arrow below, which swallows a hoisted `?`'s
        // early return. `lower_if` now returns its own `pre`, so the `if` hoists
        // as a real statement in the caller's statement position and the
        // `return` keeps exiting the enclosing function. See
        // `hoist_if_as_statement`.
        let value = hoist_if_as_statement(&mut pre, cond_expr, then_tail, else_tail, slot_ty, cx);
        pre.finish(value)
    } else {
        // T2.2 (R6.4): isolate the effectfulness flag to just this IIFE's own
        // body. Unlike `lower_match_as_iife`'s `disc_expr` (spliced *outside*
        // the returned arrow, at the call site), `cond_expr`'s *text* is
        // spliced inside this arrow's own `if (...)` below — only its hoisted
        // statements landed in `pre` above. A bare `await` in `cond_expr`'s
        // text would need to feed `needs_async`, not `saved_await`; the
        // debug_assert after the arrow closes documents that this can't
        // happen today (a literal `await` comes only from `EffectLet`/`Do`,
        // both hoisted to `pre`, or from a nested `finish_async_iife` wrap,
        // which would require a value-position `match`/`if` needing the async
        // wrap to itself type as `Bool` — ruled out by the checker's
        // Effect-typing) rather than silently trusting it.
        let saved_await = std::mem::take(&mut cx.emitted_await);
        debug_assert!(
            !cond_expr.contains("await "),
            "cond text lands inside this arrow; its awaits must feed `needs_async`, not `saved_await`: {cond_expr}"
        );
        let mut iife = String::new();
        iife.push_str("(() => {\n");
        iife.push_str("    if (");
        iife.push_str(&cond_expr);
        iife.push_str(") {\n");
        // Inject is-binding declarations on the truthy side. #908: scoped to
        // the then-branch only — pushed here, popped once it's emitted —
        // so a read inside resolves to the binding rather than an outer
        // `let` rename, and the registration doesn't leak into the else
        // branch or beyond.
        cx.shadow_scopes.push(HashMap::new());
        let mut is_bindings = Vec::new();
        let mut found = false;
        gather_is_bindings_for_emit(cond, true, cx, &mut is_bindings, &mut found);
        for b in &is_bindings {
            for _ in 0..(INDENT_STEP * 3) {
                iife.push(' ');
            }
            iife.push_str(b);
            iife.push('\n');
        }
        // v0.154 (ADR 0178): a value-position `if` lowers to an IIFE, whose
        // `return`s exit the arrow, so clear the enclosing `return_ty`. #1750:
        // an `if` whose branches hold a `?` no longer reaches here (it becomes a
        // statement, `hoist_value_as_statement`, so the `?` exits the function
        // and keeps its embedding), so nothing inside this arrow reads it.
        let saved = cx.return_ty.take();
        // #1750: the arrow's own tails `return` from the arrow.
        let saved_slot = cx.tail_slot.take();
        // #4 review: `iife` is a local buffer spliced into the caller's
        // output later, at an offset `record_span` has no way to learn —
        // see `without_source_map`.
        cx.without_source_map(|cx| {
            emit_block_as_function_body(&mut iife, then_block, cx, INDENT_STEP * 3, false)
        });
        cx.shadow_scopes.pop();
        for _ in 0..(INDENT_STEP * 2) {
            iife.push(' ');
        }
        iife.push_str("} else {\n");
        // #1654: the bindings the condition's falsity proves, scoped to the
        // else-branch the same way.
        cx.shadow_scopes.push(HashMap::new());
        let mut else_bindings = Vec::new();
        gather_is_bindings_for_emit(cond, false, cx, &mut else_bindings, &mut found);
        for b in &else_bindings {
            for _ in 0..(INDENT_STEP * 3) {
                iife.push(' ');
            }
            iife.push_str(b);
            iife.push('\n');
        }
        cx.without_source_map(|cx| {
            emit_block_as_function_body(&mut iife, else_block, cx, INDENT_STEP * 3, false)
        });
        cx.shadow_scopes.pop();
        cx.return_ty = saved;
        cx.tail_slot = saved_slot;
        for _ in 0..(INDENT_STEP * 2) {
            iife.push(' ');
        }
        iife.push_str("}\n");
        for _ in 0..INDENT_STEP {
            iife.push(' ');
        }
        iife.push_str("})()");
        // #2 review: unlike `lower_match_as_iife`'s IIFE, this one used to never
        // consult the async-wrap decision at all — confirmed live: `let r = if
        // c { let y <- fetch(); y + 1 } else { 0 }` emitted a bare `await`
        // inside this arrow with no `async` keyword, a hard JS syntax error
        // ("Unexpected reserved word"), not merely a missed optimisation.
        let needs_async = cx.emitted_await;
        cx.emitted_await = saved_await || needs_async;
        pre.finish(finish_async_iife(iife, needs_async))
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

/// True if the expression contains an `is` test with at least one
/// non-wildcard binding. Walks through `&&`, `||`, and parens.
fn cond_has_is_bindings(e: &Expr, cx: &LowerCtx) -> bool {
    // #1654: either branch may need a place for declarations — the then-branch
    // for the tests the condition proves when true, the else-branch for those
    // it proves when false (`if !(o is Some(v)) { … } else { v }`).
    [true, false].into_iter().any(|when_true| {
        bynk_check::narrowing::matched_is_tests(e, when_true)
            .into_iter()
            .any(|t| match &t.kind {
                ExprKind::Is { value, pattern } => pattern_is_has_bindings(pattern, value, cx),
                _ => false,
            })
    })
}

/// The per-pattern-kind check `cond_has_is_bindings` delegates to (#474: split
/// out so an or-pattern can recurse into its alternatives).
fn pattern_is_has_bindings(pattern: &Pattern, value: &Expr, cx: &LowerCtx) -> bool {
    match pattern {
        Pattern::Variant {
            variant, bindings, ..
        } => {
            bindings.iter().any(|b| !b.is_wildcard())
                // v0.13: a refined `is`-narrowing introduces a (shadow) binding
                // even though the pattern carries none.
                || (bindings.is_empty() && cx.is_refined_is_check(value, &variant.name))
        }
        // #474: Rule 1 guarantees every alternative binds the same names, so
        // any one alternative having bindings means the whole pattern does.
        Pattern::Or(alts, _) => alts
            .iter()
            .any(|alt| pattern_is_has_bindings(alt, value, cx)),
        _ => false,
    }
}

fn emit_if_tail(
    out: &mut String,
    cond: &Expr,
    then_block: &Block,
    else_block: &Block,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    let mut pre = Pre::new();
    let cond_expr = pre.lower(cond, cx);
    for s in pre.stmts() {
        write_line(out, indent, s);
    }
    write_line(out, indent, &format!("if ({cond_expr}) {{"));
    // is-binding declarations on the truthy path. #908: scoped to the
    // then-branch only (see the IIFE sibling above for the same reasoning).
    cx.shadow_scopes.push(HashMap::new());
    let mut is_bindings = Vec::new();
    let mut found = false;
    gather_is_bindings_for_emit(cond, true, cx, &mut is_bindings, &mut found);
    for b in &is_bindings {
        write_line(out, indent + INDENT_STEP, b);
    }
    emit_block_as_function_body(out, then_block, cx, indent + INDENT_STEP, async_tail);
    cx.shadow_scopes.pop();
    write_line(out, indent, "} else {");
    // #1654: the else-branch's bindings, those the condition's falsity proves.
    cx.shadow_scopes.push(HashMap::new());
    let mut else_bindings = Vec::new();
    gather_is_bindings_for_emit(cond, false, cx, &mut else_bindings, &mut found);
    for b in &else_bindings {
        write_line(out, indent + INDENT_STEP, b);
    }
    emit_block_as_function_body(out, else_block, cx, indent + INDENT_STEP, async_tail);
    cx.shadow_scopes.pop();
    write_line(out, indent, "}");
}

fn lower_ident(e: &Expr, id: &Ident, cx: &mut LowerCtx) -> String {
    // v0.116: inside a `transition` predicate, the contextual `old`/`new` idents
    // are the pre-/post-commit state records, lowered to their JS names (`new` is
    // reserved, so both are renamed). Field access `old.<f>` reads off the record.
    // Checked first so `old`/`new` never collide with the heuristics below.
    if let Some((old_var, new_var)) = cx.transition_states() {
        if id.name == "old" {
            return old_var.to_string();
        }
        if id.name == "new" {
            return new_var.to_string();
        }
    }
    // v0.80: inside an invariant predicate, a bare ident naming a state field
    // reads it off the proposed-state value (`s.<field>`). Checked first so a
    // field never collides with the variant-constructor heuristics below.
    if let Some((var, fields)) = cx.invariant_state()
        && fields.contains(&id.name)
    {
        return format!("{var}.{}", id.name);
    }
    // v0.81: inside a `store`-agent handler, a bare ident naming a `Cell` field
    // reads it off the mutable working state (`__state.<cell>`), so a read after
    // a `:=` write in the same handler sees the written value (read-your-writes).
    if let Some((var, cells)) = cx.agent_store_cells()
        && cells.contains(&id.name)
    {
        return format!("{var}.{}", id.name);
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
        return format!(
            "(() => globalThis.Object.values({var}.{name}).flatMap((__cid) => {{ const __c = __resolveConnection<{f_ts}>(this.state, __cid); return __c.tag === \"Some\" ? [__c.value] : []; }}))",
            name = id.name
        );
    }
    // v0.94 (ADR 0120): a bare `store Map` ident used as a **value** — not a
    // method receiver (those are handled in the method dispatch) — is a lazy
    // `Query` over the whole map, e.g. the `other` side of a join. It lowers to
    // the same deferred thunk a query builder yields: `() => Object.values(map)`.
    if !cx.is_local(&id.name) && cx.is_agent_store_map(&id.name) {
        let var = cx.agent_store_var();
        return format!("(() => globalThis.Object.values({var}.{}))", id.name);
    }
    // v0.95 (ADR 0121): a bare `store Log` ident used as a value is a lazy
    // `Query` over its entry values — `() => log.map((__e) => __e.v)`.
    if !cx.is_local(&id.name) && cx.is_agent_store_log(&id.name) {
        let var = cx.agent_store_var();
        return format!("(() => {var}.{}.map((__e) => __e.v))", id.name);
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
        return format!("HttpResult.{}", id.name);
    }
    // v0.44: a nullary QueueResult variant (`Ack`) constructs `QueueResult.Ack`.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == QUEUE_RESULT
    ) {
        return format!("QueueResult.{}", id.name);
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
            return lit;
        }
        return format!("{}.{}", type_name, id.name);
    }
    // v0.52: the multi-actor sum binder is not a runtime local — the resolved
    // actor is threaded through `deps.who` at the boundary wrapper, so the
    // binder ident lowers to it (the tagged union the body `match`es).
    if cx.actor_sum_binder() == Some(id.name.as_str())
        && matches!(cx.commons().expr_ty(e.id).as_deref(), Some(Ty::ActorSum(_)))
    {
        return "deps.who".to_string();
    }
    // #908: a re-`let` of this name (in this or an enclosing block) renamed
    // its emitted binding — resolve to it rather than the natural
    // (now-shadowed) `ts_ident` name.
    cx.resolved_local_name(&id.name)
        .unwrap_or_else(|| ts_ident(&id.name))
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
    let ts_ty = bynk_ts::print_type(&crate::emitter::ts_ty_to_ts_type(
        ty,
        &cx.commons().ty_intern,
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
) -> Vec<String> {
    args.iter()
        .enumerate()
        .map(|(i, a)| {
            if variant == "RawError"
                && i == 0
                && let Some(status) = bynk_syntax::ast::raw_error_status(a)
            {
                status.to_string()
            } else {
                pre.lower(a, cx)
            }
        })
        .collect()
}

fn lower_call(e: &Expr, name: &Ident, args: &[Expr], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    // Bare variant constructor with payload → qualify.
    let args_lowered = lower_http_variant_args(&name.name, args, &mut pre, cx);
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
        return pre.finish(format!(
            "HttpResult.{}({})",
            name.name,
            args_lowered.join(", ")
        ));
    }
    // v0.44: a QueueResult variant call (`Retry(reason)`) → `QueueResult.Retry(...)`.
    if matches!(
        cx.commons().callee(e.id),
        Some(Callee::Intrinsic { ns, .. }) if *ns == QUEUE_RESULT
    ) {
        return pre.finish(format!(
            "QueueResult.{}({})",
            name.name,
            args_lowered.join(", ")
        ));
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
        let key_expr = args_lowered.first().unwrap_or_else(|| {
            panic!(
                "bynk internal error (ADR 0334): Callee::AgentInit records agent construction \
                 `{}(...)` with {} args, but every AgentDecl declares exactly one key field — \
                 the checker already accepted this call",
                name.name,
                args_lowered.len()
            )
        });
        return pre.finish(cx.agent_construct(&name.name, key_expr));
    }
    // P6.21: sum-variant construction is `Callee::Ctor` — reads the sum
    // type's own name and the variant's own tag straight off the checker's
    // classification instead of `call_is_sum_variant`'s own name-matched
    // `sum_name`/`call_name` string comparison against the call's checked
    // type (the same re-derivation this whole `Callee` precedent, P6.0,
    // exists to close).
    if let Some(Callee::Ctor { sum, tag }) = cx.commons().callee(e.id) {
        if let Some(lit) = consumed_variant_literal(e, &sum.name.name, tag, &args_lowered, cx) {
            return pre.finish(lit);
        }
        return pre.finish(format!(
            "{}.{}({})",
            sum.name.name,
            tag,
            args_lowered.join(", ")
        ));
    }
    pre.finish(format!(
        "{}({})",
        ts_ident(&name.name),
        args_lowered.join(", ")
    ))
}

fn lower_bin_op(op: BinOp, lhs: &Expr, rhs: &Expr, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.commons().tys();
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
    if op == BinOp::And
        && let Some(AndWithIs {
            bindings,
            lhs: lhs_lowered,
            rhs: rhs_expr,
            rhs_returns,
            hoisted,
        }) = lower_and_with_is(lhs, rhs, cx)
    {
        pre.extend(hoisted);
        let lhs_expr = pre.absorb(lhs_lowered);
        if bindings.is_empty() {
            return pre.finish(format!("{lhs_expr} && {rhs_expr}"));
        }
        // Every path below emits `rhs` out of line.
        mark_unnarrowed(rhs, cx);
        // T2.3 (R6.3): `bindings` includes rhs's own hoisted statements
        // (`lower_and_with_is`'s doc comment above), so if one of those is a
        // `?`'s propagating early return, the arrow-IIFE below would capture
        // it instead of letting it exit the enclosing function — reuse
        // `hoist_if_as_statement` (built for T2.1's `if`-hoisting) to hoist
        // this `&&` as a real `if` statement instead.
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
        // same `if` that already guards evaluating it.
        if rhs_returns {
            let slot = hoist_if_as_statement(
                &mut pre,
                lhs_expr.clone(),
                Lowered {
                    pre: bindings,
                    expr: rhs_expr,
                },
                Lowered::bare("false"),
                Some("boolean".to_string()),
                cx,
            );
            return pre.finish(format!("{lhs_expr} && {slot}"));
        }
        // Emit:  lhs && (() => { const n = ...; return rhs; })()
        let mut wrap = String::new();
        wrap.push_str(&lhs_expr);
        wrap.push_str(" && ((() => { ");
        for b in &bindings {
            wrap.push_str(b);
            wrap.push(' ');
        }
        wrap.push_str(&format!("return {rhs_expr}; }})())"));
        return pre.finish(wrap);
    }
    // v0.80: `P implies Q` lowers to `(!(P) || Q)`. As with `&&`, an `is` test in
    // the antecedent binds into the consequent (the consequent is only reached
    // when the antecedent holds), so reuse the same is-binding IIFE flow.
    if op == BinOp::Implies
        && let Some(AndWithIs {
            bindings,
            lhs: lhs_lowered,
            rhs: rhs_expr,
            rhs_returns,
            hoisted,
        }) = lower_and_with_is(lhs, rhs, cx)
    {
        pre.extend(hoisted);
        let lhs_expr = pre.absorb(lhs_lowered);
        if bindings.is_empty() {
            return pre.finish(format!("(!({lhs_expr}) || {rhs_expr})"));
        }
        // Every path below emits `rhs` out of line.
        mark_unnarrowed(rhs, cx);
        // #1044 review: same fix as the `And` arm above — `lhs_expr` (the
        // antecedent's `is`-check) must stay in the returned text for a
        // caller's narrowing to hold, so gate `slot`'s computation on
        // `lhs_expr` directly (rhs is only reached when the antecedent
        // holds, same guard as `&&`) rather than folding the negation into
        // the hoisted `if`'s own condition.
        if rhs_returns {
            let slot = hoist_if_as_statement(
                &mut pre,
                lhs_expr.clone(),
                Lowered {
                    pre: bindings,
                    expr: rhs_expr,
                },
                Lowered::bare("true"),
                Some("boolean".to_string()),
                cx,
            );
            return pre.finish(format!("(!({lhs_expr}) || {slot})"));
        }
        let mut wrap = String::new();
        wrap.push_str(&format!("(!({lhs_expr}) || ((() => {{ "));
        for b in &bindings {
            wrap.push_str(b);
            wrap.push(' ');
        }
        wrap.push_str(&format!("return {rhs_expr}; }})()))"));
        return pre.finish(wrap);
    }
    let l = pre.lower(lhs, cx);
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
    // operator as a real `if` statement via `hoist_if_as_statement` (built for
    // T2.1's `if`-hoisting) instead of an expression-position IIFE — the
    // `return` then keeps exiting the enclosing function, and still only runs
    // when the operator actually reaches rhs. An ordinary hoist (no `?`
    // involved) keeps the cheaper arrow form; nothing inside it needs to
    // escape past the arrow, and this is the shape the `947_short_circuit_rhs_hoist`
    // fixture pins.
    if matches!(op, BinOp::And | BinOp::Or | BinOp::Implies) {
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
            pre.extend(cx.is_temp_hoist.take().unwrap_or_default());
        }
        let rhs_returns = cx.emitted_early_return;
        cx.emitted_early_return = saved_early_return || rhs_returns;
        if !r.pre.is_empty() {
            // Both paths below emit `rhs` out of line.
            mark_unnarrowed(rhs, cx);
        }
        if !r.pre.is_empty() && rhs_returns {
            let value = match op {
                BinOp::And => hoist_if_as_statement(
                    &mut pre,
                    l,
                    r,
                    Lowered::bare("false"),
                    Some("boolean".to_string()),
                    cx,
                ),
                BinOp::Or => hoist_if_as_statement(
                    &mut pre,
                    l,
                    Lowered::bare("true"),
                    r,
                    Some("boolean".to_string()),
                    cx,
                ),
                BinOp::Implies => hoist_if_as_statement(
                    &mut pre,
                    format!("!({l})"),
                    Lowered::bare("true"),
                    r,
                    Some("boolean".to_string()),
                    cx,
                ),
                _ => unreachable!("guarded by the outer `matches!`"),
            };
            return pre.finish(value);
        }
        let rhs_text = if r.pre.is_empty() {
            r.expr
        } else {
            let mut s = String::from("(() => { ");
            for p in &r.pre {
                s.push_str(p);
                s.push(' ');
            }
            s.push_str(&format!("return {}; }})()", r.expr));
            s
        };
        return pre.finish(match op {
            BinOp::And => format!("{l} && {rhs_text}"),
            BinOp::Or => format!("{l} || {rhs_text}"),
            // `P implies Q` ≡ `!P || Q` (no `is` bindings in the antecedent —
            // that case returned above via `lower_and_with_is`).
            BinOp::Implies => format!("(!({l}) || {rhs_text})"),
            _ => unreachable!("guarded by the outer `matches!`"),
        });
    }
    let r = pre.lower(rhs, cx);
    let text = if op == BinOp::Div {
        // v0.21: division is operand-typed (ADR 0042) — `Float`
        // true-divides; `Int` keeps truncating. The checker rejects
        // mixed operands, so the left operand decides; a missing
        // type entry falls back to the `Int` (truncating) lowering.
        let lhs_is_float = cx
            .commons()
            .expr_types
            .get(&lhs.id)
            .and_then(|te| te.ty.base(tys))
            == Some(BaseType::Float);
        if lhs_is_float {
            format!("{l} / {r}")
        } else {
            // #1657: `Int` division by zero traps, a runtime fault, rather than
            // producing `Infinity`/`NaN` as an `Int`. A runtime helper rather than
            // an inline guard, so coverage never reports its fault branch.
            cx.note_int();
            format!("__bynkIntDiv({l}, {r})")
        }
    } else if let Some(helper) = matches!(op, BinOp::Eq | BinOp::NotEq)
        .then(|| cx.commons().expr_types.get(&lhs.id).map(|te| te.ty))
        .and_then(|operand| equality_helper(operand, tys, cx))
    {
        // #1652 (runtime-semantics track §3.1): `==` is structural, dispatched by
        // the operand's type, the same operand-typed shape as `Div` above (and
        // `Bytes`, ADR 0142 D4). The checker rejects mixed operands, so the left
        // operand decides. A primitive operand returns `None` here and falls
        // through to host `===`/`!==` below.
        let call = format!("{helper}({l}, {r})");
        if op == BinOp::Eq {
            call
        } else {
            format!("!{call}")
        }
    } else {
        format!("{l} {} {r}", ts_binop(op))
    };
    pre.finish(text)
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
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::JsonError
        | Ty::Actor(_)
        | Ty::ActorSum(_)
        | Ty::Var(_) => EqualityLowering::Structural,
        // Not equality-supporting: the checker rejects `==` on these
        // (`checker/equality.rs`), so no certified program reaches here. The
        // walker is the safe answer if one ever did.
        Ty::Error
        | Ty::Fn { .. }
        | Ty::Effect(_)
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::Connection(_) => EqualityLowering::Structural,
    }
}

fn lower_record_construction(
    type_name: &Ident,
    fields: &[FieldInit],
    cx: &mut LowerCtx,
) -> Lowered {
    let mut pre = Pre::new();
    let mut parts = Vec::new();
    for f in fields {
        match &f.value {
            Some(v) => {
                let val = pre.lower(v, cx);
                parts.push(format!("{}: {}", f.name.name, val));
            }
            None => {
                // Shorthand `{ x }` references the binding `x`; if the binding
                // was renamed (a reserved word), expand to `x: __id_x` — the
                // key is wire format and must keep the source spelling.
                let v = ts_ident(&f.name.name);
                if v == f.name.name {
                    parts.push(f.name.name.clone());
                } else {
                    parts.push(format!("{}: {v}", f.name.name));
                }
            }
        }
    }
    let _ = type_name;
    pre.finish(format!("{{ {} }}", parts.join(", ")))
}

fn lower_field_access(e: &Expr, receiver: &Expr, field: &Ident, cx: &mut LowerCtx) -> Lowered {
    let tys = cx.commons().tys();
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
        return pre.finish(match field.name.as_str() {
            map_query::VALUES => format!("(() => globalThis.Object.values({m}))"),
            map_query::KEYS => {
                let decoded = decode_map_key(map_key_ty(e, cx), "__k", tys);
                if decoded == "__k" {
                    format!("(() => globalThis.Object.keys({m}))")
                } else {
                    format!("(() => globalThis.Object.keys({m}).map((__k) => {decoded}))")
                }
            }
            // entries
            _ => {
                let decoded = decode_map_key(map_key_ty(e, cx), "__k", tys);
                format!(
                    "(() => globalThis.Object.entries({m}).map(([__k, __v]) => ({{ key: {decoded}, value: __v }})))"
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
        return pre.finish(format!("HttpResult.{}", field.name));
    }
    // #2073 (with #2045): a qualified nullary variant of a consumed context's
    // sum (`Region.Domestic`), on `workers`, is built inline as its bare form
    // is, so the callee's module stays a type-only import.
    if let ExprKind::Ident(id) = &receiver.kind
        && !cx.is_local(&id.name)
        && let Some(lit) = consumed_variant_literal(e, &id.name, &field.name, &[], cx)
    {
        return pre.finish(lit);
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
        return pre.finish("this.__key".to_string());
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
            return pre.finish("deps.identity".to_string());
        }
        return pre.finish("undefined".to_string());
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
        return pre.finish("deps.claims".to_string());
    }
    let r = pre.lower(receiver, cx);
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
        return pre.finish(format!("({r} as {})", ts_base(*base)));
    }
    pre.finish(format!("{r}.{}", field.name))
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
    let tys = cx.commons().tys();
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
fn decode_map_key(k: Option<TyId>, raw: &str, tys: &Arc<Types>) -> String {
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

    /// One table for this module's fixtures — `Types` is `Send + Sync`
    /// (T3.6b), so a `LazyLock` static serves every test without threading.
    static TYS: std::sync::LazyLock<Arc<Types>> =
        std::sync::LazyLock::new(|| Arc::new(Types::new()));

    #[test]
    fn int_key_is_parsed_with_number() {
        let int = TYS.intern(Ty::Base(BaseType::Int));
        assert_eq!(decode_map_key(Some(int), "k", &TYS), "globalThis.Number(k)");
    }

    #[test]
    fn string_key_passes_through() {
        let string = TYS.intern(Ty::Base(BaseType::String));
        assert_eq!(decode_map_key(Some(string), "k", &TYS), "k");
    }

    #[test]
    fn refined_int_key_is_parsed_with_number() {
        let refined = TYS.intern(Ty::Named {
            name: "UserId".to_string(),
            kind: NamedKind::Refined(BaseType::Int),
            args: Vec::new(),
        });
        assert_eq!(
            decode_map_key(Some(refined), "k", &TYS),
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
        assert_eq!(decode_map_key(Some(opaque), "k", &TYS), "(k as Slug)");
    }

    // #70/#71 review: a non-`String`/`Int` base reaching here means the
    // value-keyable set widened elsewhere (`resolver.rs`/`validate.rs`)
    // without teaching this decode the new type's wire shape — it must fail
    // loudly, not silently emit a wrong pass-through.
    #[test]
    #[should_panic(expected = "is not value-keyable")]
    fn a_widened_non_string_int_base_panics_instead_of_silently_passing_through() {
        let b = TYS.intern(Ty::Base(BaseType::Bool));
        decode_map_key(Some(b), "k", &TYS);
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

fn lower_lambda(e: &Expr, lambda: &LambdaExpr, cx: &mut LowerCtx) -> String {
    let tys = cx.commons().tys();
    let is_async = matches!(
        cx.commons().expr_ty(e.id).as_deref(),
        Some(bynk_check::checker::Ty::Fn { ret, .. }) if ret.is_effect(tys)
    );
    let prefix = if is_async { "async " } else { "" };
    let params: Vec<String> = lambda
        .params
        .iter()
        .map(|p| match &p.type_ref {
            Some(tr) => format!("{}: {}", ts_ident(&p.name.name), ts_type_ref(tr)),
            None => ts_ident(&p.name.name),
        })
        .collect();
    let params = params.join(", ");
    // #908: a param's name is the lambda body's own binding — push a frame and
    // register each so a body read resolves to the param rather than an
    // outer `let` rename, popped once the body is lowered so the
    // registration doesn't leak past the lambda.
    cx.shadow_scopes.push(HashMap::new());
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
    let result = match &lambda.body.kind {
        ExprKind::Block(b) => {
            let mut out = format!("{prefix}({params}) => {{\n");
            // #4 review: `out` is a local buffer spliced into the caller's
            // output later — see `without_source_map`.
            cx.without_source_map(|cx| {
                emit_block_as_function_body(&mut out, b, cx, INDENT_STEP * 2, is_async)
            });
            for _ in 0..INDENT_STEP {
                out.push(' ');
            }
            out.push('}');
            out
        }
        _ => {
            let lowered = lower_expr(&lambda.body, cx);
            let body_stmts = lowered.pre;
            let body = lowered.expr;
            if body_stmts.is_empty() {
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
            }
        }
    };
    cx.is_temp_hoist = saved_hoist;
    cx.tail_slot = saved_slot;
    cx.return_ty = saved;
    cx.shadow_scopes.pop();
    result
}

fn lower_record_spread(base: &Expr, overrides: &[FieldInit], cx: &mut LowerCtx) -> Lowered {
    let mut pre = Pre::new();
    let base_expr = pre.lower(base, cx);
    let mut parts = vec![format!("...{base_expr}")];
    for f in overrides {
        match &f.value {
            Some(v) => {
                let val = pre.lower(v, cx);
                parts.push(format!("{}: {}", f.name.name, val));
            }
            None => {
                // Shorthand `{ x }` references the binding `x`; if the binding
                // was renamed (a reserved word), expand to `x: __id_x` — the
                // key is wire format and must keep the source spelling.
                let v = ts_ident(&f.name.name);
                if v == f.name.name {
                    parts.push(f.name.name.clone());
                } else {
                    parts.push(format!("{}: {v}", f.name.name));
                }
            }
        }
    }
    pre.finish(format!("{{ {} }}", parts.join(", ")))
}

fn lower_block_as_expr(b: &Block, cx: &mut LowerCtx) -> String {
    // #1750: the arrow's own tails `return` from the arrow.
    let saved_slot = cx.tail_slot.take();
    let mut iife = String::new();
    iife.push_str("(() => {\n");
    // IIFE is a synchronous arrow function; the surrounding expression context
    // expects a concrete value, so `Effect.pure(...)` must still wrap as
    // `Promise.resolve(...)`.
    // v0.154 (ADR 0178): a `return` here exits the arrow, not the function, so
    // clear `return_ty`. #1750: a block holding a `?` no longer reaches here
    // (`hoist_value_as_statement`).
    let saved = cx.return_ty.take();
    // #4 review: `iife` is a local buffer spliced into the caller's output
    // later — see `without_source_map`.
    cx.without_source_map(|cx| {
        emit_block_as_function_body(&mut iife, b, cx, INDENT_STEP * 2, false)
    });
    cx.return_ty = saved;
    cx.tail_slot = saved_slot;
    for _ in 0..INDENT_STEP {
        iife.push(' ');
    }
    iife.push_str("})()");
    iife
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
    let disc = pre.lower(discriminant, cx);
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
    // #4 review: `build_match_iife` accumulates into its own local buffer,
    // spliced into the caller's output later — see `without_source_map`.
    let built = cx.without_source_map(|cx| build_match_iife(&disc, &disc_ty, arms, cx));
    let needs_async = cx.emitted_await;
    cx.emitted_await = saved_await || needs_async;
    let inner_iife = finish_async_iife(built, needs_async);
    cx.return_ty = saved;
    cx.tail_slot = saved_slot;
    pre.finish(inner_iife)
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

fn build_match_iife(
    disc_expr: &str,
    disc_ty: &Option<TyId>,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
) -> String {
    let tys = cx.commons().tys();
    let mut out = String::new();
    out.push_str("((__d) => {\n");
    // ADR 0169: nested/guarded matches lower to an if-chain (which emits its own
    // per-arm bodies and trailing `throw`); flat/unguarded matches keep the switch.
    if bynk_ir::match_needs_if_chain(arms) {
        emit_match_if_chain(&mut out, "__d", disc_ty, arms, cx, INDENT_STEP * 2, false);
    } else {
        for _ in 0..(INDENT_STEP * 2) {
            out.push(' ');
        }
        // v0.130: literal-kind matches switch on the value; variant-kind on `.tag`.
        let scrutinee = if is_literal_match(disc_ty, tys) {
            "__d"
        } else {
            "__d.tag"
        };
        out.push_str(&format!("switch ({scrutinee}) {{\n"));
        for arm in arms {
            // IIFE form (non-tail match expression): `Effect.pure(...)` must keep
            // its `Promise.resolve` wrapper because the IIFE is a synchronous arrow.
            emit_match_case(&mut out, "__d", disc_ty, arm, cx, INDENT_STEP * 3, false);
        }
        for _ in 0..(INDENT_STEP * 2) {
            out.push(' ');
        }
        out.push_str("}\n");
        for _ in 0..(INDENT_STEP * 2) {
            out.push(' ');
        }
        out.push_str("throw new globalThis.Error(\"non-exhaustive match\");\n");
    }
    for _ in 0..INDENT_STEP {
        out.push(' ');
    }
    out.push_str(&format!("}})({disc_expr})"));
    out
}

/// v0.130: whether a `match` is literal-kind — its scrutinee is a primitive
/// `Int`/`String`/`Bool` (or a refinement over one), so arms are literal
/// patterns and the emitter switches on the *value* itself, not on a `.tag`
/// discriminant. Mirrors the checker's `literal_base_of` classification.
fn is_literal_match(disc_ty: &Option<TyId>, tys: &Arc<Types>) -> bool {
    match disc_ty.map(|t| tys.get(t)).as_deref() {
        Some(Ty::Base(b)) => matches!(b, BaseType::Int | BaseType::String | BaseType::Bool),
        Some(Ty::Named {
            kind: NamedKind::Refined(b),
            ..
        }) => matches!(b, BaseType::Int | BaseType::String | BaseType::Bool),
        _ => false,
    }
}

/// v0.130: render a literal pattern's value as a JS `switch` case label.
fn literal_case_label(value: &LiteralValue) -> String {
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

/// Whether an expression lowers to a stable reference TypeScript can narrow
/// across a `switch` (a variable or a property path rooted in one). Calls and
/// other computed expressions are not narrowable and must be bound to a temp.
fn is_narrowable_path(e: &Expr) -> bool {
    match &e.kind {
        ExprKind::Ident(_) => true,
        ExprKind::FieldAccess { receiver, .. } => is_narrowable_path(receiver),
        ExprKind::Paren(inner) => is_narrowable_path(inner),
        _ => false,
    }
}

fn emit_match_tail(
    out: &mut String,
    discriminant: &Expr,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    let tys = cx.commons().tys();
    // Anchor the discriminant lowering + `switch (…) {` header to the match's
    // scrutinee span (slice 1); each arm re-anchors to its own span below.
    cx.record_span(out.len(), discriminant.span);
    let mut pre = Pre::new();
    let mut disc = pre.lower(discriminant, cx);
    let disc_ty = cx
        .commons()
        .expr_types
        .get(&discriminant.id)
        .map(|te| te.ty);
    for s in pre.stmts() {
        write_line(out, indent, s);
    }
    // v0.9.2: a statement-position `switch` narrows the scrutinee only when it
    // is a stable reference (a variable or property path). A call discriminant
    // such as `ShortCode.of(raw)` is re-evaluated per arm and TypeScript cannot
    // narrow it (and re-evaluation could repeat side effects), so bind it to a
    // fresh temp once and switch on that.
    if !is_narrowable_path(discriminant) {
        let tmp = cx.fresh();
        write_line(out, indent, &format!("const {tmp} = {disc};"));
        disc = tmp;
    }
    // ADR 0169: nested/guarded matches lower to an if-chain; flat/unguarded
    // matches keep the `switch`.
    if bynk_ir::match_needs_if_chain(arms) {
        emit_match_if_chain(out, &disc, &disc_ty, arms, cx, indent, async_tail);
        return;
    }
    // v0.130: literal-kind matches switch on the value; variant-kind on `.tag`.
    let scrutinee = if is_literal_match(&disc_ty, tys) {
        disc.clone()
    } else {
        format!("{disc}.tag")
    };
    write_line(out, indent, &format!("switch ({scrutinee}) {{"));
    for arm in arms {
        emit_match_case(
            out,
            &disc,
            &disc_ty,
            arm,
            cx,
            indent + INDENT_STEP,
            async_tail,
        );
    }
    write_line(out, indent, "}");
    write_line(
        out,
        indent,
        "throw new globalThis.Error(\"non-exhaustive match\");",
    );
}

fn emit_match_case(
    out: &mut String,
    disc_var: &str,
    disc_ty: &Option<TyId>,
    arm: &MatchArm,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    let tys = cx.commons().tys();
    // Anchor this arm's `case`/binding/`return` lines to the arm's source span
    // (slice 1, ADR 0103 D2) — so stepping a `match` walks arm-to-arm.
    cx.record_span(out.len(), arm.span);
    // #908: this arm's payload/catch-all bindings live in the `case`'s own JS
    // block, so a read inside the body must resolve to them rather than an
    // outer `let` rename — push a frame and register each below, popped once
    // the whole arm (bindings + body) is emitted.
    cx.shadow_scopes.push(HashMap::new());
    match &arm.pattern {
        Pattern::Wildcard(_) => {
            write_line(out, indent, "default: {");
            emit_match_body(out, &arm.body, cx, indent + INDENT_STEP, async_tail);
            write_line(out, indent, "}");
        }
        // A bare name binding is a catch-all that binds the scrutinee (ADR 0169).
        Pattern::Binding(id) => {
            write_line(out, indent, "default: {");
            let local = cx.bind_local_name(&id.name);
            write_line(
                out,
                indent + INDENT_STEP,
                &format!("const {local} = {disc_var};"),
            );
            emit_match_body(out, &arm.body, cx, indent + INDENT_STEP, async_tail);
            write_line(out, indent, "}");
        }
        // v0.130: a literal pattern lowers to `case <literal>:`. JS `switch`
        // compares with `===`, matching the value semantics we want for
        // `Int`/`String`/`Bool`.
        Pattern::Literal { value, .. } => {
            write_line(
                out,
                indent,
                &format!("case {}: {{", literal_case_label(value)),
            );
            emit_match_body(out, &arm.body, cx, indent + INDENT_STEP, async_tail);
            write_line(out, indent, "}");
        }
        // #472: a refined arm always trips `match_needs_if_chain`, so the
        // switch path (this function) never sees one in practice — a
        // predicate has no `case`-label lowering, unlike a literal or a tag.
        Pattern::Refined { .. } => {
            unreachable!("a Pattern::Refined arm always routes through emit_match_if_chain")
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            write_line(
                out,
                indent,
                &format!("case \"{tag}\": {{", tag = variant.name),
            );
            // The switch path only handles flat, unguarded arms (nested/guarded
            // matches take the if-chain path), so each payload binding is an
            // irrefutable name or `_`.
            for (i, b) in bindings.iter().enumerate() {
                let Pattern::Binding(name) = b.pattern() else {
                    continue;
                };
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(*disc_ty, &variant.name, i, tys)
                    }
                };
                let local = cx.bind_local_name(&name.name);
                write_line(
                    out,
                    indent + INDENT_STEP,
                    &format!("const {local} = {disc_var}.{field};"),
                );
            }
            emit_match_body(out, &arm.body, cx, indent + INDENT_STEP, async_tail);
            write_line(out, indent, "}");
        }
        // #474: a bindingless or-pattern (guaranteed by `match_needs_if_chain`
        // routing — a bound/guarded one takes the if-chain path instead) lowers
        // to one fall-through `case` label per alternative, sharing one body —
        // *unless* an alternative is `_`/a bare binding (#825 review): that
        // alone already matches everything (`Pending | _` is exactly as
        // irrefutable as a bare `_`, per `Pattern::is_irrefutable`), and JS
        // `switch` has no `case _:` form, only `default:`. Emitting case
        // labels for the other (now-redundant) alternatives and dropping the
        // wildcard produced malformed, `tsc`-rejected output.
        Pattern::Or(alts, _) if arm.pattern.is_irrefutable() => {
            write_line(out, indent, "default: {");
            emit_match_body(out, &arm.body, cx, indent + INDENT_STEP, async_tail);
            write_line(out, indent, "}");
        }
        Pattern::Or(alts, _) => {
            let last = alts.len() - 1;
            for (i, alt) in alts.iter().enumerate() {
                let label = match alt {
                    Pattern::Literal { value, .. } => literal_case_label(value),
                    Pattern::Variant { variant, .. } => format!("\"{}\"", variant.name),
                    Pattern::Wildcard(_) | Pattern::Binding(_) => unreachable!(
                        "pattern.is_irrefutable() is false, so no alternative is a wildcard or binding"
                    ),
                    Pattern::Or(..) => unreachable!("an Or's alternatives are always leaves"),
                    // #472/#474: the parser only ever wraps a *whole*,
                    // already-folded `|`-chain in `Refined` (never the other
                    // way — see `parse_pattern_where_suffix`), so an `Or`'s
                    // alternatives (built from `parse_pattern_base`) can never
                    // themselves be `Refined`.
                    Pattern::Refined { .. } => {
                        unreachable!("an Or's alternatives are never a Refined pattern")
                    }
                };
                if i == last {
                    write_line(out, indent, &format!("case {label}: {{"));
                } else {
                    write_line(out, indent, &format!("case {label}:"));
                }
            }
            emit_match_body(out, &arm.body, cx, indent + INDENT_STEP, async_tail);
            write_line(out, indent, "}");
        }
    }
    cx.shadow_scopes.pop();
}

fn emit_match_body(
    out: &mut String,
    body: &MatchBody,
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    match body {
        MatchBody::Expr(e) => {
            let mut pre = Pre::new();
            let v = pre.absorb(lower_tail_expr(e, cx, async_tail));
            for s in pre.stmts() {
                write_line(out, indent, s);
            }
            write_line(out, indent, &tail_stmt(cx, &v));
        }
        MatchBody::Block(b) => emit_block_as_function_body(out, b, cx, indent, async_tail),
    }
}

/// Emit the boolean tests that must hold for `pattern` to match the value at
/// runtime `path` (static type `path_ty`). Irrefutable patterns add nothing;
/// nested variant/literal payloads recurse into `path.<field>`.
fn pattern_match_tests(
    path: &str,
    path_ty: Option<TyId>,
    pattern: &Pattern,
    cx: &LowerCtx,
    tests: &mut Vec<String>,
) {
    let tys = cx.commons().tys();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Binding(_) => {}
        Pattern::Literal { value, .. } => {
            tests.push(format!("{path} === {}", literal_case_label(value)));
        }
        // #472: `p where predicate` — the inner pattern's own tests, then the
        // predicate as a boolean guard over the runtime value at `path`,
        // reusing `refined_check_as_bool` verbatim (the same helper `value is
        // RefinedType` lowers through).
        Pattern::Refined {
            inner, predicate, ..
        } => {
            pattern_match_tests(path, path_ty, inner, cx, tests);
            // The checker (`check_pattern`) already rejected any scrutinee
            // that isn't literal-kind before this point, so `path_ty` is
            // always `Int` or `String` here for a well-typed program — a
            // silent fallback would mask a real bug (wrong predicate codegen
            // against the wrong base) rather than surfacing it loudly, the
            // same posture as `emit_match_case`'s `Pattern::Refined` arm.
            let base = path_ty
                .and_then(|t| literal_base_of_ty(t, tys))
                .unwrap_or_else(|| {
                    panic!(
                        "a refined pattern's scrutinee must be literal-kind (Int/String), got {:?}",
                        path_ty
                    )
                });
            tests.push(refined_check_as_bool(path, base, Some(predicate)));
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            tests.push(format!("{path}.tag === \"{}\"", variant.name));
            for (i, b) in bindings.iter().enumerate() {
                let sp = b.pattern();
                if sp.is_irrefutable() {
                    continue;
                }
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys)
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys);
                pattern_match_tests(&format!("{path}.{field}"), field_ty, sp, cx, tests);
            }
        }
        // #474: an or-pattern matches `path` when any alternative does — AND
        // each alternative's own tests, then OR the alternatives together, and
        // push the whole disjunction as a single combined test (so a caller
        // AND-joining `tests` composes it correctly with sibling tests, e.g. a
        // nested `Some(1 | 2)`'s outer `.tag === "Some"` test).
        Pattern::Or(alts, _) => {
            let terms: Vec<String> = alts
                .iter()
                .map(|alt| {
                    let mut t = Vec::new();
                    pattern_match_tests(path, path_ty, alt, cx, &mut t);
                    if t.is_empty() {
                        "true".to_string()
                    } else {
                        t.join(" && ")
                    }
                })
                .collect();
            tests.push(format!("({})", terms.join(" || ")));
        }
    }
}

/// #1187's slice 5 (the `Service` emitter cutover): the JS boolean guard for
/// a `from Events(E { field: value, .. })` subscription filter, AND-joining
/// one test per listed field — reads [`bynk_ir::EventPatternIr`] (already
/// resolved by [`bynk_lower::lower_protocol_ir`]) rather than the raw
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
/// recursing through nested payloads (ADR 0169).
fn emit_pattern_bindings(
    out: &mut String,
    indent: usize,
    path: &str,
    path_ty: Option<TyId>,
    pattern: &Pattern,
    cx: &mut LowerCtx,
) {
    let tys = cx.commons().tys();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Literal { .. } => {}
        Pattern::Binding(id) => {
            let local = cx.bind_local_name(&id.name);
            write_line(out, indent, &format!("const {local} = {path};"));
        }
        // #472: a refined pattern binds only what its inner pattern binds
        // (`_ where P` binds nothing today).
        Pattern::Refined { inner, .. } => {
            emit_pattern_bindings(out, indent, path, path_ty, inner, cx);
        }
        Pattern::Variant {
            variant, bindings, ..
        } => {
            for (i, b) in bindings.iter().enumerate() {
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys)
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys);
                emit_pattern_bindings(
                    out,
                    indent,
                    &format!("{path}.{field}"),
                    field_ty,
                    b.pattern(),
                    cx,
                );
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
        // (built by `pattern_match_tests`) has already passed.
        Pattern::Or(alts, _) => {
            let names = pattern.bound_names();
            if names.is_empty() {
                return;
            }
            let decl: Vec<String> = names
                .iter()
                .map(|id| cx.bind_local_name(&id.name))
                .collect();
            write_line(out, indent, &format!("let {};", decl.join(", ")));
            let last = alts.len() - 1;
            for (i, alt) in alts.iter().enumerate() {
                if i == last {
                    write_line(out, indent, "} else {");
                } else {
                    let mut t = Vec::new();
                    pattern_match_tests(path, path_ty, alt, cx, &mut t);
                    let cond = if t.is_empty() {
                        "true".to_string()
                    } else {
                        t.join(" && ")
                    };
                    let kw = if i == 0 { "if" } else { "} else if" };
                    write_line(out, indent, &format!("{kw} ({cond}) {{"));
                }
                let mut pairs = Vec::new();
                pattern_binding_paths(path, path_ty, alt, cx, &mut pairs);
                for (name, p) in &pairs {
                    let local = cx
                        .resolved_local_name(name)
                        .unwrap_or_else(|| ts_ident(name));
                    write_line(out, indent + INDENT_STEP, &format!("{local} = {p};"));
                }
            }
            write_line(out, indent, "}");
        }
    }
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
    let tys = cx.commons().tys();
    match pattern {
        Pattern::Wildcard(_) | Pattern::Literal { .. } => {}
        Pattern::Binding(id) => out.push((id.name.clone(), path.to_string())),
        Pattern::Variant {
            variant, bindings, ..
        } => {
            for (i, b) in bindings.iter().enumerate() {
                let field = match &b.kind {
                    PatternBindingKind::Named { field, .. } => payload_prop(&field.name),
                    PatternBindingKind::Positional { .. } => {
                        cx.positional_field_name(path_ty, &variant.name, i, tys)
                    }
                };
                let field_ty = cx.payload_field_ty(path_ty, &variant.name, i, tys);
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
    }
}

/// Lower a nested/guarded match to a sequence of independent `if` blocks
/// (ADR 0169, DECISION E). Each arm tests its structural pattern; on a match it
/// binds names, then (if guarded) tests the guard, then runs its body — whose
/// tail `return` short-circuits the remaining arms. A guard failing falls
/// through to the next arm, which a `switch` cannot express. Per-arm span
/// anchoring is preserved (ADR 0103).
fn emit_match_if_chain(
    out: &mut String,
    disc_var: &str,
    disc_ty: &Option<TyId>,
    arms: &[MatchArm],
    cx: &mut LowerCtx,
    indent: usize,
    async_tail: bool,
) {
    // An unguarded irrefutable arm is a catch-all: its body always returns, so
    // the trailing non-exhaustive `throw` would be unreachable (tsc rejects it).
    let has_catchall = arms
        .iter()
        .any(|a| a.guard.is_none() && a.pattern.is_irrefutable());
    for arm in arms {
        cx.record_span(out.len(), arm.span);
        // #908: this arm's bindings must resolve within its own body rather
        // than falling through to an outer `let` rename — see the identical
        // reasoning in `emit_match_case`. Pushed/popped once per arm
        // regardless of `has_tests` below, since the concern is which
        // binding a read resolves to, not whether the arm also happens to
        // emit a wrapping JS block.
        cx.shadow_scopes.push(HashMap::new());
        let mut tests = Vec::new();
        pattern_match_tests(disc_var, *disc_ty, &arm.pattern, cx, &mut tests);
        let has_tests = !tests.is_empty();
        if has_tests {
            write_line(out, indent, &format!("if ({}) {{", tests.join(" && ")));
        }
        let body_indent = if has_tests {
            indent + INDENT_STEP
        } else {
            indent
        };
        emit_pattern_bindings(out, body_indent, disc_var, *disc_ty, &arm.pattern, cx);
        if let Some(guard) = &arm.guard {
            let guard_lowered = lower_expr(guard, cx);
            let gv = guard_lowered.expr;
            for s in &guard_lowered.pre {
                write_line(out, body_indent, s);
            }
            write_line(out, body_indent, &format!("if ({gv}) {{"));
            emit_match_body(out, &arm.body, cx, body_indent + INDENT_STEP, async_tail);
            write_line(out, body_indent, "}");
        } else {
            emit_match_body(out, &arm.body, cx, body_indent, async_tail);
        }
        if has_tests {
            write_line(out, indent, "}");
        }
        cx.shadow_scopes.pop();
    }
    if !has_catchall {
        write_line(
            out,
            indent,
            "throw new globalThis.Error(\"non-exhaustive match\");",
        );
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
        return pre.finish(refined_check_as_bool(&recv, base, refinement.as_ref()));
    }
    let v = pre.absorb(cx.is_receiver_ref(value));
    let text = match pattern {
        Pattern::Wildcard(_) => "true".to_string(),
        // A bare name binding after `is` matches anything over a sum (ADR 0169).
        Pattern::Binding(_) => "true".to_string(),
        // v0.130 (DECISION F): the checker rejects a literal on the RHS of `is`,
        // so this is unreachable for a well-typed program; lower it to the
        // value-equality it would mean, defensively.
        Pattern::Literal { value, .. } => {
            format!("{v} === {}", literal_case_label(value))
        }
        // #472: the checker rejects a refined pattern on the RHS of `is`
        // (D5), so this is unreachable for a well-typed program; lower it
        // defensively via the same `pattern_match_tests` a `match` arm uses
        // (which already handles `Pattern::Refined` — inner tests plus the
        // predicate via `refined_check_as_bool`).
        Pattern::Refined { .. } => {
            let scrut_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
            let mut tests = Vec::new();
            pattern_match_tests(&v, scrut_ty, pattern, cx, &mut tests);
            if tests.is_empty() {
                "true".to_string()
            } else {
                tests.join(" && ")
            }
        }
        // #705: a variant pattern lowers to its full structural test — the outer
        // tag *and* any nested refutable payload patterns, so
        // `r is Rejected(RefinementViolation(_))` checks both levels (the same
        // tests a `match` arm emits, via `pattern_match_tests`). A plain
        // `is Ok(_)` / `is Rejected(_)` still yields a single `.tag` test (the
        // `_` payload is irrefutable). For a loose scrutinee — the `Wire`-call
        // outcome — the single-field `"value"` fallback matches the runtime
        // `{ tag, value }` shape, so the nested test lands without a static type.
        Pattern::Variant { .. } => {
            let scrut_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
            let mut tests = Vec::new();
            // For a loose scrutinee, a *multi*-payload nested pattern would map
            // every positional field to the single-field `"value"` fallback; that
            // is unreachable today (the only loose variant sums — the `Wire`
            // outcome's rejection kinds — are single-field) and is shared with
            // `match`'s lowering. Precise for any typed scrutinee.
            pattern_match_tests(&v, scrut_ty, pattern, cx, &mut tests);
            // `pattern_match_tests` always pushes the outer `.tag` test for a
            // variant pattern, so a variant `is` is never vacuously `true` — a
            // silently-passing `expect` is exactly the failure this reuse removes.
            debug_assert!(
                !tests.is_empty(),
                "a variant `is` pattern must emit at least its outer tag test"
            );
            if tests.is_empty() {
                "true".to_string()
            } else {
                tests.join(" && ")
            }
        }
        // #474 §2.3.6: an or-pattern is legal after `is`. `pattern_match_tests`
        // already knows how to OR its alternatives together (pushing one
        // combined `(...) || (...)` term), so this is a thin wrapper.
        Pattern::Or(..) => {
            let scrut_ty = cx.commons().expr_types.get(&value.id).map(|te| te.ty);
            let mut tests = Vec::new();
            pattern_match_tests(&v, scrut_ty, pattern, cx, &mut tests);
            tests.join(" && ")
        }
    };
    pre.finish(text)
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
                PredKind::NonNegative => format!("{recv} >= 0"),
                PredKind::Positive => format!("{recv} > 0"),
                PredKind::InRange(a, b) => {
                    format!("({recv} >= {} && {recv} <= {})", a.value, b.value)
                }
                PredKind::InRangeF(a, b) => {
                    format!("({recv} >= {} && {recv} <= {})", a.lexeme, b.lexeme)
                }
                PredKind::NonEmpty => format!("{recv}.length > 0"),
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
            ModuleCtx::new(&commons, &cross_context, &runtime_use),
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
            ModuleCtx::new(&commons, &cross_context, &runtime_use),
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
            ModuleCtx::new(&commons, &cross_context, &runtime_use),
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
