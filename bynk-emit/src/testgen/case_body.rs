//! The test-harness case-body lowering entries: the unit, property, contract and integration
//! scaffolding's way into the emitter's statement lowering. They were in `emitter/lower.rs`;
//! 1B.2b (#4028, Decision E) moved them here, so the direction is harness to emitter, and the
//! production emitter holds no test-harness entry. They reach `LowerCtx`, `Pre` and
//! `emit_statement` through `pub(crate)`.
//!
//! `extend_printed_at`, the one place the harness prints a node list into a case body's text
//! buffer, lives here with them (it is what `emit_print_calls` used to read 1 for).

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use bynk_check::checker::{HandlerAddress, TypedCommons};
use bynk_check::project_model::BuildTarget;
use bynk_model::is_effectful_return;
use bynk_syntax::ast::{Block, TypeRef};

use crate::emitter::ctx::{BodyMode, ModuleCtx, TestShared, UnitFacts};
use crate::emitter::lower_call::codec_statics::eval_value_line;
use crate::emitter::lower_ctx::LowerCtx;
use crate::emitter::lower_stmt::{
    StatementSink, emit_block_as_function_body_with_return, function_body_stmts,
};
use crate::emitter::{Pre, RuntimeUse};
use bynk_ts::SourceMapBuilder;

/// #1478: appends `stmts` to `out`, printing each at `depth` — the shared
/// "consume a real-node-returning callee's own output" step. #1479 adds the
/// `depth` parameter (was hardcoded to 0): the harness's own scaffold-body callees (e.g.
/// `emit_ns_destructure`) print nested inside a generated function body, at that body's own
/// depth. It was `case_body::extend_printed_at`; 1B.2b (#4028) moved it here with the rest of the
/// harness's printing, so the production emitter calls `bynk_ts::print_stmt` nowhere and
/// `emit_print_calls` reads 0.
pub fn extend_printed_at(out: &mut String, stmts: Vec<bynk_ts::TsStmt>, depth: usize) {
    for stmt in stmts {
        out.push_str(&bynk_ts::print_stmt(&stmt, depth));
    }
}

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

/// [`lower_block_to_async_body`] as statements (1B.5b), for a predicate closure built as nodes.
pub fn lower_block_to_async_stmts(
    block: &Block,
    return_type: &TypeRef,
    typed: &mut TypedCommons,
    cross_context: &bynk_check::resolver::CrossContextInfo,
    runtime_use: &RuntimeUse,
) -> Vec<bynk_ts::TsStmt> {
    let mut cx = LowerCtx::new(
        ModuleCtx::new(typed, cross_context, runtime_use, UnitFacts::detached()),
        BodyMode::PredicateScaffold {
            test: TestShared {
                test_scaffold: true,
                assert_loc: None,
            },
        },
    );
    let async_tail = is_effectful_return(return_type);
    function_body_stmts(block, &mut cx, async_tail, Some(return_type))
}

/// Lower a test-case body as statements (1B.5b): the body's statements followed by a discarded
/// tail expression (the runner records success via the assertion mechanism, not via a return
/// value), and, when the tail is a discarded `()` and adds no statement, the tail's source-map
/// mark for the statement the caller adds next. The source map is the statements' own spans: a
/// statement carries the span of its source statement, and the printer records it where the
/// statement starts.
///
/// With `report_value` (#3585, `bynk eval`) the tail's value is not discarded: it is encoded as
/// the wire shape `Json.encode` gives it and written to stdout as one
/// `{"type":"eval-value","valueType":"…","value":…}` line, which the test runner's NDJSON reader
/// picks up. Nothing else about the body changes.
#[allow(clippy::too_many_arguments)]
pub fn lower_test_case_stmts(
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
) -> (Vec<bynk_ts::TsStmt>, Option<bynk_syntax::span::Span>) {
    let mut sink = StatementSink::new();
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
        );
        cx.local_agents = test_agents;
        cx.harness_bound = harness_bound;
        for stmt in &block.statements {
            sink.statement(stmt, &mut cx);
        }
        // Evaluate the tail expression but discard its value; assertions inside
        // it still take effect via thrown AssertionErrors.
        sink.mark(block.tail.span);
        let mut pre = Pre::new();
        if report_value && let Some(line) = eval_value_line(&mut pre, &block.tail, &mut cx) {
            let mut stmts = pre.ts_stmts();
            stmts.push(line);
            sink.nodes(stmts);
        } else {
            let tail = pre.lower_node(&block.tail, &mut cx);
            sink.nodes(discarded_tail(&pre, tail));
        }
    }
    sink.finish()
}

/// A lowered integration case body (1B.5b): its statements, and the tail's source-map mark when
/// the tail added no statement of its own, for the statement the caller adds next. The harness
/// decides what the body names from these statements (1B.3c), and the case's source map is their
/// own spans.
pub struct LoweredCaseBody {
    pub stmts: Vec<bynk_ts::TsStmt>,
    pub tail_span: Option<bynk_syntax::span::Span>,
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
) -> LoweredCaseBody {
    let kinds: Vec<SettleKind> = block
        .statements
        .iter()
        .map(|s| settle_kind(s, &typed.callees))
        .collect();
    // Each source statement's lowered statements, in order.
    let mut groups: Vec<Vec<bynk_ts::TsStmt>> = Vec::new();
    // Each statement's top-level bindings, as emitted (a re-`let` or a name
    // the harness binds is renamed), for a settle attempt to return.
    let mut binds: Vec<Vec<String>> = Vec::new();
    let (tail_stmts, tail_span) = {
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
        );
        cx.harness_bound = harness_bound;
        for stmt in &block.statements {
            let mut sink = StatementSink::new();
            sink.statement(stmt, &mut cx);
            groups.push(sink.finish().0);
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
                        bynk_syntax::ast::Statement::Let(..)
                        | bynk_syntax::ast::Statement::EffectLet(..)
                        | bynk_syntax::ast::Statement::Expect(..)
                        | bynk_syntax::ast::Statement::Do(..)
                        | bynk_syntax::ast::Statement::Assign(..)
                        | bynk_syntax::ast::Statement::Advance(..)
                        | bynk_syntax::ast::Statement::Par(..) => None,
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
                bynk_syntax::ast::Statement::Let(..)
                | bynk_syntax::ast::Statement::EffectLet(..)
                | bynk_syntax::ast::Statement::Expect(..)
                | bynk_syntax::ast::Statement::Do(..)
                | bynk_syntax::ast::Statement::Assign(..)
                | bynk_syntax::ast::Statement::Advance(..) => Vec::new(),
            });
        }
        let mut sink = StatementSink::new();
        sink.mark(block.tail.span);
        let mut pre = Pre::new();
        let tail = pre.lower_node(&block.tail, &mut cx);
        sink.nodes(discarded_tail(&pre, tail));
        sink.finish()
    };
    let mut stmts = if settle {
        settle_segments(groups, &kinds, &binds)
    } else {
        groups.into_iter().flatten().collect()
    };
    stmts.extend(tail_stmts);
    LoweredCaseBody { stmts, tail_span }
}

/// A test case's tail, evaluated and discarded: the statements it hoisted, then `void (<tail>);`.
/// A tail that is just `undefined` (a `()`) adds no statement. Assertions inside the tail still
/// take effect, through the `AssertionError`s they throw. The caller prints these straight into
/// the case body, which records no source-map checkpoints for them.
fn discarded_tail(pre: &Pre, tail: bynk_ts::TsExpr) -> Vec<bynk_ts::TsStmt> {
    let mut stmts = pre.ts_stmts();
    if !matches!(&tail, bynk_ts::TsExpr::Ident(name) if name.is_empty() || name == "undefined") {
        stmts.push(bynk_ts::TsStmt::expr_stmt(
            bynk_ts::TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Void,
                expr: Box::new(bynk_ts::TsExpr::Paren(Box::new(tail))),
            },
            None,
        ));
    }
    stmts
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
            address,
            HandlerAddress::Http {
                verb: bynk_syntax::ast::HttpMethod::Get,
                ..
            }
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
        Statement::EffectLet(..)
        | Statement::Do(..)
        | Statement::Assign(..)
        | Statement::Advance(..)
        | Statement::Par(..) => SettleKind::Unsafe,
    }
}

/// #2138 (track §3.6): rewrite a lowered case body for `--settle`. The body's
/// top-level statements split into runs at each unsafe address. In a run
/// holding an `expect` and a safe address before it, the statements from the
/// run's start through its last `expect` become one `__bynkSettle` attempt:
/// re-run until every `expect` in it holds or the deadline passes, so the
/// addresses since the last unsafe one are replayed before each retry. The
/// bindings it declares are returned out of the attempt, so the statements
/// after it see them unchanged. `groups` are each source statement's lowered
/// statements; the tail follows them, outside any attempt.
fn settle_segments(
    groups: Vec<Vec<bynk_ts::TsStmt>>,
    kinds: &[SettleKind],
    binds: &[Vec<String>],
) -> Vec<bynk_ts::TsStmt> {
    let mut groups: Vec<Option<Vec<bynk_ts::TsStmt>>> = groups.into_iter().map(Some).collect();
    let mut out: Vec<bynk_ts::TsStmt> = Vec::new();
    let mut take = |k: usize| groups[k].take().unwrap_or_default();
    let mut i = 0;
    while i < kinds.len() {
        if kinds[i] == SettleKind::Unsafe {
            out.extend(take(i));
            i += 1;
            continue;
        }
        let end = (i..kinds.len())
            .find(|&k| kinds[k] == SettleKind::Unsafe)
            .unwrap_or(kinds.len());
        let last_expect = (i..end).rev().find(|&k| kinds[k] == SettleKind::Expect);
        match last_expect {
            Some(e) if (i..e).any(|k| kinds[k] == SettleKind::Safe) => {
                let segment: Vec<bynk_ts::TsStmt> = (i..=e).flat_map(&mut take).collect();
                let names: Vec<&str> = binds[i..=e].iter().flatten().map(String::as_str).collect();
                out.push(settle_attempt(segment, &names));
                for k in e + 1..end {
                    out.extend(take(k));
                }
            }
            _ => {
                for k in i..end {
                    out.extend(take(k));
                }
            }
        }
        i = end;
    }
    out
}

/// One `__bynkSettle` attempt around `segment`, returning `names` (the
/// segment's top-level bindings, from the AST, as emitted) so the code after
/// it sees them.
fn settle_attempt(mut segment: Vec<bynk_ts::TsStmt>, names: &[&str]) -> bynk_ts::TsStmt {
    use crate::ts_build::{await_expr, block_arrow, call, ident};
    if !names.is_empty() {
        segment.push(bynk_ts::TsStmt::return_stmt(
            Some(bynk_ts::TsExpr::object_entries(
                names
                    .iter()
                    .map(|n| bynk_ts::TsObjectEntry::Shorthand((*n).to_string()))
                    .collect(),
            )),
            None,
        ));
    }
    let attempt = await_expr(call(
        ident("__bynkSettle"),
        vec![block_arrow(Vec::new(), true, segment)],
    ));
    if names.is_empty() {
        bynk_ts::TsStmt::expr_stmt(attempt, None)
    } else {
        bynk_ts::TsStmt::const_stmt(
            bynk_ts::TsBindingName::ObjectPattern(names.iter().map(|n| n.to_string()).collect()),
            None,
            attempt,
            None,
        )
    }
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    /// Each piece's binding is the identifier after a leading `const `, as
    /// the AST would give it. The rewrite prints at depth 0, the tail after it.
    fn rewrite(pieces: &[&str], kinds: &[SettleKind], tail: &str) -> String {
        let groups: Vec<Vec<bynk_ts::TsStmt>> = pieces
            .iter()
            .map(|p| vec![bynk_ts::TsStmt::raw(p.to_string(), None)])
            .collect();
        let binds: Vec<Vec<String>> = pieces
            .iter()
            .map(|p| {
                p.strip_prefix("const ")
                    .and_then(|r| r.split(' ').next())
                    .map(|n| vec![n.to_string()])
                    .unwrap_or_default()
            })
            .collect();
        let mut text: String = settle_segments(groups, kinds, &binds)
            .iter()
            .map(|s| bynk_ts::print_stmt(s, 0))
            .collect();
        text.push_str(tail);
        text
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
            "const a = await post();\nconst { n } = await __bynkSettle(async () => {\nconst n = await get();\nif (!(n === 1)) { throw fail(); }\n  return { n };\n});\nconst after = n;\nvoid (after);\n"
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
             \x20 return { n };\n\
             });\n\
             const t = n;\n\
             const b = await post();\n\
             const { m } = await __bynkSettle(async () => {\n\
             const m = await get();\n\
             if (!(m)) { throw fail(); }\n\
             \x20 return { m };\n\
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
             \x20 return { q, n };\n\
             });\n"
        );
    }

    /// A multi-line template literal in a segment keeps its contents.
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
