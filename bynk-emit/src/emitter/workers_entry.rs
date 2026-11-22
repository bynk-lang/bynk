//! Per-Worker `index.ts` generation (v0.8 §4.3, v0.9 §5.1).
//!
//! Each Worker exports a `default { fetch }` handler that first checks
//! for the internal Service Binding prefix (`/_bynk/call/`) and dispatches
//! to the matching `on call` service operation, then matches the request
//! method + path against any `on http` routes the context declares.
//! Validation and uncaught errors map to 400 / 500 respectively.
//!
//! Arc C slice 4 (#1323): `emit_worker_entry` and its four helpers build a
//! real `bynk_ts::TsProgram` directly instead of `writeln!`-ing a `String` —
//! this file's own conversion, closing the fourth Arc C slice (`workers.rs`
//! was the third, #1321).

use crate::emitter::http_handler_method_name_ir;
use crate::emitter::serialisation;
use crate::emitter::ts_ident;
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::{BaseType, Handler, LimitsPolicy, ServiceProtocol, TypeRef};

use crate::emitter::RuntimeUse;
use bynk_check::wire_format::{self as wf, CallCheck};
use bynk_ts::{
    TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsProgram, TsStmt, TsSwitchCase,
    TsType, TsTypeMember,
};

// -- Small tree-construction helpers (#1323) ------------------------------
//
// Mirrors `workers.rs`'s own local helper set (#1321) — not part of the
// public node algebra, `bynk-ts` still owns every real constructor; these
// just compose them for this file's own repeated shapes. Kept as this
// file's own private set rather than shared with `workers.rs`'s, matching
// this track's own established per-file scoping (each Arc C slice's own
// helpers are added narrowly, not factored into a shared cross-file module).

fn ident(s: impl Into<String>) -> TsExpr {
    TsExpr::Ident(s.into())
}

fn str_lit(s: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.into()))
}

fn num_lit(n: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Num(n.into()))
}

fn null_lit() -> TsExpr {
    TsExpr::Lit(TsLit::Null)
}

/// #1323's own real gap: `CorsPolicy.credentials`/`SecurityPolicy.nosniff`
/// are real booleans — nothing in `workers.rs`'s own grounding ever needed
/// one.
fn bool_lit(b: bool) -> TsExpr {
    TsExpr::Lit(TsLit::Bool(b))
}

fn member(object: TsExpr, property: impl Into<String>) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.into(),
    }
}

fn index_expr(object: TsExpr, idx: TsExpr) -> TsExpr {
    TsExpr::Index {
        object: Box::new(object),
        index: Box::new(idx),
    }
}

fn call(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

fn method_call(object: TsExpr, method: &str, args: Vec<TsExpr>) -> TsExpr {
    call(member(object, method), args)
}

fn new_expr(callee: &str, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::New {
        callee: Box::new(ident(callee)),
        args,
    }
}

fn as_expr(expr: TsExpr, ty: TsType) -> TsExpr {
    TsExpr::As {
        expr: Box::new(expr),
        ty,
    }
}

fn await_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Await(Box::new(expr))
}

fn binary(op: bynk_ts::TsBinaryOp, left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn strict_eq(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::StrictEq, left, right)
}

fn strict_neq(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::StrictNotEq, left, right)
}

fn or_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::Or, left, right)
}

fn and_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(bynk_ts::TsBinaryOp::And, left, right)
}

/// `test ? consequent : alternate`.
fn cond_expr(test: TsExpr, consequent: TsExpr, alternate: TsExpr) -> TsExpr {
    TsExpr::Conditional {
        test: Box::new(test),
        consequent: Box::new(consequent),
        alternate: Box::new(alternate),
    }
}

/// An explicit, printer-preserved `(<inner>)` — #1323's own real gap, the
/// CORS-preflight guard's own unconditional grouping. See
/// [`bynk_ts::TsExpr::Paren`]'s own doc for why this needs to be a distinct
/// variant rather than relying on the printer's precedence-derived rules.
fn paren(inner: TsExpr) -> TsExpr {
    TsExpr::Paren(Box::new(inner))
}

fn return_(expr: Option<TsExpr>) -> TsStmt {
    TsStmt::return_stmt(expr, None)
}

fn const_(name: impl Into<String>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), None, init, None)
}

fn const_typed(name: impl Into<String>, ty: TsType, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), Some(ty), init, None)
}

/// `let name: ty;` — an uninitialised, typed `let`. `emit_http_route_dispatch`'s
/// own `let __body_json: JsonValue;` and `emit_http_sum_wrapper`-adjacent
/// `let __raw: string;` are the real, grounded sites.
fn let_typed(name: impl Into<String>, ty: TsType) -> TsStmt {
    TsStmt::let_stmt(TsBindingName::Ident(name.into()), Some(ty), None, None)
}

fn expr_stmt(expr: TsExpr) -> TsStmt {
    TsStmt::expr_stmt(expr, None)
}

fn if_(cond: TsExpr, then_branch: TsStmt) -> TsStmt {
    TsStmt::if_stmt(cond, then_branch, None)
}

fn block(stmts: Vec<TsStmt>) -> TsStmt {
    TsStmt::block(stmts, None)
}

/// What `fetch`'s catch does with a fault (#1825): log it, report it, and answer
/// the bare 500. #3580: a traced build also hands the fault to the request's record.
fn fault_handler(context: &str, trace: bool) -> Vec<TsStmt> {
    let mut handler = vec![expr_stmt(method_call(
        ident("globalThis.console"),
        "error",
        vec![
            TsExpr::template_lit(
                vec![format!("{context} "), " faulted".to_string()],
                vec![ident("__route")],
            ),
            fault_detail("e"),
        ],
    ))];
    if trace {
        handler.push(expr_stmt(call(ident("__traceFault"), vec![ident("e")])));
    }
    handler.push(report_fault());
    handler.push(return_(Some(call(
        ident("__faultResponse"),
        vec![str_lit(context.to_string()), ident("__route")],
    ))));
    handler
}

/// #3580: a traced build moves the dispatch (with any edge-cache guard) into
/// `__bynkTracedFetch`, and `fetch` serves each request under a record that
/// prints one `__bynk_trace:` line as the response is ready. Returns `fetch`'s
/// new body.
fn traced_fetch(
    context: &str,
    header_decls: &mut Vec<TsStmt>,
    fetch_params: &[TsParam],
    fetch_return: &Option<TsType>,
    fetch_body: Vec<TsStmt>,
) -> Vec<TsStmt> {
    header_decls.push(TsStmt::decl(
        TsDecl::Function {
            name: "__bynkTracedFetch".to_string(),
            generics: Vec::new(),
            params: fetch_params.to_vec(),
            return_type: fetch_return.clone(),
            body: fetch_body,
            is_async: true,
            inline: false,
        },
        None,
    ));
    vec![return_(Some(call(
        ident("__traceFetch"),
        vec![
            str_lit(context.to_string()),
            ident("__bynkTracedFetch"),
            ident("request"),
            ident("env"),
            ident("ctx"),
        ],
    )))]
}

/// #1825: `__route = "<what>";`, naming the dispatch `fetch`'s fault log
/// reports.
fn set_fault_route(what: String) -> TsStmt {
    TsStmt::assign(ident("__route"), str_lit(what), None)
}

/// #2053: what a fault handler logs for a caught `e`, the expression
/// `e instanceof globalThis.Error ? (e.stack || globalThis.String(e)) : e`.
///
/// Logging the error as a value reaches `wrangler tail` as its string,
/// `Error: <message>`, with no frames at all. A V8 stack's own first line is
/// that same string, so nothing is lost; a thrown non-`Error` is logged as
/// itself, as before. `||`, not `??`, so an empty `stack` (assignable on any
/// `Error`) falls back to the string too, rather than logging nothing.
///
/// #2192: the frames stay in the bundle's coordinates (`index.js:L:C`).
/// Cloudflare applies an uploaded source map only to an exception it records,
/// never to a logged string, so the handler also reports the fault
/// (`__bynkReportFault`), and that exception's stack is the mapped one.
pub(crate) fn fault_detail(e: &str) -> TsExpr {
    cond_expr(
        binary(
            bynk_ts::TsBinaryOp::InstanceOf,
            ident(e),
            ident("globalThis.Error"),
        ),
        paren(binary(
            bynk_ts::TsBinaryOp::Or,
            member(ident(e), "stack"),
            call(ident("globalThis.String"), vec![ident(e)]),
        )),
        ident(e),
    )
}

/// #2192: reports a fault the entry caught to the platform, so Cloudflare
/// records it as an exception and applies an uploaded source map to its
/// stack (the logged `fault_detail` string is never mapped). The rejected
/// promise goes to `waitUntil`, which reports it without replacing the
/// handler's own answer: `fetch` still returns its 500 and keeps the outcome
/// `ok`, and a queue message's `ack()`/`retry()` still decides its delivery.
/// It is handled first, so a host that never awaits `waitUntil` (or awaits it
/// late, as the test doubles do) sees no unhandled rejection. `ctx` is absent
/// when the entry is called in-process, e.g. by the test runner.
const REPORT_FAULT_HELPER: &str = "function __bynkReportFault(ctx: { waitUntil(promise: globalThis.Promise<unknown>): void } | undefined, e: unknown): void {
  if (ctx === undefined) return;
  const fault = globalThis.Promise.reject(e);
  fault.catch(() => {});
  ctx.waitUntil(fault);
}
";

/// #3562 (G2b, 2c-4a): the marker of a member's own misses. A Worker group's
/// dispatcher tries its members in turn and must tell a member that does not own
/// a request (its router's `404`, its synthesised `405`) from one that answered
/// it, including with a `404` of its own. The router's own misses are added to a
/// module-level `WeakSet`, and `isMiss` reads it; a response a handler returned
/// is never added. It is never a header, so it cannot reach a client.
///
/// The dispatcher's contract, which the group `index.ts` implements: members are
/// tried in declaration order; a marked response is a miss and the next member
/// is tried; the first unmarked response is the answer. If every member misses,
/// the answer is the first marked response that is not a `404` (the synthesised
/// `405`/`204`) with the `Allow` of every such response merged, else the
/// single-context `Not Found`. The synthesised
/// `405` is a miss on purpose: `GET /x` in one member and `POST /x` in another is
/// allowed (a route is keyed on method and path), so the `POST` must reach the
/// second member rather than be answered by the first member's `405`.
const MISS_HELPER: &str = "const __misses = new globalThis.WeakSet<globalThis.Response>();
function __miss(response: globalThis.Response): globalThis.Response {
  __misses.add(response);
  return response;
}
export function isMiss(response: globalThis.Response): boolean {
  return __misses.has(response);
}
";

/// `__miss(<response>)` in a Worker group's member, `<response>` otherwise: the
/// router's own "not mine" answers are marked there ([`MISS_HELPER`]).
fn miss(member_mode: bool, response: TsExpr) -> TsExpr {
    if member_mode {
        call(ident("__miss"), vec![response])
    } else {
        response
    }
}

/// The condition of an internal dispatch block: `path.startsWith(<prefix>)`, and,
/// in a Worker group's member, that the rest of the path is one of `names`. A
/// member answers only what it owns, and decides that before it reads the body
/// or verifies a signature, so a request meant for another member reaches that
/// member untouched.
fn own_path_guard(member_mode: bool, prefix: &str, names: &[String]) -> TsExpr {
    let starts = method_call(ident("path"), "startsWith", vec![str_lit(prefix)]);
    if !member_mode {
        return starts;
    }
    let rest = method_call(
        ident("path"),
        "slice",
        vec![member(str_lit(prefix), "length")],
    );
    and_expr(
        starts,
        method_call(str_array(names), "includes", vec![rest]),
    )
}

/// #2192: `__bynkReportFault(ctx, e);`, in an entry-point catch clause.
fn report_fault() -> TsStmt {
    expr_stmt(call(
        ident("__bynkReportFault"),
        vec![ident("ctx"), ident("e")],
    ))
}

fn try_catch_stmt(
    try_stmts: Vec<TsStmt>,
    catch_param: Option<&str>,
    catch_stmts: Vec<TsStmt>,
) -> TsStmt {
    TsStmt::try_catch(block(try_stmts), catch_param, block(catch_stmts), None)
}

/// `switch (<discriminant>) { <cases> }`.
fn switch_stmt(discriminant: TsExpr, cases: Vec<TsSwitchCase>) -> TsStmt {
    TsStmt::switch_stmt(discriminant, cases, None)
}

fn case_(test: TsExpr, body: Vec<TsStmt>) -> TsSwitchCase {
    TsSwitchCase {
        labels: vec![test],
        body,
        default_braced: false,
        // Arc E slice 6 (#1445): every real non-`default` case in this file
        // is `{ }`-blocked, same as before `case_braced` existed.
        case_braced: true,
        span: None,
    }
}

fn default_case(body: Vec<TsStmt>) -> TsSwitchCase {
    TsSwitchCase {
        labels: vec![],
        body,
        default_braced: false,
        case_braced: true,
        span: None,
    }
}

fn new_response(args: Vec<TsExpr>) -> TsExpr {
    new_expr("globalThis.Response", args)
}

fn json_stringify(expr: TsExpr) -> TsExpr {
    method_call(ident("globalThis.JSON"), "stringify", vec![expr])
}

fn status_obj(status: i64) -> TsExpr {
    TsExpr::object(vec![("status".to_string(), num_lit(status.to_string()))])
}

fn json_content_type_headers() -> (String, TsExpr) {
    // `TsObjectEntry::Prop`'s own key prints verbatim (no quoting logic in
    // the printer), so a hyphenated key must be pre-quoted by the caller —
    // the same precedent `workers.rs`'s own conversion (#1321) already set
    // for this exact header key.
    (
        "headers".to_string(),
        TsExpr::object(vec![(
            "\"content-type\"".to_string(),
            str_lit("application/json"),
        )]),
    )
}

fn status_json_headers_obj(status: i64) -> TsExpr {
    TsExpr::object(vec![
        ("status".to_string(), num_lit(status.to_string())),
        json_content_type_headers(),
    ])
}

/// `new Response(JSON.stringify(<body>), { status: <status>, headers: {
/// "content-type": "application/json" } })` — the dominant JSON-error-
/// response shape across this file's own real content.
fn json_response(body: TsExpr, status: i64) -> TsExpr {
    new_response(vec![json_stringify(body), status_json_headers_obj(status)])
}

fn text_response(text: &str, status: i64) -> TsExpr {
    new_response(vec![str_lit(text), status_obj(status)])
}

/// I2 (#1971): `const __body = await __verifyCall(request, env, "<context>");
/// if (typeof __body !== "string") return __body;`. Every internal dispatch
/// (`/_bynk/call/`, `/_bynk/event/`) makes it first, after only the call
/// dispatch's method check.
///
/// The runtime reads the body under the published cap (`413`), then checks
/// the request's signature under the key this Worker holds for the edge from
/// the claimed caller, bound to `context` as the callee (`401`), and hands back
/// the verified body as text. The prefixes are reachable from the public
/// internet on any route the Worker serves (ADR 0092's "not externally
/// routable" premise does not hold), so a request is refused here, before the
/// service is looked up, the contract compared or the body parsed: an
/// unauthenticated sender learns nothing about which services exist (no 404
/// against 409), never has a hash echoed, and cannot write the skew log line.
/// The refusals are bare words, not JSON detail.
///
/// I3 (#1971): a Worker whose `on call` handlers admit foreign callers passes
/// its table of them (`__FOREIGN_CALLERS`, actor name → secret), so a call
/// naming one is checked under that actor's secret rather than an edge key.
fn verify_gate(context: &str, foreign: bool) -> [TsStmt; 2] {
    let mut args = vec![ident("request"), ident("env"), str_lit(context)];
    if foreign {
        args.push(ident(FOREIGN_CALLERS));
    }
    [
        const_("__body", await_expr(call(ident("__verifyCall"), args))),
        if_(
            strict_neq(
                TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Typeof,
                    expr: Box::new(ident("__body")),
                },
                str_lit("string"),
            ),
            return_(Some(ident("__body"))),
        ),
    ]
}

/// #3581 (the dev agent-inspection track): the `/_bynk/inspect/` dispatch of a
/// `bynk dev` build. The runtime verifies the request's signature under the local
/// edge key, as `/_bynk/call/` does, before it looks anything up, so an unsigned
/// sender gets a bare `401`. `classes` is each agent class this Worker hosts, with
/// its Durable Object binding name.
fn inspect_route(context: &str, member_mode: bool, lists: bool, classes: &[String]) -> TsStmt {
    let table = TsExpr::object(
        classes
            .iter()
            .map(|c| {
                (
                    format!("\"{c}\""),
                    str_lit(crate::emitter::wrangler::agent_binding(c).name),
                )
            })
            .collect(),
    );
    if_(
        inspect_guard(member_mode, lists, classes),
        block(vec![return_(Some(await_expr(call(
            ident("__inspectRoute"),
            vec![ident("env"), ident("request"), str_lit(context), table],
        ))))]),
    )
}

/// The condition of the inspect dispatch: `path.startsWith("/_bynk/inspect/")`,
/// and, in a Worker group's member, that the request is this member's: the
/// listing, if this is the member that answers it (`lists`, the group's first
/// member that hosts an agent), or an agent of this member's own class. A member
/// decides that before it verifies a signature, so a request for another
/// member's agent reaches that member untouched.
fn inspect_guard(member_mode: bool, lists: bool, classes: &[String]) -> TsExpr {
    let starts = method_call(ident("path"), "startsWith", vec![str_lit(INSPECT_PREFIX)]);
    if !member_mode {
        return starts;
    }
    let own_agent = own_path_guard(true, INSPECT_AGENT_PREFIX, classes);
    let owned = if lists {
        or_expr(
            strict_eq(ident("path"), str_lit(INSPECT_LIST_PATH)),
            own_agent,
        )
    } else {
        own_agent
    };
    and_expr(starts, paren(owned))
}

/// The `bynk dev` inspect routes (`runtime/inspect.ts`): the prefix, the listing
/// and the per-agent prefix.
pub(crate) const INSPECT_PREFIX: &str = "/_bynk/inspect/";
const INSPECT_LIST_PATH: &str = "/_bynk/inspect/agents";
const INSPECT_AGENT_PREFIX: &str = "/_bynk/inspect/agent/";

/// #3581: the registry Durable Object class a `bynk dev` build hosts, imported
/// from the runtime and exported from the entry like an agent's class, so the dev
/// copy of `wrangler.toml` can bind it.
fn inspect_registry_export(inspect: &[String]) -> Option<TsStmt> {
    (!inspect.is_empty()).then(|| {
        TsStmt::decl(
            TsDecl::ExportNames {
                names: vec!["BynkInspectRegistry".to_string()],
            },
            None,
        )
    })
}

/// The entry's internal admin routes, in dispatch order, each followed by a blank:
/// State-migrations S4 (#2259 Decision D): `bynk migrate sweep`'s wake, present only
/// while a sweep has set its key (without it the route is `404`, as if absent); it
/// admits the sweep's caller name alone and verifies the signature under the
/// sweep's key. Then #3581: the dev-only agent inspection route, signed as an
/// internal call. Neither is present unless the build has it.
fn admin_routes(
    context: &str,
    member_mode: bool,
    migratable: &[(String, i64)],
    (inspect, inspect_lists): (&[String], bool),
) -> Vec<TsStmt> {
    let mut routes = Vec::new();
    if !migratable.is_empty() {
        routes.push(migrate_route(context, member_mode, migratable));
        routes.push(TsStmt::blank(None));
    }
    if !inspect.is_empty() {
        routes.push(inspect_route(context, member_mode, inspect_lists, inspect));
        routes.push(TsStmt::blank(None));
    }
    routes
}

/// State-migrations S4 (#2259 Decision D): the `/_bynk/migrate/<Class>`
/// dispatch. See the call site for the order of its checks.
fn migrate_route(context: &str, member_mode: bool, migratable: &[(String, i64)]) -> TsStmt {
    let not_found = || return_(Some(miss(member_mode, text_response("Not found", 404))));
    let mut cases: Vec<TsSwitchCase> = migratable
        .iter()
        .map(|(class, version)| {
            case_(
                str_lit(class.clone()),
                vec![return_(Some(await_expr(call(
                    ident("__sweepAgentInstances"),
                    vec![
                        member(
                            ident("env"),
                            crate::emitter::wrangler::agent_binding(class).name,
                        ),
                        ident("__body"),
                        num_lit(version.to_string()),
                    ],
                ))))],
            )
        })
        .collect();
    cases.push(default_case(vec![not_found()]));
    let body = vec![
        if_(
            strict_neq(
                TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Typeof,
                    expr: Box::new(index_expr(
                        ident("(env as unknown as globalThis.Record<string, unknown>)"),
                        str_lit(wf::MIGRATE_KEY),
                    )),
                },
                str_lit("string"),
            ),
            not_found(),
        ),
        call_method_gate(),
        if_(
            strict_neq(
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit(wf::CALLER_HEADER)],
                ),
                str_lit(wf::MIGRATE_CALLER),
            ),
            return_(Some(text_response("Unauthorized", 401))),
        ),
        const_(
            "__body",
            await_expr(call(
                ident("__verifyCall"),
                vec![
                    ident("request"),
                    ident("env"),
                    str_lit(context),
                    TsExpr::object(vec![(
                        format!("\"{}\"", wf::MIGRATE_CALLER),
                        str_lit(wf::MIGRATE_KEY),
                    )]),
                ],
            )),
        ),
        if_(
            strict_neq(
                TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Typeof,
                    expr: Box::new(ident("__body")),
                },
                str_lit("string"),
            ),
            return_(Some(ident("__body"))),
        ),
        switch_stmt(
            method_call(
                ident("path"),
                "slice",
                vec![member(str_lit(wf::MIGRATE_PATH_PREFIX), "length")],
            ),
            cases,
        ),
    ];
    let classes: Vec<String> = migratable.iter().map(|(class, _)| class.clone()).collect();
    if_(
        own_path_guard(member_mode, wf::MIGRATE_PATH_PREFIX, &classes),
        block(body),
    )
}

/// I3 (#1971): the per-Worker table of the foreign callers its `on call`
/// handlers admit, actor name → the secret their calls verify under.
const FOREIGN_CALLERS: &str = "__FOREIGN_CALLERS";

/// I3 (#1971): the published admission check for one `on call` handler, made
/// after the service is found and before its contract is compared: the
/// caller the signature proved must be one the handler's `by` clause admits.
/// A Bynk context is admitted by `Caller`, a foreign caller only by its own
/// actor, never by `Caller`; anyone else gets a bare `403` (authenticated, not
/// admitted). For a sum it then builds the resolved actor, `__who`.
fn admission_gate(admission: &bynk_check::actors::CallAdmission) -> Vec<TsStmt> {
    let from = || ident("__from");
    let mut admitted: Option<TsExpr> = None;
    let mut push = |e: TsExpr| {
        admitted = Some(match admitted.take() {
            Some(a) => or_expr(a, e),
            None => e,
        });
    };
    if admission.contexts {
        push(TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(ident("__foreign")),
        });
    }
    for f in &admission.foreign {
        push(strict_eq(from(), str_lit(f.actor.clone())));
    }
    let admitted = admitted.unwrap_or_else(|| ident("false"));
    let mut stmts = vec![const_(
        "__from",
        as_expr(
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit(wf::CALLER_HEADER)],
            ),
            TsType::named("string"),
        ),
    )];
    // Whether the caller is a foreign one, read only where a context is
    // admitted too.
    if admission.contexts {
        stmts.push(const_(
            "__foreign",
            call(
                ident("globalThis.Object.hasOwn"),
                vec![ident(FOREIGN_CALLERS), from()],
            ),
        ));
    }
    stmts.push(if_(
        TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(paren(admitted)),
        },
        return_(Some(text_response(
            CallCheck::NotAdmitted.text_body().unwrap_or_default(),
            check_status(CallCheck::NotAdmitted),
        ))),
    ));
    if let Some(members) = &admission.sum {
        // The members in `by` order: a foreign one by its name, `Caller` for
        // any context. Admission above means one of them matches.
        let mut who = TsExpr::object(vec![
            ("tag".to_string(), str_lit("Caller")),
            ("identity".to_string(), from()),
        ]);
        for m in members.iter().rev() {
            if let bynk_check::actors::CallSumMember::Foreign(f) = m {
                who = cond_expr(
                    strict_eq(from(), str_lit(f.actor.clone())),
                    TsExpr::object(vec![("tag".to_string(), str_lit(f.actor.clone()))]),
                    who,
                );
            }
        }
        stmts.push(const_typed(
            "__who",
            crate::emitter::emit::call_sum_ts_type(members),
            who,
        ));
    }
    stmts
}

/// #1969: a [`CallCheck`]'s published status, as the emitter's `i64`.
fn check_status(check: CallCheck) -> i64 {
    i64::from(check.status())
}

/// #1969: a [`CallCheck`]'s published error `kind`. Only called for the checks
/// that answer with a JSON error body.
fn check_kind(check: CallCheck) -> &'static str {
    check
        .kind()
        .expect("only a check with a JSON error body names a kind")
}

/// #1969 (wire format v1): `if (method !== "POST") return new Response(null,
/// { status: 405, headers: { allow: "POST" } });`, the first statement of the
/// `/_bynk/call/` dispatch. A call is only ever a `POST`; before this, a `GET`
/// reached the body read and failed there as a `500`.
fn call_method_gate() -> TsStmt {
    if_(
        strict_neq(ident("method"), str_lit(wf::CALL_METHOD)),
        return_(Some(new_response(vec![
            null_lit(),
            TsExpr::object(vec![
                (
                    "status".to_string(),
                    num_lit(check_status(CallCheck::Method).to_string()),
                ),
                (
                    "headers".to_string(),
                    TsExpr::object(vec![("allow".to_string(), str_lit(wf::CALL_METHOD))]),
                ),
            ]),
        ]))),
    )
}

/// #1969 (wire format v1): `const __wire = request.headers.get("X-Bynk-Wire");
/// if (__wire !== null && __wire !== "1") return <400 UnsupportedWireVersion>;`.
///
/// After the signature, so an unauthenticated sender cannot probe which
/// versions a callee speaks; before the service lookup, because the version
/// decides how everything after it reads. An absent header is version 1. The
/// refusal names the versions this callee accepts, which is how a caller
/// learns them.
fn wire_version_gate() -> Vec<TsStmt> {
    let mut refused = strict_neq(ident("__wire"), null_lit());
    for v in wf::ACCEPTED_WIRE_VERSIONS {
        refused = and_expr(refused, strict_neq(ident("__wire"), str_lit(v.to_string())));
    }
    vec![
        const_(
            "__wire",
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit(wf::WIRE_HEADER)],
            ),
        ),
        if_(
            refused,
            return_(Some(json_response(
                json_error_kind(
                    check_kind(CallCheck::WireVersion),
                    vec![(
                        "accepted".to_string(),
                        TsExpr::array(
                            wf::ACCEPTED_WIRE_VERSIONS
                                .iter()
                                .map(|v| num_lit(v.to_string()))
                                .collect(),
                        ),
                    )],
                ),
                check_status(CallCheck::WireVersion),
            ))),
        ),
    ]
}

fn null_response(status: i64) -> TsExpr {
    new_response(vec![null_lit(), status_obj(status)])
}

/// `{ kind: "<kind>", ...fields }` — the shared shape of every synthesised
/// boundary-error body this file builds.
fn json_error_kind(kind: &str, fields: Vec<(String, TsExpr)>) -> TsExpr {
    let mut entries = vec![("kind".to_string(), str_lit(kind))];
    entries.extend(fields);
    TsExpr::object(entries)
}

/// `[<items>]` of string literals.
fn str_array(items: &[String]) -> TsExpr {
    TsExpr::array(items.iter().map(|s| str_lit(s.clone())).collect())
}

/// #3386: the cookie a route's `SignedCookie` seam reads, if it has one.
fn signed_cookie_name(
    h: &bynk_syntax::ast::Handler,
    actors: &std::collections::HashMap<String, bynk_syntax::ast::ActorDecl>,
) -> Option<String> {
    bynk_check::actors::signed_cookie_seam_for(h, actors).map(|s| s.cookie)
}

/// #3477: each container's Durable Object class lives in `containers.ts`
/// (generated by `emit_containers_ts`) and is re-exported from the entry the
/// same way an agent's is.
///
/// A Worker group's members do not re-export them: `containers.ts` belongs to
/// the Worker, so the group's entry exports the classes of all its members once.
fn container_class_reexport(table: &UnitTable, member_mode: bool) -> Option<TsStmt> {
    if member_mode {
        return None;
    }
    let mut names: Vec<String> = table
        .derived
        .iter()
        .filter(|(_, d)| d.is_container())
        .map(|(capability, _)| bynk_check::containers::container_class_name(capability))
        .collect();
    names.sort();
    (!names.is_empty()).then(|| {
        TsStmt::decl(
            TsDecl::ReExport {
                names,
                from: "./containers.js".to_string(),
            },
            None,
        )
    })
}

/// The `/_bynk/call/` dispatch: the published wire format's checks
/// (`bynk_check::wire_format::CALL_CHECKS`) in their published order: method,
/// body size and signature (I2), wire version, then (per service) the service,
/// its contract, the body's JSON and the arguments.
///
/// A member of a Worker group (`member_mode`) answers only the services it declares,
/// and tests that **before** any gate: the gates read the body and verify the
/// signature for this member's context, so a member that missed would otherwise
/// refuse a call signed for another member. A call to a service no member
/// declares is therefore a miss, answered by the group's own `404`. A member
/// with no `on call` service has no such block.
fn call_dispatch(
    context: &str,
    member_mode: bool,
    table: &UnitTable,
    service_names: &[&String],
    (own_contracts, accepted_contracts): (
        &std::collections::BTreeMap<String, String>,
        &std::collections::BTreeMap<String, Vec<String>>,
    ),
    has_foreign: bool,
    runtime_use: &RuntimeUse,
) -> Option<TsStmt> {
    let mut call_cases: Vec<TsSwitchCase> = Vec::new();
    let mut call_names: Vec<String> = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        let Some(h) = service.handlers.iter().find(|h| {
            matches!(
                bynk_model::lower_handler_kind_ir(&h.kind),
                bynk_model::IrHandlerKind::Call
            )
        }) else {
            continue;
        };

        // #1825: names the dispatch for the fault log below.
        let mut case_body: Vec<TsStmt> = vec![set_fault_route(format!("call {sname}"))];
        if has_foreign
            && let Some(admission) = bynk_check::actors::call_admission_for(h, &table.actors)
        {
            case_body.extend(admission_gate(&admission));
        }
        // v0.177 (#643): the deploy-skew check runs **before the body is read**.
        //
        // The caller stamps a hash of its compiled view of this contract; this
        // context stamps its own. They are constants in two separately-deployed
        // artifacts, frozen at two different deploy times — so a mismatch means
        // the deployed callee is not the one the caller was compiled against,
        // which is exactly what `deploy --context NAME` makes possible.
        //
        // First, ahead of even parsing the payload: once the contracts disagree
        // the body's *interpretation* is the thing in doubt, so validating it
        // would report a misleading `StructuralMismatch` for the real fault —
        // and there is no reason to parse a body that is already refused.
        //
        // `409` rather than `400`: the payload is not malformed, and the caller
        // cannot fix it by sending different bytes — the two deployments
        // conflict.
        //
        // An absent header fails closed. A Bynk caller always stamps one, so its
        // absence means a non-Bynk or pre-upgrade caller — and a pre-upgrade
        // caller is a skewed caller by definition. This departs from ADR 0092's
        // conditional posture (a missing `X-Bynk-Caller` fail-closes only on a
        // `by c: Caller` handler) because there is no binder to condition on:
        // identity matters only when read, but the contract always matters. It
        // therefore also precedes the caller check — if the contracts disagree,
        // nothing about the request is trustworthy, including the identity.
        if let Some(expected) = own_contracts.get(*sname) {
            case_body.push(const_(
                "__contract",
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit(wf::CONTRACT_HEADER)],
                ),
            ));
            // #1826: the callee decides, so it logs the skew, by name and with
            // both hashes, before refusing: otherwise the diagnosis existed only
            // in a response body passed between two Workers.
            let skew = |actual: TsExpr| {
                vec![
                    ("service".to_string(), str_lit(sname.to_string())),
                    ("expected".to_string(), str_lit(expected.clone())),
                    ("actual".to_string(), actual),
                ]
            };
            // #1826 review: `/_bynk/call/` is reachable from outside, and
            // nothing about the request is trusted yet, so the header is
            // logged bounded to a hash's 16 characters: an operator loses
            // nothing, and a sender can't write arbitrary text to the log.
            // The 409 body echoes it unbounded, to the sender who sent it.
            let logged_actual = method_call(
                call(ident("globalThis.String"), vec![ident("__contract")]),
                "slice",
                vec![num_lit("0"), num_lit("16")],
            );
            // #2554: refused unless it is this contract's hash or one the
            // lock records this callee as still accepting. The 409 below still
            // names the current hash as the expected one.
            let mismatch = accepted_contracts.get(*sname).into_iter().flatten().fold(
                strict_neq(ident("__contract"), str_lit(expected.clone())),
                |all, hash| and_expr(all, strict_neq(ident("__contract"), str_lit(hash.clone()))),
            );
            case_body.push(if_(
                mismatch,
                block(vec![
                    expr_stmt(method_call(
                        ident("globalThis.console"),
                        "error",
                        vec![
                            str_lit(format!("ContractMismatch {context} call {sname}")),
                            TsExpr::object(skew(logged_actual)),
                        ],
                    )),
                    return_(Some(json_response(
                        json_error_kind(check_kind(CallCheck::Contract), skew(ident("__contract"))),
                        check_status(CallCheck::Contract),
                    ))),
                ]),
            ));
        }
        // #1969 (wire format v1): the body is parsed here, so malformed JSON is
        // the caller's fault (`400 MalformedJson`), not a fault in this Worker
        // (`500`). I2 (#1971): it is the text `__verifyCall` read and verified,
        // so the bytes decoded are the bytes signed.
        case_body.push(let_typed("args", TsType::named("__JsonValue")));
        case_body.push(try_catch_stmt(
            vec![TsStmt::assign(
                ident("args"),
                as_expr(
                    method_call(ident("globalThis.JSON"), "parse", vec![ident("__body")]),
                    TsType::named("__JsonValue"),
                ),
                None,
            )],
            None,
            vec![return_(Some(json_response(
                json_error_kind(
                    check_kind(CallCheck::MalformedJson),
                    vec![("details".to_string(), str_lit(wf::MALFORMED_JSON_DETAILS))],
                ),
                check_status(CallCheck::MalformedJson),
            )))],
        ));
        case_body.extend(emit_call_handler_dispatch(
            sname,
            h,
            &table.actors,
            runtime_use,
        ));
        call_names.push(sname.to_string());
        call_cases.push(case_(str_lit(sname.to_string()), case_body));
    }
    call_cases.push(default_case(vec![return_(Some(miss(
        member_mode,
        text_response(
            CallCheck::UnknownService.text_body().unwrap_or_default(),
            check_status(CallCheck::UnknownService),
        ),
    )))]));
    if member_mode && call_names.is_empty() {
        return None;
    }
    let gates = std::iter::once(call_method_gate())
        .chain(verify_gate(context, has_foreign))
        .chain(wire_version_gate())
        .chain([
            const_(
                "servicePath",
                method_call(
                    ident("path"),
                    "slice",
                    vec![member(str_lit(wf::CALL_PATH_PREFIX), "length")],
                ),
            ),
            switch_stmt(ident("servicePath"), call_cases),
        ])
        .collect();
    Some(if_(
        own_path_guard(member_mode, wf::CALL_PATH_PREFIX, &call_names),
        block(gates),
    ))
}

/// L7 (#2414, Decision C): an opted-in Worker's `fetch`, which runs what
/// `__bynkFetch` answers through `edgeCacheGuard`. The guard can answer with a
/// copy, which would drop a member's miss mark, so a member re-marks the copy
/// when the answer it guarded was a miss.
fn edge_cache_return(member_mode: bool) -> Vec<TsStmt> {
    let answer = await_expr(call(
        ident("__bynkFetch"),
        vec![ident("request"), ident("env"), ident("ctx")],
    ));
    let guarded =
        |response: TsExpr| call(ident("__edgeCacheGuard"), vec![ident("request"), response]);
    if !member_mode {
        return vec![return_(Some(guarded(answer)))];
    }
    vec![
        const_("__answer", answer),
        const_("__guarded", guarded(ident("__answer"))),
        return_(Some(cond_expr(
            call(ident("isMiss"), vec![ident("__answer")]),
            call(ident("__miss"), vec![ident("__guarded")]),
            ident("__guarded"),
        ))),
    ]
}

/// Events track, slice 0 (spine #936, ADR 0284): the re-export of the fan-out DO,
/// which lives in its own file (`events_fanout.ts`, not `handlers.ts`; a fan-out
/// DO has no backing `AgentDecl` for `emit_agent` to emit it from). A group's one
/// fan-out is exported by the group's entry, not a member's (#3562).
fn fanout_reexport(
    context: &str,
    layout: &bynk_project::worker_layout::WorkerLayout,
    wanted: bool,
) -> Option<TsStmt> {
    wanted.then(|| {
        TsStmt::decl(
            TsDecl::ReExport {
                names: vec![crate::emitter::wrangler::EVENTS_FANOUT_CLASS_NAME.to_string()],
                from: layout.own_module_specifier(context, "events_fanout.ts"),
            },
            None,
        )
    })
}

#[allow(clippy::too_many_arguments)]
pub fn emit_worker_entry(
    context: &str,
    layout: &bynk_project::worker_layout::WorkerLayout,
    table: &UnitTable,
    // v0.177 (#643): this context's own contract hash per `on call` service.
    own_contracts: &std::collections::BTreeMap<String, String>,
    // #2554: the older contract hashes each service also accepts, from the
    // contracts lock. Absent for a service that accepts only its own.
    accepted_contracts: &std::collections::BTreeMap<String, Vec<String>>,
    // Locale capability track, slice 2 (#882): whether `compose` was widened
    // to take an optional `request` (the bool `emit_worker_compose` itself
    // returned) — only `fetch` has a `Request` in scope, never
    // `scheduled`/`queue`, so the two entry points need distinct compose
    // calls, not one shared string.
    needs_locale_request: bool,
    // #1187's slice 6 plumbing — see `emit_worker_compose`'s own matching
    // parameter (`emitter/workers.rs`) for the full grounding.
    uses_emit: bool,
    // #2136 (ADR DECISION B): the test build of a context a `system` case
    // drives. Its `scheduled` entry stores each cron handler's `Result` on the
    // execution-context double the harness supplies, so the case's
    // `svc.schedule(…)` yields the handler's own result. Never set for a
    // deployable build: that build carries no suites.
    scheduled_result_hook: bool,
    // #2252: the types this context takes from a consumed *context* (not
    // declared here). Each imports type-only (#661), so the entry validates a
    // path parameter of one with this context's own codec, not `<T>.of`.
    consumed_types: &std::collections::HashSet<String>,
    // State-migrations S4 (#2259 Decision D): each versioned agent class this
    // Worker hosts, with its lock version, for the `/_bynk/migrate/` route.
    // Empty when the build has no lock or the context hosts no agent.
    migratable: &[(String, i64)],
    // #2719: whether any of this context's service handlers hands work to the
    // execution context, from the checker's needs-context set.
    ctx_uses_send: bool,
    // #2903: the context's own types plus those of the commons it `uses`, so a
    // query field typed by an imported refined type or enum resolves to its real
    // wire kind rather than falling through to `string`.
    visible_types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    // #3580: wrap `fetch` in the request recorder (`bynk dev --trace-events` only).
    trace: bool,
    // #3581: the dev inspect route's agent classes (else empty), and if this member lists them.
    inspect: (&[String], bool),
) -> TsProgram {
    let member_mode = layout.is_member(context);
    let mut program = TsProgram::new();
    // Which conditional runtime helpers the entry's own inbound/outbound codecs
    // reach for; read back below to decide the import injection.
    let runtime_use = RuntimeUse::default();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment(
        format!("Worker entry point for context `{context}`."),
        None,
    ));

    // v0.79: if any handler uses `detach`, each entry point captures the runtime's
    // execution context and threads it into `compose`, so a fire-and-forget send
    // can hand its promise to `waitUntil`. Otherwise the signatures are unchanged.
    let exec_params: Vec<TsParam> = if ctx_uses_send {
        vec![TsParam::new(
            "ctx".to_string(),
            Some(TsType::Object(vec![TsTypeMember::method(
                "waitUntil",
                vec![TsParam {
                    name: "promise".to_string(),
                    ty: Some(TsType::named_with_args(
                        "globalThis.Promise",
                        vec![TsType::named("unknown")],
                    )),
                    optional: false,
                    rest: false,
                    array_pattern: None,
                    default: None,
                }],
                TsType::named("void"),
            )])),
        )]
    } else {
        Vec::new()
    };
    // L7 (#2414, Decision E): an `EdgeCache` instance purges through the
    // entry's `ctx`, so `compose` takes it as `exec` (optional when nothing
    // sends: an in-process caller may pass none), and so does `scheduled`.
    let edge_cache = table.derived.values().any(|d| d.is_edge_cache());
    let compose_takes_exec = ctx_uses_send || edge_cache;
    // #2192: `fetch` and `queue` always take `ctx`, which their fault handlers
    // report through. Optional when no send needs it, so an in-process caller
    // (the test runner) can still pass just `request`/`batch` and `env`.
    let fault_exec_params: Vec<TsParam> = if ctx_uses_send {
        exec_params.clone()
    } else {
        vec![TsParam::optional(
            "ctx".to_string(),
            Some(TsType::Object(vec![TsTypeMember::method(
                "waitUntil",
                vec![TsParam {
                    name: "promise".to_string(),
                    ty: Some(TsType::named_with_args(
                        "globalThis.Promise",
                        vec![TsType::named("unknown")],
                    )),
                    optional: false,
                    rest: false,
                    array_pattern: None,
                    default: None,
                }],
                TsType::named("void"),
            )])),
        )]
    };
    // `fetch` alone has a `request` in scope; `scheduled`/`queue` never do.
    let fetch_compose_args: Vec<TsExpr> = match (needs_locale_request, compose_takes_exec) {
        (true, true) => vec![ident("env"), ident("request"), ident("ctx")],
        (true, false) => vec![ident("env"), ident("request")],
        (false, true) => vec![ident("env"), ident("ctx")],
        (false, false) => vec![ident("env")],
    };
    let other_compose_args: Vec<TsExpr> = if compose_takes_exec {
        vec![ident("env"), ident("ctx")]
    } else {
        vec![ident("env")]
    };

    // Collect HTTP routes across all services, sorted so literal-segment
    // routes precede parameter-segment routes that share the same prefix.
    let mut http_routes: Vec<HttpRoute> = Vec::new();
    let mut service_names: Vec<&String> = table.services.keys().collect();
    service_names.sort();
    for sname in &service_names {
        let service = table.services.get(*sname).unwrap();
        for h in &service.handlers {
            // Slice 1 of `#1542` (the IR cutover, `design/archive/retired-tracks.md` §5): reads
            // `IrHandlerKind::Http`'s own `method`/`path` directly — the IR
            // payload P6.24a already computes here, no longer re-derived from
            // `h.kind` a second time. `HttpRoute::method` is `IrHttpMethod`
            // (un-deferred from Q7's original "stays AST until phase 7's
            // printer" — phase 7 shipped, see the front-loading ADR).
            if let bynk_model::IrHandlerKind::Http { method, path } =
                bynk_model::lower_handler_kind_ir(&h.kind)
            {
                let query_fields = query_field_specs(h, &path, visible_types);
                let cookies_fields = cookies_field_specs(h, &path, visible_types);
                http_routes.push(HttpRoute {
                    service: (*sname).clone(),
                    method,
                    path,
                    handler: h.clone(),
                    // v0.47: a Bearer handler's surface wrapper runs the
                    // verification seam and needs the request passed in.
                    bearer: bynk_check::actors::bearer_seam_for(h, &table.actors).is_some(),
                    // v0.151: an `Oidc` wrapper runs its verification seam, taking the request first.
                    oidc: bynk_check::actors::oidc_seam_for(h, &table.actors).is_some(),
                    signed_cookie: signed_cookie_name(h, &table.actors),
                    signature: bynk_check::actors::signature_seam_for(h, &table.actors),
                    // v0.52: a multi-actor sum handler's wrapper owns the whole
                    // boundary (raw read, first-wins resolution, body parse), so
                    // the entry just passes `request` and skips the body parse.
                    sum: bynk_check::actors::sum_members_for(h, &table.actors).is_some(),
                    // v0.140 (ADR 0163): the handler's `@cache` freshness policy, if
                    // any — lowered to a `Cache-Control` on this GET's responses.
                    cache: bynk_model::lower_route_cache_ir(h),
                    edge_cache,
                    vary_locale: edge_cache && needs_locale_request,
                    // v0.142 (ADR 0165): the route's effective request-body ceiling
                    // in bytes — the handler's `@limit` if any, else the service's
                    // `limits { }`, else none. `Some` drives the synthesised `413`
                    // `Content-Length` guard before the body is read.
                    max_body: effective_max_body(service.limits.as_ref(), h),
                    raw: h.raw_content_type().map(str::to_string),
                    json_plain: bynk_syntax::ast::effective_json_options(service.json.as_ref(), h)
                        == bynk_syntax::ast::JsonOptions::Plain,
                    rejected: rejected_surface(service, sname),
                    query_fields,
                    form_fields: form_field_specs(h, visible_types),
                    cookies_fields,
                    multipart: multipart_spec(service.limits.as_ref(), h, visible_types),
                });
            }
        }
    }
    http_routes.sort_by(|a, b| {
        // Sort by literal-specificity: a path with fewer parameter segments
        // (more literals) wins. Stable secondary sort by method + path keeps
        // the diff deterministic.
        param_count(&a.path)
            .cmp(&param_count(&b.path))
            .then_with(|| a.method.as_str().cmp(b.method.as_str()))
            .then_with(|| a.path.cmp(&b.path))
    });

    // v0.131 (ADR 0159): the per-service CORS policies. A `from http` service
    // with a `cors { }` section gets one synthesised `CorsPolicy` constant; its
    // routes are answered with a preflight `OPTIONS` branch and stamped with the
    // `Access-Control-*` headers. Allow-methods is derived from the service's
    // routes (never restated); allow-headers defaults to `content-type` (plus
    // `Authorization` when the service has a Bearer route) unless overridden.
    let cors_services: Vec<CorsService> = build_cors_services(&service_names, table, &http_routes);

    // v0.141 (ADR 0164): the per-service security-headers policies. Unlike CORS,
    // *every* `from http` service with routes gets one — the safe header
    // (`nosniff`) is on by default, so a service with no `security { }` still
    // stamps the defaults. Its responses (and the synthesised preflight / `405` /
    // `304`) carry `applySecurityHeaders`, composing with `applyCors`.
    let security_services: Vec<SecurityService> =
        build_security_services(&service_names, table, &http_routes);

    // v0.139 (ADR 0162): the per-path method table driving the method-aware
    // router fall-through — a wrong method to a live path is a `405 + Allow`, a
    // bare `OPTIONS` is a `204 + Allow`, and both are derived from this one table
    // (the same derivation CORS reads for its allow-methods).
    let path_methods: Vec<PathMethods> =
        build_path_method_table(&http_routes, &cors_services, &security_services);

    // v0.10a: collect cron handlers across all services, carrying the per-service
    // declaration index (the method-name key) and sorting by schedule expression
    // for deterministic switch output.
    let mut cron_routes: Vec<CronRoute> = Vec::new();
    for sname in &service_names {
        let service = table.services.get(*sname).unwrap();
        let mut cron_idx = 0usize;
        for h in &service.handlers {
            // P6.31: dispatches on `IrHandlerKind`; `expr` is a plain `String`
            // in both, so no AST re-derivation is needed here (unlike the
            // `Http` arm above).
            if let bynk_model::IrHandlerKind::Cron { expr } =
                bynk_model::lower_handler_kind_ir(&h.kind)
            {
                cron_routes.push(CronRoute {
                    service: (*sname).clone(),
                    index: cron_idx,
                    expr,
                    has_param: !h.params.is_empty(),
                });
                cron_idx += 1;
            }
        }
    }
    cron_routes.sort_by(|a, b| a.expr.cmp(&b.expr));

    // v0.10b: collect queue consumers across all services, carrying the
    // per-service declaration index and sorting by queue name.
    let mut queue_routes: Vec<QueueRoute> = Vec::new();
    for sname in &service_names {
        let service = table.services.get(*sname).unwrap();
        let mut queue_idx = 0usize;
        for h in &service.handlers {
            // P6.31: dispatches on `IrHandlerKind` (no fields to re-derive
            // here). `service.protocol` stays raw `ServiceProtocol` — this
            // function has no `TypedCommons` in scope, the same constraint
            // P6.30 found in `emit_worker_compose`.
            if matches!(
                bynk_model::lower_handler_kind_ir(&h.kind),
                bynk_model::IrHandlerKind::Message
            ) {
                // v0.44: the bound queue name lives on the service header
                // (`from queue("name")`), not on the handler.
                let ServiceProtocol::Queue { name } = &service.protocol else {
                    continue;
                };
                let msg_type = h.params.first().map(|p| p.type_ref.clone());
                queue_routes.push(QueueRoute {
                    service: (*sname).clone(),
                    index: queue_idx,
                    name: name.clone(),
                    msg_type,
                });
                queue_idx += 1;
            }
        }
    }
    queue_routes.sort_by(|a, b| a.name.cmp(&b.name));

    // v0.104 (real-time track slice 3b): the `from websocket` upgrade routes. An
    // `Upgrade: websocket` request dispatches to the service's edge wrapper
    // (`ws_<service>_open`), which authenticates and forwards to the hosting DO.
    // Route params come from the upgrade URL's query string (the v1 convention).
    let mut ws_open_routes: Vec<(&String, &Handler)> = Vec::new();
    for sname in &service_names {
        let service = table.services.get(*sname).unwrap();
        for h in &service.handlers {
            if matches!(
                bynk_model::lower_handler_kind_ir(&h.kind),
                bynk_model::IrHandlerKind::Open
            ) {
                ws_open_routes.push((*sname, h));
            }
        }
    }

    // The runtime import is read off the finished tree (below), so the header is held until the
    // body has been built (1A.36b-2).
    let mut header_decls: Vec<TsStmt> = vec![
        TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names: vec!["compose".to_string(), "type Env".to_string()],
                from: layout.own_module_specifier(context, "compose.ts"),
            },
            None,
        ),
        TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: "handlers".to_string(),
                from: layout.own_handlers_specifier(context),
            },
            None,
        ),
    ];

    // v0.9.2: re-export each agent's Durable Object class from the Worker
    // entry. Cloudflare resolves a `class_name` binding against the named
    // exports of the Worker's `main`, so the DO classes declared in
    // `handlers.ts` must be visible here.
    let mut agent_names: Vec<&String> = table.agents.keys().collect();
    agent_names.sort();
    //
    // #3202: each is exported wrapped, so the instance runs the handlers that
    // reach it one at a time (`__serialiseAgentClass`, `runtime/src/agent.ts`).
    for name in &agent_names {
        header_decls.push(TsStmt::decl(
            TsDecl::Export(Box::new(TsDecl::ConstDecl {
                name: name.to_string(),
                ty: None,
                init: call(
                    ident("__serialiseAgentClass"),
                    vec![member(ident("handlers"), name.as_str())],
                ),
            })),
            None,
        ));
    }
    header_decls.extend(fanout_reexport(context, layout, uses_emit && !member_mode));

    header_decls.extend(container_class_reexport(table, member_mode));
    header_decls.extend(inspect_registry_export(inspect.0));

    let mut fetch_params: Vec<TsParam> = vec![
        TsParam::new(
            "request".to_string(),
            Some(TsType::named("globalThis.Request")),
        ),
        TsParam::new("env".to_string(), Some(TsType::named("Env"))),
    ];
    fetch_params.extend(fault_exec_params.iter().cloned());

    let mut fetch_body: Vec<TsStmt> = vec![
        const_(
            "url",
            new_expr("globalThis.URL", vec![member(ident("request"), "url")]),
        ),
        const_("path", member(ident("url"), "pathname")),
        const_("method", member(ident("request"), "method")),
        const_(
            "surface",
            call(ident("compose"), fetch_compose_args.clone()),
        ),
    ];
    // v0.131: the synthesised CORS policy constants, one per CORS-enabled service.
    for cs in &cors_services {
        fetch_body.push(const_typed(
            cs.const_name.clone(),
            TsType::named("__CorsPolicy"),
            cs.literal.clone(),
        ));
    }
    // v0.141: the synthesised security-headers policy constants, one per
    // `from http` service (with routes) — the default `nosniff` and any opt-in HSTS.
    for ss in &security_services {
        fetch_body.push(const_typed(
            ss.const_name.clone(),
            TsType::named("__SecurityPolicy"),
            ss.literal.clone(),
        ));
    }

    let mut try_body: Vec<TsStmt> = Vec::new();

    // 1. Internal Service Binding dispatch.
    //
    // I3 (#1971): the foreign callers any `on call` handler here admits, by
    // actor name. Empty for a Worker that admits none, whose dispatch then
    // admits Bynk contexts alone, as before.
    let mut foreign_callers: Vec<bynk_check::actors::ForeignCaller> = Vec::new();
    for sname in &service_names {
        for h in &table.services[*sname].handlers {
            for f in bynk_check::actors::call_admission_for(h, &table.actors)
                .map(|a| a.foreign)
                .unwrap_or_default()
            {
                if !foreign_callers.iter().any(|g| g.actor == f.actor) {
                    foreign_callers.push(f);
                }
            }
        }
    }
    let has_foreign = !foreign_callers.is_empty();
    if has_foreign {
        fetch_body.push(const_typed(
            FOREIGN_CALLERS,
            TsType::named_with_args(
                "globalThis.Record",
                vec![TsType::named("string"), TsType::named("string")],
            ),
            TsExpr::object(
                foreign_callers
                    .iter()
                    .map(|f| (f.actor.clone(), str_lit(f.secret.clone())))
                    .collect(),
            ),
        ));
    }
    // The foreign callers any `on call` handler admits are `foreign_callers`
    // above; `has_foreign` makes the dispatch check them.
    try_body.extend(call_dispatch(
        context,
        member_mode,
        table,
        &service_names,
        (own_contracts, accepted_contracts),
        has_foreign,
        &runtime_use,
    ));
    try_body.push(TsStmt::blank(None));

    // 1.25. The internal admin routes: see `admin_routes`.
    try_body.extend(admin_routes(context, member_mode, migratable, inspect));

    // 1.5. WebSocket upgrade dispatch (v0.104, slice 3b). An `Upgrade: websocket`
    // request routes to the `from websocket` service's edge wrapper, which runs the
    // fail-closed auth seam and forwards to the hosting Durable Object. Route params
    // are read from the upgrade URL's query string by name (the v1 convention; a
    // missing required param is a `400`).
    if !ws_open_routes.is_empty() {
        let mut ws_body: Vec<TsStmt> = Vec::new();
        for (sname, h) in &ws_open_routes {
            // #1825 review: the auth seam and the DO forward run in the `try`
            // too, so name the dispatch for the fault log.
            ws_body.push(set_fault_route(format!("ws {sname}")));
            let mut args: Vec<TsExpr> = vec![ident("request")];
            for p in &h.params {
                let pn = &p.name.name;
                ws_body.push(const_(
                    format!("__ws_{pn}"),
                    method_call(
                        member(ident("url"), "searchParams"),
                        "get",
                        vec![str_lit(pn.clone())],
                    ),
                ));
                ws_body.push(if_(
                    strict_eq(ident(format!("__ws_{pn}")), null_lit()),
                    return_(Some(text_response(
                        &format!("Missing parameter: {pn}"),
                        400,
                    ))),
                ));
                args.push(ident(format!("__ws_{pn}")));
            }
            ws_body.push(return_(Some(call(
                member(ident("surface"), format!("ws_{sname}_open")),
                args,
            ))));
        }
        try_body.push(if_(
            strict_eq(
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("Upgrade")],
                ),
                str_lit("websocket"),
            ),
            block(ws_body),
        ));
        try_body.push(TsStmt::blank(None));
    }

    // 1.6. Events dispatch (spine #936, ADR 0284). Reached only from a
    // publishing context's fan-out DO calling in over this subscriber's
    // Service Binding (`deliverEvent`) — never from external edge traffic, so
    // it needs no CORS/actor handling, unlike the HTTP routes below.
    let event_services: Vec<&String> = service_names
        .iter()
        .filter(|sname| {
            table.services.get(**sname).is_some_and(|s| {
                s.handlers.iter().any(|h| {
                    matches!(
                        bynk_model::lower_handler_kind_ir(&h.kind),
                        bynk_model::IrHandlerKind::Event
                    )
                })
            })
        })
        .copied()
        .collect();
    if !event_services.is_empty() {
        let mut event_body: Vec<TsStmt> = verify_gate(context, false).into_iter().collect();
        event_body.extend([
            const_(
                "servicePath",
                method_call(
                    ident("path"),
                    "slice",
                    vec![member(str_lit("/_bynk/event/"), "length")],
                ),
            ),
            // Slice 2 (spine #936): the fan-out DO's `deliverEvent` always sends
            // `{ payload, envelope }` — the envelope is minted at emission
            // (`lower.rs`'s `__events.push`) and forwarded unconditionally over
            // this hop. Whether a *specific* subscriber's handler actually
            // wants it is decided by that subscriber's own generated wrapper
            // (`emit_event_wrapper`), not here — this route passes both through
            // uniformly regardless of which `case` it dispatches to.
            //
            // #973: until this fix, `payload`/`envelope` were cast `as unknown`
            // and forwarded unchecked — nothing on this whole path ever called a
            // `deserialise_*` function, so a malformed event silently reached the
            // subscriber's handler body. Both are now validated here, the same
            // validate-then-400 shape `/_bynk/call/` above already uses: the
            // envelope once, unconditionally (it's always on the wire regardless
            // of whether a given subscriber declared `env`), then the payload
            // per-`case`, against that subscriber's own generated codec (change A
            // in #973 — the codec didn't previously exist at all for a pure
            // subscriber, since it calls no method on the publisher).
            TsStmt::const_stmt(
                TsBindingName::ObjectPattern(vec!["payload".to_string(), "envelope".to_string()]),
                None,
                as_expr(
                    // I2 (#1971): the text `__verifyCall` read and verified.
                    method_call(ident("globalThis.JSON"), "parse", vec![ident("__body")]),
                    TsType::Object(vec![
                        TsTypeMember::prop("payload", TsType::named("__JsonValue")),
                        TsTypeMember::prop("envelope", TsType::named("__JsonValue")),
                    ]),
                ),
                None,
            ),
            const_(
                "__r_envelope",
                call(
                    ident("__deserialiseEventEnvelope"),
                    vec![ident("envelope"), str_lit("$.envelope")],
                ),
            ),
        ]);
        event_body.push(if_(
            strict_eq(member(ident("__r_envelope"), "tag"), str_lit("Err")),
            return_(Some(json_response(
                member(ident("__r_envelope"), "error"),
                400,
            ))),
        ));

        let mut event_cases: Vec<TsSwitchCase> = Vec::new();
        for sname in &event_services {
            let h = table.services[*sname]
                .handlers
                .iter()
                .find(|h| {
                    matches!(
                        bynk_model::lower_handler_kind_ir(&h.kind),
                        bynk_model::IrHandlerKind::Event
                    )
                })
                .expect("event_services filtered to services with an Event handler");
            let dser_payload =
                deserialise_call(&h.params[0].type_ref, "payload", "$.payload", &runtime_use);
            let case_body = vec![
                // #1825 review: a subscriber that throws is a fault the
                // publisher's fan-out retries; name it for the log.
                set_fault_route(format!("event {sname}")),
                const_("__r_payload", dser_payload),
                if_(
                    strict_eq(member(ident("__r_payload"), "tag"), str_lit("Err")),
                    return_(Some(json_response(
                        member(ident("__r_payload"), "error"),
                        400,
                    ))),
                ),
                expr_stmt(await_expr(call(
                    member(ident("surface"), sname.to_string()),
                    vec![
                        member(ident("__r_payload"), "value"),
                        member(ident("__r_envelope"), "value"),
                    ],
                ))),
                return_(Some(null_response(204))),
            ];
            event_cases.push(case_(str_lit(sname.to_string()), case_body));
        }
        event_cases.push(default_case(vec![return_(Some(miss(
            member_mode,
            text_response("Not found", 404),
        )))]));
        event_body.push(switch_stmt(ident("servicePath"), event_cases));

        let event_names: Vec<String> = event_services.iter().map(|s| s.to_string()).collect();
        try_body.push(if_(
            own_path_guard(member_mode, "/_bynk/event/", &event_names),
            block(event_body),
        ));
        try_body.push(TsStmt::blank(None));
    }

    // v0.131 (ADR 0159): CORS preflight. An `OPTIONS` against any route path of a
    // CORS-enabled service is answered here — before the route dispatch and its
    // auth seam, since a preflight is credential-less by spec and must not be
    // rejected by a `by` actor / Bearer check.
    //
    // v0.139 (ADR 0162 D4): a *real* preflight is distinguished from a bare
    // discovery `OPTIONS` by the `Access-Control-Request-Method` header. A
    // preflight (has it) is answered here with the `Access-Control-*` grant; a
    // bare `OPTIONS` (lacks it) falls through to the generic `204 + Allow` below,
    // so the two `OPTIONS` answers compose instead of colliding.
    for cs in &cors_services {
        let mut cond_terms = cs.paths.iter().map(|(path, has_params)| {
            if *has_params {
                strict_neq(
                    call(
                        ident("__matchPath"),
                        vec![str_lit(path.clone()), ident("path")],
                    ),
                    null_lit(),
                )
            } else {
                strict_eq(ident("path"), str_lit(path.clone()))
            }
        });
        let first = cond_terms
            .next()
            .expect("a CORS service always has ≥1 route");
        let cond = cond_terms.fold(first, or_expr);
        let full_cond = and_expr(
            and_expr(
                strict_eq(ident("method"), str_lit("OPTIONS")),
                strict_neq(
                    method_call(
                        member(ident("request"), "headers"),
                        "get",
                        vec![str_lit("access-control-request-method")],
                    ),
                    null_lit(),
                ),
            ),
            paren(cond),
        );
        // v0.141 (ADR 0164 DECISION E): the synthesised preflight also carries the
        // service's security headers — a CORS-enabled service is a `from http`
        // service, so it always has a security policy constant.
        let preflight = call(
            ident("__corsPreflightResponse"),
            vec![
                ident(cs.const_name.clone()),
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("origin")],
                ),
            ],
        );
        let stamped = match security_services.iter().find(|s| s.service == cs.service) {
            Some(ss) => call(
                ident("__applySecurityHeaders"),
                vec![preflight, ident(ss.const_name.clone())],
            ),
            None => preflight,
        };
        try_body.push(if_(full_cond, block(vec![return_(Some(stamped))])));
    }

    // 2. External HTTP routes.
    for route in &http_routes {
        let cors_const = cors_services
            .iter()
            .find(|cs| cs.service == route.service)
            .map(|cs| cs.const_name.as_str());
        // v0.141: every `from http` route's service has a security policy constant.
        let security_const = security_services
            .iter()
            .find(|ss| ss.service == route.service)
            .map(|ss| ss.const_name.as_str());
        try_body.push(emit_http_route_dispatch(
            route,
            cors_const,
            security_const,
            &runtime_use,
            consumed_types,
        ));
    }

    // v0.139 (ADR 0162): the method-aware router fall-through. A request that
    // matched no dispatch block is tested against each known path; a match means
    // a live path reached under an unhandled method, so a bare `OPTIONS` is a
    // `204 + Allow` and any other method is a `405 + Allow` (the `Allow` derived
    // from the route table). For a CORS-enabled path the synthesised response is
    // stamped with `applyCors` (D5) so a cross-origin `405`/`OPTIONS` is not
    // invisible to the browser. No path matches ⇒ the `404` as before.
    for pm in &path_methods {
        let allow = pm.methods.join(", ");
        let match_cond = if pm.has_params {
            strict_neq(
                call(
                    ident("__matchPath"),
                    vec![str_lit(pm.path.clone()), ident("path")],
                ),
                null_lit(),
            )
        } else {
            strict_eq(ident("path"), str_lit(pm.path.clone()))
        };
        let mut fallthrough_body: Vec<TsStmt> = Vec::new();
        // OPTIONS → 204, everything else → 405; both carry the derived Allow.
        fallthrough_body.push(const_(
            "__status",
            cond_expr(
                strict_eq(ident("method"), str_lit("OPTIONS")),
                num_lit("204"),
                num_lit("405"),
            ),
        ));
        let res = new_response(vec![
            null_lit(),
            TsExpr::object(vec![
                ("status".to_string(), ident("__status")),
                (
                    "headers".to_string(),
                    TsExpr::object(vec![("allow".to_string(), str_lit(allow))]),
                ),
            ]),
        ]);
        // #2341 (decision C): a `405` is rendered through the owning service's
        // `on rejected` (the `204` to `OPTIONS` is not a rejection, and passes
        // through untouched).
        let res = match &pm.rejected {
            Some(m) => await_expr(call(
                ident("__renderRejection"),
                vec![res, ident(format!("(why) => surface.{m}(why)"))],
            )),
            None => res,
        };
        fallthrough_body.push(const_("__res", res));
        // Stamp CORS (v0.131) then security headers (v0.141) — disjoint sets, so
        // the order is immaterial; every `from http` path carries the security
        // policy (DECISION E), CORS only when the path's service opts in.
        let cors_res = match &pm.cors_const {
            Some(c) => call(
                ident("__applyCors"),
                vec![
                    ident("__res"),
                    ident(c.clone()),
                    method_call(
                        member(ident("request"), "headers"),
                        "get",
                        vec![str_lit("origin")],
                    ),
                ],
            ),
            None => ident("__res"),
        };
        let stamped = match &pm.security_const {
            Some(s) => call(
                ident("__applySecurityHeaders"),
                vec![cors_res, ident(s.clone())],
            ),
            None => cors_res,
        };
        fallthrough_body.push(return_(Some(miss(member_mode, stamped))));
        try_body.push(if_(match_cond, block(fallthrough_body)));
    }

    try_body.push(return_(Some(miss(
        member_mode,
        text_response("Not Found", 404),
    ))));

    // #1825: a fault answers a bare 500, never leaking the error to the
    // client (#184), but it is logged here first, with the context and the
    // dispatch that faulted: the route *pattern* or `call <service>`, never
    // the request, so no key or value reaches the log. Caught, it is no
    // uncaught exception the platform would record, so without this a fault
    // left no trace at all. #2053: the log carries the error's stack
    // (`fault_detail`), not its bare string, so a tail shows the frames.
    // #2192: they stay `index.js:L:C`, since Cloudflare maps an uploaded
    // source map only onto an exception it records, so the fault is also
    // reported (`__bynkReportFault`). It does not rethrow: that would replace
    // this 500 with the platform's own error page, and the queue handler's
    // per-message `retry()` with a whole-batch retry. #2193: the 500 comes
    // from `__faultResponse`, which marks a fault in an `on call` service with
    // this context's name, so the calling Worker reports `CalleeFaulted`
    // naming it rather than a bare `Transport`.
    fetch_body.push(TsStmt::let_stmt(
        TsBindingName::Ident("__route".to_string()),
        Some(TsType::named("string")),
        Some(str_lit("request")),
        None,
    ));
    fetch_body.push(try_catch_stmt(
        try_body,
        Some("e"),
        fault_handler(context, trace),
    ));

    let fetch_return = Some(TsType::named_with_args(
        "globalThis.Promise",
        vec![TsType::named("globalThis.Response")],
    ));
    // L7 (#2414, Decision C): an opted-in Worker's dispatch moves into
    // `__bynkFetch`, and `fetch` runs whatever it answers through
    // `edgeCacheGuard`, so the router's own answers (a `404`, a `405`, a
    // rejection, a fault) say `no-store` too.
    let fetch_body = if edge_cache {
        header_decls.push(TsStmt::decl(
            TsDecl::Function {
                name: "__bynkFetch".to_string(),
                generics: Vec::new(),
                params: fetch_params.clone(),
                return_type: fetch_return.clone(),
                body: fetch_body,
                is_async: true,
                inline: false,
            },
            None,
        ));
        edge_cache_return(member_mode)
    } else {
        fetch_body
    };
    let fetch_body = if trace {
        traced_fetch(
            context,
            &mut header_decls,
            &fetch_params,
            &fetch_return,
            fetch_body,
        )
    } else {
        fetch_body
    };
    let mut default_entries: Vec<TsObjectEntry> = vec![TsObjectEntry::method(
        "fetch",
        true,
        fetch_params,
        fetch_return,
        fetch_body,
    )];

    // v0.10a: scheduled (cron) entry point. Dispatches on `event.cron`. A
    // failing run has no retry channel, so an `Err` is logged and the run
    // completes (v0.10 §5.1, [DECISION 3]).
    if !cron_routes.is_empty() {
        default_entries.push(emit_scheduled_handler(
            &cron_routes,
            if edge_cache {
                &fault_exec_params
            } else {
                &exec_params
            },
            &other_compose_args,
            scheduled_result_hook,
        ));
    }

    // v0.10b: the queue (consumer) entry point; #3328: the email entry point.
    push_message_entries(
        table,
        &queue_routes,
        &fault_exec_params,
        &other_compose_args,
        &runtime_use,
        &mut default_entries,
    );

    let runtime_import_at = program.stmts.len();
    for decl in header_decls {
        program.push(decl);
    }
    if member_mode {
        // 1A.38: fixed authored snippet, no interpolation
        program.push(TsStmt::raw_using(MISS_HELPER, &[]));
    }
    // 1A.38: fixed authored snippet, no interpolation
    program.push(TsStmt::raw_using(REPORT_FAULT_HELPER, &[]));
    program.push(TsStmt::decl(
        TsDecl::ExportDefault(TsExpr::multiline_object_entries(default_entries)),
        None,
    ));

    // 1A.36b-2: the runtime import is what the module's tree uses, after the banner and before
    // the other imports.
    let names =
        super::runtime_header::derived_worker_runtime_names(&program.stmts[runtime_import_at..]);
    for (i, stmt) in super::runtime_files::runtime_import_stmts(false, &names, "../../runtime.js")
        .into_iter()
        .enumerate()
    {
        program.stmts.insert(runtime_import_at + i, stmt);
    }
    // Each fixed import (`handlers`, `compose`) is kept only if the tree uses it.
    program.prune_unused_imports();

    program
}

/// Emit the Worker `scheduled` handler aggregating every `on cron` handler in
/// the context. `event` is typed structurally (`{ cron: string }`) to avoid a
/// dependency on `@cloudflare/workers-types`, matching how the rest of the
/// emitter hand-declares the minimal ambient shapes it needs.
///
/// #2136: with `result_hook` (the test build of a context a `system` case
/// drives), the entry takes the execution context even when no handler sends,
/// and stores each run's `Result` on it as `__bynkResult` for the harness's
/// driver to read back. The deployable entry is unchanged.
fn emit_scheduled_handler(
    cron_routes: &[CronRoute],
    exec_params: &[TsParam],
    compose_args: &[TsExpr],
    result_hook: bool,
) -> TsObjectEntry {
    let mut params = vec![TsParam::new(
        "event".to_string(),
        Some(TsType::Object(vec![
            TsTypeMember::readonly_prop("cron", TsType::named("string")),
            TsTypeMember::readonly_prop("scheduledTime", TsType::named("number")),
        ])),
    )];
    params.push(TsParam::new("env".to_string(), Some(TsType::named("Env"))));
    let result_member = TsTypeMember::optional_prop("__bynkResult", TsType::named("unknown"));
    if !result_hook {
        params.extend(exec_params.iter().cloned());
    } else if let Some(ctx) = exec_params.first() {
        // The sending entry's `ctx`, widened with the result slot.
        let mut ctx = ctx.clone();
        if let Some(TsType::Object(members)) = &mut ctx.ty {
            members.push(result_member);
        }
        params.push(ctx);
    } else {
        params.push(TsParam::optional(
            "ctx".to_string(),
            Some(TsType::Object(vec![result_member])),
        ));
    }
    let ctx_optional = exec_params.first().is_none_or(|p| p.optional);

    let mut body: Vec<TsStmt> = vec![const_(
        "surface",
        call(ident("compose"), compose_args.to_vec()),
    )];
    let mut cases: Vec<TsSwitchCase> = Vec::new();
    for route in cron_routes {
        let method_key = crate::emitter::cron_handler_method_name(&route.service, route.index);
        // Pass the scheduled fire time (epoch ms) when the handler asked for it.
        let args: Vec<TsExpr> = if route.has_param {
            vec![member(ident("event"), "scheduledTime")]
        } else {
            vec![]
        };
        let mut case_body = vec![const_(
            "result",
            await_expr(call(member(ident("surface"), method_key), args)),
        )];
        if result_hook {
            let store = TsStmt::assign(member(ident("ctx"), "__bynkResult"), ident("result"), None);
            case_body.push(if ctx_optional {
                if_(strict_neq(ident("ctx"), ident("undefined")), store)
            } else {
                store
            });
        }
        case_body.extend([
            if_(
                strict_eq(member(ident("result"), "tag"), str_lit("Err")),
                expr_stmt(method_call(
                    ident("globalThis.console"),
                    "error",
                    vec![
                        str_lit(format!("cron {} failed", route.expr)),
                        member(ident("result"), "error"),
                    ],
                )),
            ),
            return_(None),
        ]);
        cases.push(case_(str_lit(route.expr.clone()), case_body));
    }
    cases.push(default_case(vec![return_(None)]));
    body.push(switch_stmt(member(ident("event"), "cron"), cases));

    TsObjectEntry::Method {
        name: "scheduled".to_string(),
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: Some(TsType::named_with_args(
            "globalThis.Promise",
            vec![TsType::named("void")],
        )),
        doc: None,
        inline: false,
        body,
        span: None,
    }
}

/// The queue (v0.10b) and inbound-email (#3328) entry points. `queue` dispatches
/// on `batch.queue`, deserialises each message, invokes the handler, and acks on
/// `Ok` / retries on `Err` (a deserialisation failure also retries); `email`
/// hands the message to the runtime with one route per `from email` service.
fn push_message_entries(
    table: &UnitTable,
    queue_routes: &[QueueRoute],
    exec_params: &[TsParam],
    compose_args: &[TsExpr],
    runtime_use: &RuntimeUse,
    entries: &mut Vec<TsObjectEntry>,
) {
    if !queue_routes.is_empty() {
        entries.push(emit_queue_handler(
            queue_routes,
            exec_params,
            compose_args,
            runtime_use,
        ));
    }
    let email_routes = super::workers_email::collect_routes(table);
    if !email_routes.is_empty() {
        entries.push(super::workers_email::emit_email_handler(
            &email_routes,
            exec_params,
            compose_args,
        ));
    }
}

/// Emit the Worker `queue` handler aggregating every `on queue` consumer in the
/// context. Dispatches on `batch.queue`; for each message it deserialises the
/// body (v0.8 wire-format), invokes the handler, and acks on `Ok` / retries on
/// `Err`. A deserialisation failure or a thrown error also retries. `batch` is
/// typed structurally to avoid a `@cloudflare/workers-types` dependency.
fn emit_queue_handler(
    queue_routes: &[QueueRoute],
    exec_params: &[TsParam],
    compose_args: &[TsExpr],
    ru: &RuntimeUse,
) -> TsObjectEntry {
    let mut params = vec![TsParam::new(
        "batch".to_string(),
        Some(TsType::Object(vec![
            TsTypeMember::readonly_prop("queue", TsType::named("string")),
            TsTypeMember::readonly_prop(
                "messages",
                TsType::named_with_args(
                    "ReadonlyArray",
                    vec![TsType::Object(vec![
                        TsTypeMember::readonly_prop("body", TsType::named("unknown")),
                        TsTypeMember::method("ack", vec![], TsType::named("void")),
                        TsTypeMember::method("retry", vec![], TsType::named("void")),
                    ])],
                ),
            ),
        ])),
    )];
    params.push(TsParam::new("env".to_string(), Some(TsType::named("Env"))));
    params.extend(exec_params.iter().cloned());

    let mut body: Vec<TsStmt> = vec![const_(
        "surface",
        call(ident("compose"), compose_args.to_vec()),
    )];
    let mut cases: Vec<TsSwitchCase> = Vec::new();
    for route in queue_routes {
        let method_key = crate::emitter::queue_handler_method_name(&route.service, route.index);
        let dser = match &route.msg_type {
            Some(t) => deserialise_node(
                t,
                &TsExpr::Paren(Box::new(as_expr(
                    member(ident("msg"), "body"),
                    TsType::named("__JsonValue"),
                ))),
                "$",
                ru,
            ),
            // P7.2: `msg.body` is already declared `unknown` at `queue()`'s own
            // signature above — no cast needed at all when there's no declared type
            // to (dis)trust it against.
            None => as_expr(
                call(ident("Ok"), vec![member(ident("msg"), "body")]),
                TsType::named("Result<unknown, __BoundaryError>"),
            ),
        };
        let try_stmts = vec![
            const_("__r", dser),
            // `{ ...; ...; continue; }` — a single-line braced block, this
            // file's own real (hand-written) shape, not the tree's usual
            // one-statement-per-line `Block`.
            if_(
                strict_eq(member(ident("__r"), "tag"), str_lit("Err")),
                TsStmt::inline_block(
                    vec![
                        expr_stmt(method_call(
                            ident("globalThis.console"),
                            "error",
                            vec![
                                str_lit(format!("queue {} deserialise failed", route.name)),
                                member(ident("__r"), "error"),
                            ],
                        )),
                        expr_stmt(method_call(ident("msg"), "retry", vec![])),
                        TsStmt::continue_stmt(None),
                    ],
                    None,
                ),
            ),
            const_(
                "result",
                await_expr(call(
                    member(ident("surface"), method_key),
                    vec![member(ident("__r"), "value")],
                )),
            ),
            TsStmt::if_else_stmt(
                strict_eq(member(ident("result"), "tag"), str_lit("Ack")),
                expr_stmt(method_call(ident("msg"), "ack", vec![])),
                TsStmt::inline_block(
                    vec![
                        expr_stmt(method_call(
                            ident("globalThis.console"),
                            "error",
                            vec![
                                str_lit(format!("queue {} retry", route.name)),
                                member(ident("result"), "reason"),
                            ],
                        )),
                        expr_stmt(method_call(ident("msg"), "retry", vec![])),
                    ],
                    None,
                ),
                None,
            ),
        ];
        // The catch clause's own real content packs both statements onto
        // one compact line (`console.error(...); msg.retry();`) inside the
        // usual braces-on-their-own-lines shape — `TsStmt::try_catch`
        // directly, not the `try_catch_stmt` convenience helper, since this
        // is the one real site in this file needing `InlineBlock` here
        // rather than the ordinary one-statement-per-line `Block`.
        let for_of_body = TsStmt::try_catch(
            block(try_stmts),
            Some("e"),
            TsStmt::inline_block(
                vec![
                    expr_stmt(method_call(
                        ident("globalThis.console"),
                        "error",
                        vec![
                            str_lit(format!("queue {} threw", route.name)),
                            fault_detail("e"),
                        ],
                    )),
                    report_fault(),
                    expr_stmt(method_call(ident("msg"), "retry", vec![])),
                ],
                None,
            ),
            None,
        );
        let case_body = vec![
            TsStmt::for_of(
                "msg",
                member(ident("batch"), "messages"),
                // The `for...of`'s own body is a braced block containing the
                // `try`/`catch` as one statement — passing the `try`/`catch`
                // directly (not wrapped in `block(...)`) would hit
                // `render_branch`'s brace-free inline fallback instead,
                // resetting to depth 0 (real bug, caught by the fixture
                // diff: `render_inline_stmt`'s own `TryCatch` arm falls back
                // to `render_stmt(out, stmt, 0)`, matching neither this
                // loop's real nesting nor the original's own braced shape).
                block(vec![for_of_body]),
                None,
            ),
            return_(None),
        ];
        cases.push(case_(str_lit(route.name.clone()), case_body));
    }
    cases.push(default_case(vec![return_(None)]));
    body.push(switch_stmt(member(ident("batch"), "queue"), cases));

    TsObjectEntry::Method {
        name: "queue".to_string(),
        is_async: true,
        generics: Vec::new(),
        params,
        return_type: Some(TsType::named_with_args(
            "globalThis.Promise",
            vec![TsType::named("void")],
        )),
        doc: None,
        inline: false,
        body,
        span: None,
    }
}

/// Count the number of `:param` segments in a path pattern.
fn param_count(path: &str) -> usize {
    path.split('/')
        .filter(|s| s.starts_with(':') && s.len() > 1)
        .count()
}

/// A CORS-enabled service and its synthesised policy (v0.131, ADR 0159). Carries
/// the emitted `CorsPolicy` object literal, the constant name it binds to, and
/// the service's distinct route paths (with a param flag) so the preflight branch
/// can match any of them.
struct CorsService {
    service: String,
    const_name: String,
    literal: TsExpr,
    paths: Vec<(String, bool)>,
}

/// v0.139 (ADR 0162 D2): the one derivation of the methods a path answers,
/// shared by the CORS preflight's allow-methods, the `Allow` header on a
/// synthesised `405`/`OPTIONS`, and the `HEAD`-from-`GET` synthesis. It is the
/// union of the methods declared on the path, plus `HEAD` when `GET` is present,
/// plus `OPTIONS` always. The `BTreeSet` yields a stable alphabetical order
/// (`DELETE, GET, HEAD, OPTIONS, PATCH, POST, PUT`) — so `Allow: GET, HEAD,
/// OPTIONS` and `Allow: OPTIONS, POST` read as the proposal specifies.
fn derive_allowed_methods(methods: impl Iterator<Item = bynk_model::IrHttpMethod>) -> Vec<String> {
    let mut set: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut has_get = false;
    for m in methods {
        let s = m.as_str().to_string();
        if s == "GET" {
            has_get = true;
        }
        set.insert(s);
    }
    if has_get {
        set.insert("HEAD".to_string());
    }
    set.insert("OPTIONS".to_string());
    set.into_iter().collect()
}

/// #2341 (decision C): the compose-surface method a service's `on rejected`
/// is reached by, when it declares one.
fn rejected_surface(service: &bynk_syntax::ast::ServiceDecl, sname: &str) -> Option<String> {
    service
        .handlers
        .iter()
        .any(|h| h.kind == bynk_syntax::ast::HandlerKind::Rejected)
        .then(|| format!("rejected_{sname}"))
}

/// v0.139 (ADR 0162): a distinct route path and the method set it answers, the
/// table the method-aware router fall-through reads. `cors_const` names the
/// owning CORS-enabled service's policy constant (if any) so a synthesised
/// `405`/`OPTIONS` for a cross-origin path is stamped with `applyCors` (D5).
struct PathMethods {
    path: String,
    has_params: bool,
    /// Alphabetical, e.g. `["GET", "HEAD", "OPTIONS"]`.
    methods: Vec<String>,
    cors_const: Option<String>,
    /// v0.141 (ADR 0164): the owning service's security policy constant, so a
    /// synthesised `405`/`OPTIONS` for the path is stamped with the same headers
    /// its real responses carry (DECISION E). `Some` for every `from http` path.
    security_const: Option<String>,
    /// #2341 (decision C): the owning service's `on rejected` surface method,
    /// through which the synthesised `405` is rendered.
    rejected: Option<String>,
}

/// Build the per-path method table across *every* `from http` service in the
/// context (unlike `build_cors_services`, which is scoped to CORS-enabled
/// services). Paths are emitted in sorted order for a deterministic router.
fn build_path_method_table(
    http_routes: &[HttpRoute],
    cors_services: &[CorsService],
    security_services: &[SecurityService],
) -> Vec<PathMethods> {
    let mut paths: Vec<String> = Vec::new();
    for r in http_routes {
        if !paths.contains(&r.path) {
            paths.push(r.path.clone());
        }
    }
    paths.sort();
    paths
        .into_iter()
        .map(|path| {
            let methods = derive_allowed_methods(
                http_routes
                    .iter()
                    .filter(|r| r.path == path)
                    .map(|r| r.method),
            );
            let has_params = param_count(&path) > 0;
            // A path is expected unique across a context's HTTP surface (the same
            // note ADR 0159 makes); if two services declare it, the first CORS
            // owner wins the stamping.
            let cors_const = cors_services
                .iter()
                .find(|cs| cs.paths.iter().any(|(p, _)| *p == path))
                .map(|cs| cs.const_name.clone());
            // v0.141: the owning service's security constant — the first service
            // that declares this path (paths are expected unique, so this is the
            // owner). Every `from http` service has one.
            let security_const = http_routes
                .iter()
                .find(|r| r.path == path)
                .and_then(|r| security_services.iter().find(|ss| ss.service == r.service))
                .map(|ss| ss.const_name.clone());
            let rejected = http_routes
                .iter()
                .find(|r| r.path == path)
                .and_then(|r| r.rejected.clone());
            PathMethods {
                path,
                has_params,
                methods,
                cors_const,
                security_const,
                rejected,
            }
        })
        .collect()
}

/// Build the per-service CORS policies. A service with a `cors { }` section on a
/// `from http` protocol gets one `CorsService`; allow-methods is derived from its
/// routes (union + `OPTIONS`), and allow-headers defaults to `content-type` (plus
/// `Authorization` when any of its routes carries a Bearer seam) unless the author
/// overrode `headers:`.
fn build_cors_services(
    service_names: &[&String],
    table: &UnitTable,
    http_routes: &[HttpRoute],
) -> Vec<CorsService> {
    let mut out = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        let Some(policy) = &service.cors else {
            continue;
        };
        if !matches!(service.protocol, ServiceProtocol::Http) {
            continue;
        }
        let routes: Vec<&HttpRoute> = http_routes
            .iter()
            .filter(|r| &r.service == *sname)
            .collect();
        if routes.is_empty() {
            continue;
        }

        // Allow-methods: derived from the routes via the one shared rule
        // (v0.139, ADR 0162 D2) — the union of the service's route methods, plus
        // HEAD when GET is present, plus OPTIONS always, alphabetical. So a
        // CORS-enabled service that answers GET advertises HEAD in its
        // `Access-Control-Allow-Methods` too (a cross-origin HEAD is a real
        // request), from the same table that drives the `Allow` header.
        let methods = derive_allowed_methods(routes.iter().map(|r| r.method));

        // Allow-headers: the author's override, else content-type (+ Authorization
        // when the service has a Bearer route — the header the browser must be
        // allowed to send for it).
        let allow_headers = policy.allow_headers().unwrap_or_else(|| {
            let mut hs = vec!["content-type".to_string()];
            if routes.iter().any(|r| r.bearer || r.oidc) {
                hs.push("authorization".to_string());
            }
            hs
        });

        let max_age_expr = match policy.max_age_secs() {
            Some(n) => num_lit(n.to_string()),
            None => null_lit(),
        };
        let literal = TsExpr::object(vec![
            ("origins".to_string(), str_array(&policy.origins())),
            ("allowMethods".to_string(), str_array(&methods)),
            ("allowHeaders".to_string(), str_array(&allow_headers)),
            ("credentials".to_string(), bool_lit(policy.credentials())),
            ("maxAgeSecs".to_string(), max_age_expr),
        ]);

        // Distinct route paths for the preflight match.
        let mut paths: Vec<(String, bool)> = Vec::new();
        for r in &routes {
            let entry = (r.path.clone(), param_count(&r.path) > 0);
            if !paths.contains(&entry) {
                paths.push(entry);
            }
        }

        out.push(CorsService {
            service: (*sname).clone(),
            const_name: format!("__cors_{sname}"),
            literal,
            paths,
        });
    }
    out
}

/// v0.141 (ADR 0164): a `from http` service and its synthesised security-headers
/// policy constant. Unlike `CorsService`, one exists for *every* `from http`
/// service with routes (not only those with a `security { }` block) — the safe
/// header (`nosniff`) is on by default, so a blockless service still stamps the
/// defaults.
struct SecurityService {
    service: String,
    const_name: String,
    literal: TsExpr,
}

/// Build the per-service security policies. Every `from http` service with at
/// least one route gets a `__security_<service>` constant: the author's
/// `security { }` values, or the defaults (`nosniff: true`, no HSTS) when the
/// service declares no block. This is the one place the security lowering
/// diverges from `build_cors_services` (which skips a serviceless-of-a-block
/// service) — a default policy applies to every service (DECISION D).
fn build_security_services(
    service_names: &[&String],
    table: &UnitTable,
    http_routes: &[HttpRoute],
) -> Vec<SecurityService> {
    let mut out = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        if !matches!(service.protocol, ServiceProtocol::Http) {
            continue;
        }
        if !http_routes.iter().any(|r| &r.service == *sname) {
            continue;
        }
        // The author's policy, or the safe defaults when there is no block.
        let (nosniff, hsts) = match &service.security {
            Some(p) => (p.nosniff(), p.hsts_max_age_secs()),
            None => (true, None),
        };
        let hsts_expr = match hsts {
            Some(n) => num_lit(n.to_string()),
            None => null_lit(),
        };
        let literal = TsExpr::object(vec![
            ("nosniff".to_string(), bool_lit(nosniff)),
            ("hstsMaxAgeSecs".to_string(), hsts_expr),
        ]);
        out.push(SecurityService {
            service: (*sname).clone(),
            const_name: format!("__security_{sname}"),
            literal,
        });
    }
    out
}

#[derive(Debug, Clone)]
struct HttpRoute {
    service: String,
    method: bynk_model::IrHttpMethod,
    path: String,
    handler: Handler,
    /// v0.47: the handler's `by` clause names a Bearer actor, so its surface
    /// wrapper runs the verification seam and takes the request as its first
    /// argument.
    bearer: bool,
    /// v0.151: the handler's `by` clause names an `Oidc` actor — its surface
    /// wrapper runs the JWKS verification seam and takes the request first, like
    /// Bearer.
    oidc: bool,
    /// #3386: the handler's `by` clause names a `SignedCookie` actor, whose session
    /// cookie (this name) the wrapper verifies and the handler's `cookies` view omits.
    /// Its wrapper takes the request first, like Bearer.
    signed_cookie: Option<String>,
    /// v0.51: the handler's `by` clause names a Signature actor — the entry
    /// dispatch reads the raw body, verifies the HMAC, and parses the body from
    /// those same bytes.
    signature: Option<bynk_check::actors::SignatureSeam>,
    /// v0.52: the handler's `by` clause names a multi-actor sum — its wrapper
    /// owns the boundary, so the entry passes `request` (+ path params) and does
    /// not read or parse the body itself.
    sum: bool,
    /// v0.140 (ADR 0163): the GET handler's `@cache` freshness policy, if declared.
    /// `Some` only for a `GET` carrying a well-formed `@cache`; drives the
    /// `applyCache` `Cache-Control` stamp. The conditional `ETag`/`304` half is
    /// automatic for every eligible GET and needs no policy.
    cache: Option<bynk_model::CacheIr>,
    /// L7 (#2414, Decision C): the context declares an `EdgeCache` instance,
    /// so its Worker is served through Workers Cache: a `@cache` route stamps
    /// its freshness on a `2xx` alone (`applyEdgeCache`), and a `GET` without
    /// `@cache` says `no-store`.
    edge_cache: bool,
    /// Review of #2417: an opted-in Worker that negotiates its locale from
    /// `Accept-Language` names that header in a `@cache` route's `Vary`, so
    /// the edge keeps one response per language.
    vary_locale: bool,
    /// v0.142 (ADR 0165): the route's effective request-body ceiling in bytes —
    /// the handler's `@limit(maxBody:)` if present, else the service's
    /// `limits { maxBody }`, else `None` (no cap). `Some` emits a `Content-Length`
    /// fast-reject to a synthesised `413` before the body is read; `None` leaves
    /// the route byte-for-byte unchanged (opt-in, DECISION E).
    max_body: Option<i64>,
    /// The media type of a `@raw(contentType:)` handler: its `body: Bytes` is the
    /// request's octets as read, undecoded, and the router answers `415` to any
    /// other `Content-Type` (`"*/*"` accepts all). A raw route always has a cap
    /// (`bynk.http.raw_body_unbounded`), so it always takes the counted read.
    raw: Option<String>,
    /// #2858: the route writes `Option` as plain JSON (`v` / `null`) in its
    /// response body — its `@json(options:)`, else the service's `json { }`.
    json_plain: bool,
    /// #2341 (decision C): the compose-surface method of the service's
    /// `on rejected` (`rejected_<service>`), when it declares one. The route's
    /// synthesised rejections are rendered through it.
    rejected: Option<String>,
    /// #2854: the fields of the handler's typed `query` record, as the runtime's
    /// `queryObject` reads them off the query string: `(name, wire kind, list)`.
    /// `None` when the route has no typed `query` (a raw `RequestQuery` is not
    /// decoded).
    query_fields: Option<Vec<FieldSpec>>,
    /// #3276: the fields of a `@form` handler's `body` record, as the runtime's
    /// `formObject` reads them off an `application/x-www-form-urlencoded` body:
    /// `(name, wire kind, list, option)`, as `query_fields`. `Some` marks the
    /// route's `body` as a form: the router checks the `Content-Type` (`415`) and
    /// decodes the body through the same record codec a JSON body takes.
    form_fields: Option<Vec<FieldSpec>>,
    /// #3277: the fields of the handler's `cookies` record, as the runtime's
    /// `cookieObject` reads them off the `Cookie` header: `(name, wire kind, list,
    /// option)`. `None` when the route takes no `cookies` parameter.
    cookies_fields: Option<Vec<FieldSpec>>,
    /// #3276: a `@multipart` route's body fields (as `form_fields`, with the `File`
    /// fields kind `"file"`) and its per-file and part-count caps.
    multipart: Option<MultipartSpec>,
}

/// One field of a record read off a query string, a form, a `Cookie` header or a
/// multipart body: `(name, leaf kind, list, option, wire name)`. The wire name is
/// the `@name("…")` rename (#3343), `None` when the field reads under its own name.
pub(crate) type FieldSpec = (String, &'static str, bool, bool, Option<String>);

/// The `QueryField` object the runtime's readers take for one field.
fn field_spec_expr((name, kind, list, option, wire): &FieldSpec) -> TsExpr {
    let mut props = vec![("name".to_string(), str_lit(name.clone()))];
    if let Some(wire) = wire {
        props.push(("wire".to_string(), str_lit(wire.clone())));
    }
    props.push(("kind".to_string(), str_lit(*kind)));
    props.push(("list".to_string(), bool_lit(*list)));
    props.push(("option".to_string(), bool_lit(*option)));
    TsExpr::object(props)
}

/// The same object as TypeScript source (`{ name: "sid", wire: "__Host-sid", kind: … }`),
/// for the system-tier drivers and the `Form` lowering, which write text.
pub fn field_spec_source((name, kind, list, option, wire): &FieldSpec) -> String {
    let wire = wire
        .as_ref()
        .map(|w| format!(" wire: {w:?},"))
        .unwrap_or_default();
    format!("{{ name: {name:?},{wire} kind: {kind:?}, list: {list}, option: {option} }}")
}

/// #3343: the `QueryField` array a system-tier driver passes the request writers
/// (`__queryString`, `__cookieString`) for a typed record parameter, when a field of
/// the record is renamed by `@name`; `None` otherwise (the writers need no table).
pub fn renamed_specs_source(
    type_ref: &TypeRef,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<String> {
    use bynk_syntax::ast::TypeBody;
    let TypeRef::Named(id) = type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    let specs = record_field_specs(rec, types, false);
    if specs.iter().all(|s| s.4.is_none()) {
        return None;
    }
    let items: Vec<String> = specs.iter().map(field_spec_source).collect();
    Some(format!("[{}] as const", items.join(", ")))
}

fn field_specs_exprs(fields: &[FieldSpec]) -> Vec<TsExpr> {
    fields.iter().map(field_spec_expr).collect()
}

/// #3276: what the router needs to read a `@multipart` body.
#[derive(Debug, Clone)]
struct MultipartSpec {
    fields: Vec<FieldSpec>,
    /// The per-file byte cap (`@limit(maxFile:)`, else `limits { maxFile }`), if any.
    max_file: Option<i64>,
    /// The part-count cap (`@limit(maxParts:)`, else `limits { maxParts }`, else 64).
    max_parts: i64,
    /// Whether the record has a `File` field, whose real values are laid over
    /// the decoded record.
    has_files: bool,
}

/// #2854: the wire kind of one query-string value of type `r`, as the runtime's
/// `QueryField.kind` names it.
pub(crate) fn query_leaf_kind(
    r: &TypeRef,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    allow_file: bool,
) -> &'static str {
    use bynk_syntax::ast::{BaseType, TypeBody};
    let of_base = |b: &BaseType| match b {
        BaseType::Int => "int",
        BaseType::Bool => "bool",
        // #3276: a `Decimal` is read as text (its own codec parses it), but is not a
        // plain `String`: a form's empty value is absent for it.
        BaseType::Decimal => "decimal",
        BaseType::String
        | BaseType::Float
        | BaseType::Duration
        | BaseType::Instant
        | BaseType::Bytes => "string",
    };
    match r {
        TypeRef::Base(b, _) => of_base(b),
        // #3276: the `File` a `@multipart` body names a part with.
        // The checker admits only the std `File` as a multipart file field
        // (`bynk.http.form_field_not_stringy` refuses a user type of that name), so
        // the name is enough here, on the multipart path alone.
        TypeRef::Named(id) if allow_file && id.name == "File" => "file",
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Refined { base, .. }) | Some(TypeBody::Opaque { base, .. }) => {
                of_base(base)
            }
            Some(TypeBody::Sum(_)) => "enum",
            _ => "string",
        },
        TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::List(..)
        | TypeRef::Map(..)
        | TypeRef::Set(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..)
        | TypeRef::App { .. } => "string",
    }
}

/// #2854: the fields of a handler's typed `query` record, or `None` when the
/// handler has no `query` parameter, or it is the raw `RequestQuery` view (or a
/// path parameter named `query`).
fn query_field_specs(
    h: &Handler,
    path: &str,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<Vec<FieldSpec>> {
    use bynk_syntax::ast::TypeBody;
    if path.split('/').any(|seg| seg == ":query") {
        return None;
    }
    let p = h.params.iter().find(|p| p.name.name == "query")?;
    let TypeRef::Named(id) = &p.type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    Some(record_field_specs(rec, types, false))
}

/// #3276: the fields of a `@form` handler's `body` record, as `formObject` reads
/// them. `None` when the handler is not a `@form` route.
fn form_field_specs(
    h: &Handler,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<Vec<FieldSpec>> {
    use bynk_syntax::ast::TypeBody;
    if !h.annotations.iter().any(|a| a.name.name == "form") {
        return None;
    }
    let p = h.params.iter().find(|p| p.name.name == "body")?;
    let TypeRef::Named(id) = &p.type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    Some(record_field_specs(rec, types, false))
}

/// #3277: bind a handler's `cookies` parameter. `cookies: RequestCookies` (decision A) is the
/// raw view of the request's cookies; a `cookies` record is decoded off the `Cookie` header and
/// validated before the handler runs, answering `400` as a `body` does, with the record's own
/// codec checking refinements and filling defaults. #3386: a route guarded by a `SignedCookie`
/// actor consumes that cookie at the seam, so it is hidden from either form. Returns whether the
/// parameter was a `cookies` one (a `:cookies` path segment is a path parameter instead).
/// #2341: the `RequestHeaders` a handler is passed. #3386: on a `SignedCookie` route the session
/// cookie is hidden from its `Cookie` header, as it is from the `cookies` views.
fn request_headers_arg(route: &HttpRoute) -> TsExpr {
    let mut args = vec![ident("request")];
    args.extend(
        route
            .signed_cookie
            .as_deref()
            .map(|h| str_lit(crate::emitter::escape_ts_string(h))),
    );
    call(ident("__requestHeaders"), args)
}

/// #3513 C: `cf` is what Cloudflare derived about the connection, read off `request.cf`
/// into the `RequestCf` record; a missing or oddly shaped property is `None`.
fn bind_cf_param(
    route: &HttpRoute,
    p: &bynk_syntax::ast::Param,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    (cors_const, security_const, rejected): (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) -> bool {
    if route.path.split('/').any(|seg| seg == ":cf")
        || !matches!(&p.type_ref, TypeRef::Named(id) if id.name == "RequestCf")
    {
        return false;
    }
    let dser = deserialise_call(&p.type_ref, "__cfObject(request)", "cf", ru);
    guarded.push(const_("__r_cf", dser));
    // `__cfObject` keeps only fields the record accepts, so this does not reject; the arm
    // is what narrows the `Result` for the compiler.
    guarded.push(if_(
        strict_eq(member(ident("__r_cf"), "tag"), str_lit("Err")),
        return_(Some(stamp_rejection(
            json_response(member(ident("__r_cf"), "error"), 400),
            cors_const,
            security_const,
            rejected,
        ))),
    ));
    guarded.push(const_("cf", member(ident("__r_cf"), "value")));
    call_args.push(ident("cf"));
    true
}

/// The request-reading parameters that bind a record: `cf`, and `cookies`.
fn bind_request_record_param(
    route: &HttpRoute,
    p: &bynk_syntax::ast::Param,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    wrappers: (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) -> bool {
    if p.name.name == "cf" {
        return bind_cf_param(route, p, guarded, call_args, wrappers, ru);
    }
    bind_cookies_param(route, p, guarded, call_args, wrappers, ru)
}

fn bind_cookies_param(
    route: &HttpRoute,
    p: &bynk_syntax::ast::Param,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    wrappers: (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) -> bool {
    if p.name.name != "cookies" || route.path.split('/').any(|seg| seg == ":cookies") {
        return false;
    }
    let hidden = route.signed_cookie.as_deref();
    if matches!(p.type_ref, TypeRef::RequestCookies(_)) {
        let mut args = vec![ident("request")];
        args.extend(hidden.map(|h| str_lit(crate::emitter::escape_ts_string(h))));
        call_args.push(call(ident("__requestCookies"), args));
        return true;
    }
    let Some(fields) = &route.cookies_fields else {
        return false;
    };
    push_cookies_param(
        fields,
        &p.type_ref,
        hidden,
        guarded,
        call_args,
        wrappers,
        ru,
    );
    true
}

/// #3277: the statements that decode a route's `cookies` record off the `Cookie`
/// header and bind it for the handler call: `400` on a rejection, as a `body`
/// answers, with the record's own codec checking refinements and filling defaults.
fn push_cookies_param(
    fields: &[FieldSpec],
    type_ref: &TypeRef,
    hide: Option<&str>,
    guarded: &mut Vec<TsStmt>,
    call_args: &mut Vec<TsExpr>,
    (cors_const, security_const, rejected): (Option<&str>, Option<&str>, Option<&str>),
    ru: &RuntimeUse,
) {
    let spec: Vec<TsExpr> = field_specs_exprs(fields);
    let mut cookie_args = vec![ident("request"), TsExpr::array(spec)];
    // #3386: a `SignedCookie` actor's credential is not the handler's to read.
    if let Some(hidden) = hide {
        cookie_args.push(str_lit(crate::emitter::escape_ts_string(hidden)));
    }
    guarded.push(const_("__c", call(ident("__cookieObject"), cookie_args)));
    guarded.push(if_(
        TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Not,
            expr: Box::new(member(ident("__c"), "ok")),
        },
        return_(Some(stamp_rejection(
            json_response(member(ident("__c"), "error"), 400),
            cors_const,
            security_const,
            rejected,
        ))),
    ));
    let dser = deserialise_call(type_ref, "__c.value", "cookies", ru);
    guarded.push(const_("__r_cookies", dser));
    guarded.push(if_(
        strict_eq(member(ident("__r_cookies"), "tag"), str_lit("Err")),
        return_(Some(stamp_rejection(
            json_response(member(ident("__r_cookies"), "error"), 400),
            cors_const,
            security_const,
            rejected,
        ))),
    ));
    guarded.push(const_("cookies", member(ident("__r_cookies"), "value")));
    call_args.push(ident("cookies"));
}

/// #3277: the fields of a handler's `cookies` record, or `None` when it has no
/// `cookies` parameter (or a `:cookies` path segment binds the name).
fn cookies_field_specs(
    h: &Handler,
    path: &str,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<Vec<FieldSpec>> {
    use bynk_syntax::ast::TypeBody;
    if path.split('/').any(|seg| seg == ":cookies") {
        return None;
    }
    let p = h.params.iter().find(|p| p.name.name == "cookies")?;
    let TypeRef::Named(id) = &p.type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    Some(record_field_specs(rec, types, false))
}

/// #3276: the body field spec of a `@form` or `@multipart` handler, as the system
/// test drivers write the body (`(name, wire kind, list, option)`).
pub fn form_body_specs(
    h: &Handler,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<Vec<FieldSpec>> {
    form_field_specs(h, types).or_else(|| multipart_spec(None, h, types).map(|m| m.fields))
}

/// #3276: a `@multipart` handler's body spec, or `None` when it is not one.
fn multipart_spec(
    service_limits: Option<&LimitsPolicy>,
    h: &Handler,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Option<MultipartSpec> {
    use bynk_syntax::ast::TypeBody;
    if !h.annotations.iter().any(|a| a.name.name == "multipart") {
        return None;
    }
    let p = h.params.iter().find(|p| p.name.name == "body")?;
    let TypeRef::Named(id) = &p.type_ref else {
        return None;
    };
    let Some(TypeBody::Record(rec)) = types.get(&id.name).map(|t| &t.body) else {
        return None;
    };
    let fields = record_field_specs(rec, types, true);
    let has_files = fields.iter().any(|(_, kind, _, _, _)| *kind == "file");
    Some(MultipartSpec {
        fields,
        max_file: h
            .limit_max_file()
            .or_else(|| service_limits.and_then(|p| p.max_file())),
        max_parts: h
            .limit_max_parts()
            .or_else(|| service_limits.and_then(|p| p.max_parts()))
            .unwrap_or(bynk_syntax::ast::DEFAULT_MAX_PARTS),
        has_files,
    })
}

/// The wire shape of each field of a record read off a query string or a form:
/// `(name, leaf kind, list, option)`.
pub(crate) fn record_field_specs(
    rec: &bynk_syntax::ast::RecordBody,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    allow_file: bool,
) -> Vec<FieldSpec> {
    rec.fields
        .iter()
        .map(|f| {
            let (inner, list, option) = serialisation::peel_list_option(&f.type_ref);
            (
                f.name.name.clone(),
                query_leaf_kind(inner, types, allow_file),
                list,
                option,
                f.wire_name(),
            )
        })
        .collect()
}

/// #2858: whether any `from http` route in `table` writes plain JSON, so the
/// generated `Option` encoders carry the plain branch.
pub fn table_has_plain_json(table: &UnitTable) -> bool {
    table.services.values().any(|svc| {
        svc.handlers.iter().any(|h| {
            bynk_syntax::ast::effective_json_options(svc.json.as_ref(), h)
                == bynk_syntax::ast::JsonOptions::Plain
        })
    })
}

/// v0.142 (ADR 0165): resolve a route's effective request-body ceiling in bytes.
/// A route's `@limit(maxBody:)` annotation wins over the service's `limits { }`
/// default (DECISION B); with neither, the route has no cap (`None`). Project
/// validation (`bynk.http.limit_*` / `limits_*`) has already rejected a malformed
/// or misplaced `@limit`/`limits`, so a bad/absent value here simply yields no
/// cap. Only a body-taking method can carry a cap — a `@limit` on a GET/DELETE is
/// a checker error, so it never reaches a live route here. The route-annotation
/// half is `bynk-emit::ir`'s own [`bynk_model::lower_route_limit_ir`]
/// (#1228); only the service-wide fallback composition stays here.
fn effective_max_body(service_limits: Option<&LimitsPolicy>, h: &Handler) -> Option<i64> {
    h.limit_max_body()
        .or_else(|| service_limits.and_then(|p| p.max_body()))
}

/// One `on cron` handler, identified by its service and per-service declaration
/// index (which together form its `cron_<service>_<index>` method key).
#[derive(Debug, Clone)]
struct CronRoute {
    service: String,
    index: usize,
    expr: String,
    /// Whether the handler declares the optional scheduled-time parameter.
    has_param: bool,
}

/// One `on queue` consumer, identified by its service and per-service
/// declaration index (its `queue_<service>_<index>` method key), plus the
/// message parameter's type for wire-format deserialisation.
#[derive(Debug, Clone)]
struct QueueRoute {
    service: String,
    index: usize,
    name: String,
    msg_type: Option<TypeRef>,
}

/// Stamp a rejection response with the service's policy the same way a
/// handled response is stamped — `applySecurityHeaders(applyCors(…))` — so a
/// boundary `400`/`401` carries `nosniff` (and CORS, when the service opts in)
/// exactly as the `200`, the `405`, and the `413` do. `inner` is the finished
/// `new Response(…)` TS expression; the wrapping mirrors the happy path
/// (`emit_http_route_dispatch`) and the `413` ceiling above it. ADR 0164 D6:
/// *every response a `from http` route emits carries the policy* — before this,
/// the bare-`new Response` rejection sites skipped the wrapper (#659), leaving
/// `nosniff` off precisely the responses that reflect attacker input.
fn stamp_rejection(
    inner: TsExpr,
    cors_const: Option<&str>,
    security_const: Option<&str>,
    // #2341 (decision C): the service's `on rejected` surface method, if it
    // declares one — the default response is rendered through it first, and
    // the headers are stamped on what it returns.
    rejected: Option<&str>,
) -> TsExpr {
    let inner = match rejected {
        Some(m) => await_expr(call(
            ident("__renderRejection"),
            vec![inner, ident(format!("(why) => surface.{m}(why)"))],
        )),
        None => inner,
    };
    let corsed = match cors_const {
        Some(c) => call(
            ident("__applyCors"),
            vec![
                inner,
                ident(c.to_string()),
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("origin")],
                ),
            ],
        ),
        None => inner,
    };
    match security_const {
        Some(s) => call(
            ident("__applySecurityHeaders"),
            vec![corsed, ident(s.to_string())],
        ),
        None => corsed,
    }
}

/// Generate the per-route dispatch block for one `on http` handler. The
/// router has already been entered via `try`; this block extracts path
/// parameters, deserialises the body (when present), invokes the handler,
/// and serialises the HttpResult through `httpResultToResponse`. Returns
/// the whole bare `{ ... }` block statement — `emit_worker_entry`'s own
/// caller pushes it directly into the `try` block's own statement list.
fn emit_http_route_dispatch(
    route: &HttpRoute,
    cors_const: Option<&str>,
    security_const: Option<&str>,
    ru: &RuntimeUse,
    consumed_types: &std::collections::HashSet<String>,
) -> TsStmt {
    let h = &route.handler;
    let rejected = route.rejected.as_deref();
    let method_key = http_handler_method_name_ir(route.method, &route.path);
    let has_path_params = param_count(&route.path) > 0;
    // v0.139 (ADR 0162 D3): a `GET` route also answers `HEAD` — the guard widens
    // so the `GET` handler runs, then the built response's body is stripped
    // below. Other methods match exactly.
    let method_guard = if route.method == bynk_model::IrHttpMethod::Get {
        or_expr(
            strict_eq(ident("method"), str_lit("GET")),
            strict_eq(ident("method"), str_lit("HEAD")),
        )
    } else {
        strict_eq(ident("method"), str_lit(route.method.as_str()))
    };

    let mut inner: Vec<TsStmt> = Vec::new();
    if has_path_params {
        inner.push(const_(
            "__m",
            call(
                ident("__matchPath"),
                vec![str_lit(route.path.clone()), ident("path")],
            ),
        ));
    }
    let route_cond = if has_path_params {
        and_expr(method_guard, ident("__m"))
    } else {
        and_expr(
            method_guard,
            strict_eq(ident("path"), str_lit(route.path.clone())),
        )
    };

    // #1825: names the route, by its pattern, for the fault log.
    let mut guarded: Vec<TsStmt> = vec![set_fault_route(format!(
        "{} {}",
        route.method.as_str(),
        route.path
    ))];

    // v0.142 (ADR 0165): the request-body ceiling. A route with an effective cap
    // rejects an oversized body with a synthesised `413` derived from the declared
    // `Content-Length`, *before* the body is read — ahead of the `by`/Bearer/sum
    // wrapper and the Signature seam's raw read, so an oversized (possibly
    // unauthenticated) body is never buffered (DECISION D/E). The `413` is
    // `applyCors`/`applySecurityHeaders`-stamped so a cross-origin caller can read
    // it, the same visibility rule the `405` follows. Emitted only for a
    // body-taking route with a cap; a capless route is byte-for-byte unchanged
    // (opt-in). This placement also covers a multi-actor sum route, whose wrapper
    // reads the body itself.
    let has_body = h.params.iter().any(|p| p.name.name == "body");
    // The synthesised `413`, shared by the `Content-Length` fast-reject and the
    // counted read below.
    let too_large = |cap: i64| {
        stamp_rejection(
            json_response(
                json_error_kind(
                    "PayloadTooLarge",
                    vec![(
                        "details".to_string(),
                        str_lit(format!("request body exceeds {cap} bytes")),
                    )],
                ),
                413,
            ),
            cors_const,
            security_const,
            rejected,
        )
    };
    // A `@raw` route answers `415` to a `Content-Type` other than the declared
    // media type — a router decision like the `413`, ahead of the auth seam and
    // the body read, so it costs nothing and reveals nothing.
    if let Some(ct) = route.raw.as_deref().filter(|ct| has_body && *ct != "*/*") {
        let unsupported = stamp_rejection(
            json_response(
                json_error_kind(
                    "UnsupportedMediaType",
                    vec![(
                        "details".to_string(),
                        str_lit(format!("expected Content-Type {ct}")),
                    )],
                ),
                415,
            ),
            cors_const,
            security_const,
            rejected,
        );
        guarded.push(if_(
            TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                expr: Box::new(call(
                    ident("__mediaTypeMatches"),
                    vec![ident("request"), str_lit(ct.to_string())],
                )),
            },
            block(vec![return_(Some(unsupported))]),
        ));
    }
    // #3276: a `@form` route answers `415` to any `Content-Type` but a UTF-8
    // `application/x-www-form-urlencoded`, at the same point as a `@raw` route.
    if has_body && route.form_fields.is_some() {
        let unsupported = stamp_rejection(
            json_response(
                json_error_kind(
                    "UnsupportedMediaType",
                    vec![(
                        "details".to_string(),
                        str_lit("expected Content-Type application/x-www-form-urlencoded (UTF-8)"),
                    )],
                ),
                415,
            ),
            cors_const,
            security_const,
            rejected,
        );
        guarded.push(if_(
            TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                expr: Box::new(call(
                    ident("__formMediaTypeMatches"),
                    vec![ident("request")],
                )),
            },
            block(vec![return_(Some(unsupported))]),
        ));
    }
    // #3276: a `@multipart` route answers `415` to any `Content-Type` but
    // `multipart/form-data`; a body with no boundary is a `400 MalformedForm`, below.
    if has_body && route.multipart.is_some() {
        let unsupported = stamp_rejection(
            json_response(
                json_error_kind(
                    "UnsupportedMediaType",
                    vec![(
                        "details".to_string(),
                        str_lit("expected Content-Type multipart/form-data"),
                    )],
                ),
                415,
            ),
            cors_const,
            security_const,
            rejected,
        );
        guarded.push(if_(
            TsExpr::Unary {
                op: bynk_ts::TsUnaryOp::Not,
                expr: Box::new(call(
                    ident("__multipartMediaTypeMatches"),
                    vec![ident("request")],
                )),
            },
            block(vec![return_(Some(unsupported))]),
        ));
    }
    if let Some(cap) = route.max_body.filter(|_| has_body) {
        guarded.push(const_(
            "__contentLength",
            method_call(
                member(ident("request"), "headers"),
                "get",
                vec![str_lit("content-length")],
            ),
        ));
        guarded.push(if_(
            and_expr(
                strict_neq(ident("__contentLength"), null_lit()),
                binary(
                    bynk_ts::TsBinaryOp::GreaterThan,
                    call(ident("globalThis.Number"), vec![ident("__contentLength")]),
                    num_lit(cap.to_string()),
                ),
            ),
            block(vec![return_(Some(too_large(cap)))]),
        ));
    }

    // Extract path parameters from the matched params map.
    let mut call_args: Vec<TsExpr> = Vec::new();
    for p in &h.params {
        let pname = &p.name.name;
        if pname == "body" {
            // Body deserialisation happens below; the body takes its place in
            // the argument list here, in declaration order. A multi-actor sum
            // route's wrapper reads the body itself, so it isn't passed.
            if !route.sum {
                call_args.push(ident("body"));
            }
            continue;
        }
        // #2341 (decision A): `headers` is the request's headers, read-only.
        if pname == "headers" && matches!(p.type_ref, TypeRef::RequestHeaders(_)) {
            call_args.push(request_headers_arg(route));
            continue;
        }
        // #2854 (decision B): `query: RequestQuery` is the raw query string.
        let query_is_path = route.path.split('/').any(|seg| seg == ":query");
        if pname == "query" && !query_is_path && matches!(p.type_ref, TypeRef::RequestQuery(_)) {
            call_args.push(call(ident("__requestQuery"), vec![ident("request")]));
            continue;
        }
        // #2854 (decision A): a typed `query` is decoded off the query string
        // and validated before the handler runs, answering `400` as a `body`
        // does. The runtime reads the fields into a JSON object, which the
        // record's own codec then checks (refinements, defaults, `Option`s).
        if pname == "query"
            && !query_is_path
            && let Some(fields) = &route.query_fields
        {
            let spec: Vec<TsExpr> = field_specs_exprs(fields);
            guarded.push(const_(
                "__q",
                call(
                    ident("__queryObject"),
                    vec![ident("request"), TsExpr::array(spec)],
                ),
            ));
            guarded.push(if_(
                TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Not,
                    expr: Box::new(member(ident("__q"), "ok")),
                },
                return_(Some(stamp_rejection(
                    json_response(member(ident("__q"), "error"), 400),
                    cors_const,
                    security_const,
                    rejected,
                ))),
            ));
            let dser = deserialise_call(&p.type_ref, "__q.value", "query", ru);
            guarded.push(const_("__r_query", dser));
            guarded.push(if_(
                strict_eq(member(ident("__r_query"), "tag"), str_lit("Err")),
                return_(Some(stamp_rejection(
                    json_response(member(ident("__r_query"), "error"), 400),
                    cors_const,
                    security_const,
                    rejected,
                ))),
            ));
            guarded.push(const_("query", member(ident("__r_query"), "value")));
            call_args.push(ident("query"));
            continue;
        }
        if bind_request_record_param(
            route,
            p,
            &mut guarded,
            &mut call_args,
            (cors_const, security_const, rejected),
            ru,
        ) {
            continue;
        }
        // It's a path parameter — extract from __m.params and construct. The
        // `__m.params` key is the wire segment name (verbatim); the constructed
        // binding must dodge JS reserved words for the surface call.
        let jname = wire_ident(pname);
        guarded.push(const_(
            format!("__raw_{pname}"),
            index_expr(member(ident("__m"), "params"), str_lit(pname.clone())),
        ));
        guarded.extend(emit_path_param_construction(
            pname,
            &p.type_ref,
            cors_const,
            security_const,
            rejected,
            consumed_types,
        ));
        call_args.push(ident(jname));
    }

    // Body parameter (POST/PUT/PATCH). A multi-actor sum route's wrapper reads
    // and parses the body itself (it must verify over the raw bytes first), so
    // the entry skips the body here and passes only `request`.
    if !route.sum
        && let Some(body_param) = h.params.iter().find(|p| p.name.name == "body")
    {
        // v0.188 (#659): the body-read rejections are stamped with the service
        // policy just like the handled `200`. bynk_ts::TsExpr here, at their use site, to
        // match the local style (the `413`, `body_reject`, and path-param
        // rejections are all built lazily too) — a bodyless `GET` never allocates
        // them. `malformed_400` is reached from both the signature and plain body
        // paths; `unauthorized_401` only from the signature seam, so it is built
        // there; the input-reflecting body-deser `400` stamps inline below.
        let malformed_400 = || {
            stamp_rejection(
                json_response(
                    json_error_kind(
                        "MalformedJson",
                        vec![("details".to_string(), str_lit("Invalid request body"))],
                    ),
                    400,
                ),
                cors_const,
                security_const,
                rejected,
            )
        };
        // A capped body is read through the counting reader, so a chunked body, or
        // one whose `Content-Length` understates it, is cut off at the cap with the
        // same `413` as the fast-reject above. A raw route is always capped.
        let streamed = streamed_body(route); // pulled by the handler, so not read here
        let counted = route.max_body.filter(|_| !streamed);
        let raw = route.raw.is_some();
        let form = route.form_fields.as_ref();
        let multipart = route.multipart.as_ref();
        let decoded = || {
            method_call(
                new_expr("globalThis.TextDecoder", vec![]),
                "decode",
                vec![ident("__bytes")],
            )
        };
        if let Some(cap) = counted {
            guarded.push(let_typed(
                "__bytes",
                TsType::named("globalThis.Uint8Array | undefined"),
            ));
            guarded.push(try_catch_stmt(
                vec![TsStmt::assign(
                    ident("__bytes"),
                    await_expr(call(
                        ident("__readBodyCapped"),
                        vec![ident("request"), num_lit(cap.to_string())],
                    )),
                    None,
                )],
                None,
                vec![return_(Some(malformed_400()))],
            ));
            guarded.push(if_(
                strict_eq(ident("__bytes"), ident("undefined")),
                block(vec![return_(Some(too_large(cap)))]),
            ));
        }
        if !raw && form.is_none() && multipart.is_none() {
            guarded.push(let_typed("__body_json", TsType::named("__JsonValue")));
        }
        if let Some(seam) = &route.signature {
            // v0.51: read the raw body once, verify the HMAC fail-closed (401),
            // then parse the body param from the *same* bytes (not a re-read /
            // re-serialisation — the signature is over these exact bytes).
            let unauthorized_401 =
                || stamp_rejection(null_response(401), cors_const, security_const, rejected);
            // What the HMAC covers: a raw route's octets as received; otherwise
            // the body's text.
            let signed = if raw || multipart.is_some() {
                ident("__bytes")
            } else if counted.is_some() {
                guarded.push(const_("__raw", decoded()));
                ident("__raw")
            } else {
                guarded.push(let_typed("__raw", TsType::named("string")));
                guarded.push(try_catch_stmt(
                    vec![TsStmt::assign(
                        ident("__raw"),
                        await_expr(method_call(ident("request"), "text", vec![])),
                        None,
                    )],
                    None,
                    vec![return_(Some(malformed_400()))],
                ));
                ident("__raw")
            };
            guarded.push(const_("__secret", {
                let explicit = index_expr(
                    as_expr(
                        as_expr(ident("env"), TsType::named("unknown")),
                        TsType::named_with_args(
                            "globalThis.Record",
                            vec![TsType::named("string"), TsType::named("unknown")],
                        ),
                    ),
                    str_lit(seam.secret.clone()),
                );
                let global_probe = TsExpr::OptionalIndex {
                    object: Box::new(TsExpr::OptionalMember {
                        object: Box::new(member(
                            as_expr(
                                ident("globalThis"),
                                TsType::Object(vec![TsTypeMember::optional_prop(
                                    "process",
                                    TsType::Object(vec![TsTypeMember::optional_prop(
                                        "env",
                                        TsType::named_with_args(
                                            "globalThis.Record",
                                            vec![TsType::named("string"), TsType::named("unknown")],
                                        ),
                                    )]),
                                )]),
                            ),
                            "process",
                        )),
                        property: "env".to_string(),
                    }),
                    index: Box::new(str_lit(seam.secret.clone())),
                };
                binary(
                    bynk_ts::TsBinaryOp::NullishCoalescing,
                    explicit,
                    global_probe,
                )
            }));
            guarded.push(if_(
                strict_neq(
                    TsExpr::Unary {
                        op: bynk_ts::TsUnaryOp::Typeof,
                        expr: Box::new(ident("__secret")),
                    },
                    str_lit("string"),
                ),
                return_(Some(unauthorized_401())),
            ));
            // Timestamp (when bound): must be present; passed to the verifier.
            let ts_expr: TsExpr = match &seam.timestamp_header {
                Some(th) => {
                    guarded.push(const_(
                        "__ts",
                        method_call(
                            member(ident("request"), "headers"),
                            "get",
                            vec![str_lit(th.clone())],
                        ),
                    ));
                    guarded.push(if_(
                        strict_eq(ident("__ts"), null_lit()),
                        return_(Some(unauthorized_401())),
                    ));
                    ident("__ts")
                }
                None => null_lit(),
            };
            let tol_expr = match seam.tolerance_secs {
                Some(n) => num_lit(n.to_string()),
                None => null_lit(),
            };
            guarded.push(const_(
                "__ok",
                await_expr(call(
                    ident("__verifySignatureHmacSha256"),
                    vec![
                        signed.clone(),
                        ident("__secret"),
                        method_call(
                            member(ident("request"), "headers"),
                            "get",
                            vec![str_lit(seam.header.clone())],
                        ),
                        ts_expr,
                        tol_expr,
                    ],
                )),
            ));
            guarded.push(if_(
                TsExpr::Unary {
                    op: bynk_ts::TsUnaryOp::Not,
                    expr: Box::new(ident("__ok")),
                },
                return_(Some(unauthorized_401())),
            ));
            if !raw && form.is_none() && multipart.is_none() {
                guarded.push(try_catch_stmt(
                    vec![TsStmt::assign(
                        ident("__body_json"),
                        as_expr(
                            method_call(ident("globalThis.JSON"), "parse", vec![ident("__raw")]),
                            TsType::named("__JsonValue"),
                        ),
                        None,
                    )],
                    None,
                    vec![return_(Some(malformed_400()))],
                ));
            }
        } else if multipart.is_some() {
            // The counted bytes in `__bytes` are parsed below.
        } else if form.is_some() {
            // The body's text, read here; the form reader below decodes it.
            if counted.is_some() {
                guarded.push(const_("__raw", decoded()));
            } else {
                guarded.push(let_typed("__raw", TsType::named("string")));
                guarded.push(try_catch_stmt(
                    vec![TsStmt::assign(
                        ident("__raw"),
                        await_expr(method_call(ident("request"), "text", vec![])),
                        None,
                    )],
                    None,
                    vec![return_(Some(malformed_400()))],
                ));
            }
        } else if !raw {
            let parsed = if counted.is_some() {
                method_call(ident("globalThis.JSON"), "parse", vec![decoded()])
            } else {
                // This site's own real historical text keeps the
                // parens (`(await request.json()) as JsonValue`),
                // unlike the `/_bynk/call/` dispatch's bare `args`
                // above — see that site's own comment.
                paren(await_expr(method_call(ident("request"), "json", vec![])))
            };
            guarded.push(try_catch_stmt(
                vec![TsStmt::assign(
                    ident("__body_json"),
                    as_expr(parsed, TsType::named("__JsonValue")),
                    None,
                )],
                None,
                vec![return_(Some(malformed_400()))],
            ));
        }
        if streamed {
            guarded.push(streamed_body_binding(route));
        } else if raw {
            // The octets, untouched: no JSON, no base64.
            guarded.push(const_("body", ident("__bytes")));
        } else {
            // #3276: a form body is read into the JSON object the record's codec
            // takes, naming `body.<field>` in its rejections.
            let (source, root) = if let Some(mp) = multipart {
                let spec: Vec<TsExpr> = field_specs_exprs(&mp.fields);
                let limits = TsExpr::object(vec![
                    (
                        "maxFile".to_string(),
                        match mp.max_file {
                            Some(n) => num_lit(n.to_string()),
                            None => null_lit(),
                        },
                    ),
                    ("maxParts".to_string(), num_lit(mp.max_parts.to_string())),
                ]);
                guarded.push(const_(
                    "__f",
                    await_expr(call(
                        ident("__multipartObject"),
                        vec![
                            ident("__bytes"),
                            binary(
                                bynk_ts::TsBinaryOp::NullishCoalescing,
                                method_call(
                                    member(ident("request"), "headers"),
                                    "get",
                                    vec![str_lit("content-type")],
                                ),
                                str_lit(""),
                            ),
                            TsExpr::array(spec),
                            limits,
                        ],
                    )),
                ));
                // A `413` (too many parts, a file over its cap) or a `400` (not a
                // multipart body, or a part that is not its field's shape).
                let reject = |status: i64| {
                    stamp_rejection(
                        json_response(member(ident("__f"), "error"), status),
                        cors_const,
                        security_const,
                        rejected,
                    )
                };
                guarded.push(if_(
                    TsExpr::Unary {
                        op: bynk_ts::TsUnaryOp::Not,
                        expr: Box::new(member(ident("__f"), "ok")),
                    },
                    block(vec![return_(Some(cond_expr(
                        strict_eq(member(ident("__f"), "status"), num_lit("413")),
                        reject(413),
                        reject(400),
                    )))]),
                ));
                ("__f.value", "body")
            } else if let Some(fields) = form {
                let spec: Vec<TsExpr> = field_specs_exprs(fields);
                guarded.push(const_(
                    "__f",
                    call(
                        ident("__formObject"),
                        vec![ident("__raw"), TsExpr::array(spec)],
                    ),
                ));
                guarded.push(if_(
                    TsExpr::Unary {
                        op: bynk_ts::TsUnaryOp::Not,
                        expr: Box::new(member(ident("__f"), "ok")),
                    },
                    return_(Some(stamp_rejection(
                        json_response(member(ident("__f"), "error"), 400),
                        cors_const,
                        security_const,
                        rejected,
                    ))),
                ));
                ("__f.value", "body")
            } else {
                ("__body_json", "$")
            };
            let dser = deserialise_call(&body_param.type_ref, source, root, ru);
            guarded.push(const_("__r_body", dser));
            // The body-deser `400` reflects the offending input into its JSON body, so
            // stamping it (#659) matters most here — `nosniff` on a reflected value.
            let body_reject = stamp_rejection(
                json_response(member(ident("__r_body"), "error"), 400),
                cors_const,
                security_const,
                rejected,
            );
            guarded.push(if_(
                strict_eq(member(ident("__r_body"), "tag"), str_lit("Err")),
                return_(Some(body_reject)),
            ));
            if multipart.is_some_and(|mp| mp.has_files) {
                // The `File` values, laid over the decoded record's placeholders.
                guarded.push(const_(
                    "body",
                    call(
                        ident("globalThis.Object.assign"),
                        vec![
                            TsExpr::object(vec![]),
                            member(ident("__r_body"), "value"),
                            member(ident("__f"), "files"),
                        ],
                    ),
                ));
            } else {
                guarded.push(const_("body", member(ident("__r_body"), "value")));
            }
        }
    }

    // Invoke the handler and serialise the HttpResult. The handler is
    // wrapped on the surface so its deps are wired by `compose`. v0.47: a
    // Bearer wrapper takes the request first (it runs the verification seam).
    let surface_args: Vec<TsExpr> =
        if route.bearer || route.sum || route.oidc || route.signed_cookie.is_some() {
            // The Bearer, Oidc, and sum wrappers take the request first (they run the
            // verification seam); a sum wrapper also reads/parses the body itself,
            // so `call_args` here carries only the path params.
            let mut a = vec![ident("request")];
            a.extend(call_args.iter().cloned());
            a
        } else {
            call_args
        };
    push_handler_invocation(route, method_key, surface_args, too_large, &mut guarded);
    let inner_ty = http_result_inner(&h.return_type);
    let ser_fn = crate::emitter::serialisation::serialise_ref_via(&inner_ty, "handlers.", ru);
    // #2858: a plain-JSON route's body serialiser runs with the `Option` encoders
    // switched to `v` / `null`; every other route passes `ser_fn` untouched.
    let ser_fn = if route.json_plain {
        call(ident("__withPlainJson"), vec![ser_fn])
    } else {
        ser_fn
    };
    // v0.140 (ADR 0163): a `GET` response carries a weak `ETag` (an `Ok` body gets
    // the validator via `weakEtag`), an optional `@cache` `Cache-Control`
    // (`applyCache`), and is answered `304` when the request revalidates
    // (`notModifiedIfMatch`) — composed innermost-first so CORS still stamps the
    // `304`. Non-`GET`/unsafe methods are byte-for-byte unchanged: no validator, no
    // conditional, no freshness (DECISION B).
    let response_expr = if route.method == bynk_model::IrHttpMethod::Get {
        let base = call(
            ident("__httpResultToResponse"),
            vec![
                ident("result"),
                ser_fn,
                TsExpr::object(vec![("weakEtag".to_string(), bool_lit(true))]),
            ],
        );
        // `immutable` is passed only when `true`, so every other `@cache` route
        // emits exactly the three-argument call it did before.
        let cached = match &route.cache {
            Some(p) => {
                let mut args = vec![base, num_lit(p.max_age_secs.to_string()), str_lit(p.scope)];
                if p.immutable {
                    args.push(bool_lit(true));
                }
                let apply = if route.edge_cache {
                    "__applyEdgeCache"
                } else {
                    "__applyCache"
                };
                if route.vary_locale {
                    args[0] = call(ident("__varyAcceptLanguage"), vec![args[0].clone()]);
                }
                call(ident(apply), args)
            }
            None if route.edge_cache => call(ident("__noStore"), vec![base]),
            None => base,
        };
        call(
            ident("__notModifiedIfMatch"),
            vec![cached, ident("request")],
        )
    } else {
        call(
            ident("__httpResultToResponse"),
            vec![ident("result"), ser_fn],
        )
    };
    // v0.131: a CORS-enabled service stamps the `Access-Control-*` headers onto
    // every real response, uniformly across variant families — including the
    // synthesised `304`, so a cross-origin revalidation stays readable.
    let cors_expr = match cors_const {
        Some(c) => call(
            ident("__applyCors"),
            vec![
                response_expr,
                ident(c.to_string()),
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit("origin")],
                ),
            ],
        ),
        None => response_expr,
    };
    // v0.141 (ADR 0164): stamp the security headers (`nosniff` by default, HSTS
    // opt-in) onto every response — outside `applyCors`, but the two set disjoint
    // headers so order is immaterial. Every `from http` service has a policy, so
    // this is `Some` on every route; it also flows onto the `304` (inside
    // `response_expr`) and the `HEAD` answer (`headResponse` copies the headers).
    let build_expr = match security_const {
        Some(s) => call(
            ident("__applySecurityHeaders"),
            vec![cors_expr, ident(s.to_string())],
        ),
        None => cors_expr,
    };
    // v0.139 (ADR 0162 D3): on a `GET` route, a `HEAD` returns the same status
    // and headers with an empty body — the handler ran, so the headers are the
    // real ones a `GET` would produce; `headResponse` discards the body without
    // reading it (a `Streaming` body is never drained).
    if route.method == bynk_model::IrHttpMethod::Get {
        guarded.push(const_("__response", build_expr));
        guarded.push(return_(Some(cond_expr(
            strict_eq(ident("method"), str_lit("HEAD")),
            call(ident("__headResponse"), vec![ident("__response")]),
            ident("__response"),
        ))));
    } else {
        guarded.push(return_(Some(build_expr)));
    }

    inner.push(if_(route_cond, block(guarded)));
    block(inner)
}

/// #3380: `const body = requestBodyStream(request, cap)`: the octets as they
/// arrive, counted against the cap as the handler pulls them. The checker
/// requires a cap on every `@raw` route.
fn streamed_body_binding(route: &HttpRoute) -> TsStmt {
    let cap = route
        .max_body
        .expect("a streamed `@raw` route has a cap (bynk.http.raw_body_unbounded)");
    const_(
        "body",
        call(
            ident("__requestBodyStream"),
            vec![ident("request"), num_lit(cap.to_string())],
        ),
    )
}

/// The statements that invoke a route's handler and bind its `result`. A streamed
/// body that crosses its cap while the handler reads it faults that read with
/// `BodyTooLarge` (#3380); no response has been built yet, so the router answers the
/// route's `413`, stamped as the `Content-Length` fast-reject is, and not a `500`.
fn push_handler_invocation(
    route: &HttpRoute,
    method_key: String,
    surface_args: Vec<TsExpr>,
    too_large: impl Fn(i64) -> TsExpr,
    guarded: &mut Vec<TsStmt>,
) {
    let invoke = call(member(ident("surface"), method_key), surface_args);
    let Some(cap) = route.max_body.filter(|_| streamed_body(route)) else {
        guarded.push(const_("result", await_expr(invoke)));
        return;
    };
    guarded.push(const_(
        "__handled",
        await_expr(call(ident("__untilBodyTooLarge"), vec![invoke])),
    ));
    guarded.push(if_(
        member(ident("__handled"), "tooLarge"),
        block(vec![return_(Some(too_large(cap)))]),
    ));
    guarded.push(const_("result", member(ident("__handled"), "value")));
}

/// #3380: whether `route`'s `body` is a streamed one: a `@raw` route whose `body`
/// is a `Stream[Bytes]`, handed to the handler as the request body arrives rather
/// than read whole first.
fn streamed_body(route: &HttpRoute) -> bool {
    route.raw.is_some()
        && route
            .handler
            .params
            .iter()
            .any(|p| p.name.name == "body" && matches!(p.type_ref, TypeRef::Stream(..)))
}

/// #2648: the TypeScript binder for a user parameter in a Worker wrapper or
/// entry dispatch.
///
/// On top of [`ts_ident`]'s reserved-word renames, a parameter whose name is one
/// the wrapper itself declares or reads after binding the parameters (`request`,
/// `surface`, `handlers`, `result`, `args`, ...) goes into the generated-name
/// namespace (`__id_<name>`), which the parser keeps free of user identifiers.
/// Otherwise such a parameter redeclares or shadows the wrapper's own local and
/// the Worker fails `tsc`.
///
/// `body` is *not* in this set: on an `http` route a parameter named `body` is
/// the request body the wrapper binds under that very name. The `call` dispatch,
/// where it is an ordinary parameter, renames it through [`call_wire_ident`].
pub(crate) fn wire_ident(name: &str) -> String {
    // Deliberately absent, because no user parameter is bound in their scope:
    // - `event` / `batch`: the scheduled / queue entries' parameters; those
    //   handlers are fed `event.scheduledTime` / the message directly.
    // - `servicePath`: read only by the `switch`, before any binding.
    // - `why`: the parameter of the `on rejected` wrapper, which has no user
    //   parameters.
    // `url` / `path` are listed preventively: today's dispatch reads them only in
    // the route conditions, before any parameter is bound, but a later read in
    // the route block would otherwise hit the user's string.
    const WRAPPER_NAMES: &[&str] = &[
        // The fetch/scheduled/queue entry's own parameters and locals.
        "request",
        "env",
        "ctx",
        "method",
        "surface",
        "url",
        "path",
        // `deps` is not listed: `ts_ident` already renames it.
        // The dispatch's decoded-arguments and result locals.
        "args",
        "argsObj",
        "result",
        // The compose-surface namespace and runtime constructors the wrappers
        // and the deserialising IIFEs reference by name.
        "handlers",
        "HttpResult",
        "Result",
        "Option",
        "Ok",
        "Err",
        "Some",
        "None",
    ];
    let t = ts_ident(name);
    if WRAPPER_NAMES.contains(&t.as_str()) {
        format!("__id_{t}")
    } else {
        t
    }
}

/// [`wire_ident`] for the `call` dispatch, where `body` is an ordinary
/// parameter and the dispatch's own encoded-result local is called `body`.
fn call_wire_ident(name: &str) -> String {
    if name == "body" {
        "__id_body".to_string()
    } else {
        wire_ident(name)
    }
}

/// Synthesise a deps object literal for invoking a handler from the
/// fetch entry point. Mirrors `compose`'s deps construction so handlers
/// see the same shape whether invoked through the surface or directly.
/// Returns the statements to append to the surrounding `switch` case body —
/// `emit_worker_entry`'s own caller extends the case with them.
fn emit_call_handler_dispatch(
    sname: &str,
    h: &Handler,
    actors: &std::collections::HashMap<String, bynk_syntax::ast::ActorDecl>,
    ru: &RuntimeUse,
) -> Vec<TsStmt> {
    let mut stmts: Vec<TsStmt> = Vec::new();
    // v0.54: a `by c: Caller` handler reads the caller's context name from the
    // `X-Bynk-Caller` header (stamped by `callService`) and threads it into the
    // surface call. I2 (#1971): `__verifyCall` already refused a request whose
    // caller holds no key here, so the header is present, non-empty and
    // proved: there is no separate missing-caller check.
    let has_caller = bynk_check::actors::caller_binder_for(h, actors).is_some();
    // I3 (#1971): a `call` sum's resolved actor, built by `admission_gate`.
    let has_who =
        bynk_check::actors::call_admission_for(h, actors).is_some_and(|a| a.sum.is_some());
    if has_caller {
        stmts.push(const_(
            "__caller",
            as_expr(
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit(wf::CALLER_HEADER)],
                ),
                TsType::named("string"),
            ),
        ));
    }

    if h.params.len() == 1 {
        let p = &h.params[0];
        let pname = &p.name.name;
        // The binding target must dodge JS reserved words (`class`, `const`, …)
        // that lex as valid Bynk identifiers; the `__r_` temp is already safe.
        let jname = call_wire_ident(pname);
        let dser_call = deserialise_call(&p.type_ref, "args", "$", ru);
        stmts.push(const_(format!("__r_{pname}"), dser_call));
        stmts.push(if_(
            strict_eq(member(ident(format!("__r_{pname}")), "tag"), str_lit("Err")),
            return_(Some(json_response(
                member(ident(format!("__r_{pname}")), "error"),
                400,
            ))),
        ));
        stmts.push(const_(
            jname.clone(),
            member(ident(format!("__r_{pname}")), "value"),
        ));
        let mut call_args = Vec::new();
        if has_who {
            call_args.push(ident("__who"));
        }
        if has_caller {
            call_args.push(ident("__caller"));
        }
        call_args.push(ident(jname));
        stmts.push(const_(
            "result",
            await_expr(call(member(ident("surface"), sname.to_string()), call_args)),
        ));
    } else {
        let typeof_args = TsExpr::Unary {
            op: bynk_ts::TsUnaryOp::Typeof,
            expr: Box::new(ident("args")),
        };
        stmts.push(if_(
            or_expr(
                or_expr(
                    strict_neq(typeof_args.clone(), str_lit("object")),
                    strict_eq(ident("args"), null_lit()),
                ),
                call(
                    member(ident("globalThis.Array"), "isArray"),
                    vec![ident("args")],
                ),
            ),
            return_(Some(json_response(
                json_error_kind(
                    "StructuralMismatch",
                    vec![
                        ("path".to_string(), str_lit("$")),
                        ("expected".to_string(), str_lit("object")),
                        ("actual".to_string(), typeof_args),
                    ],
                ),
                400,
            ))),
        ));
        stmts.push(const_(
            "argsObj",
            as_expr(ident("args"), index_signature_record_ty()),
        ));
        let mut names = Vec::new();
        for p in &h.params {
            // `pname` is the wire field name (the `argsObj` key + diagnostic path)
            // and must stay verbatim; `jname` is the reserved-word-safe binding
            // the surface call references.
            let pname = &p.name.name;
            let jname = call_wire_ident(pname);
            let dser = deserialise_call(
                &p.type_ref,
                &format!("argsObj[\"{pname}\"]"),
                &format!("$.{pname}"),
                ru,
            );
            stmts.push(const_(format!("__r_{pname}"), dser));
            stmts.push(if_(
                strict_eq(member(ident(format!("__r_{pname}")), "tag"), str_lit("Err")),
                return_(Some(json_response(
                    member(ident(format!("__r_{pname}")), "error"),
                    400,
                ))),
            ));
            stmts.push(const_(
                jname.clone(),
                member(ident(format!("__r_{pname}")), "value"),
            ));
            names.push(jname);
        }
        // Prepend the caller (when present) without a dangling comma for the
        // zero-param case.
        let mut call_args: Vec<TsExpr> = Vec::new();
        if has_who {
            call_args.push(ident("__who"));
        }
        if has_caller {
            call_args.push(ident("__caller"));
        }
        call_args.extend(names.into_iter().map(ident));
        stmts.push(const_(
            "result",
            await_expr(call(member(ident("surface"), sname.to_string()), call_args)),
        ));
    }
    let ser_expr = serialise_call(&h.return_type, "result", ru);
    stmts.push(const_("body", ser_expr));
    stmts.push(return_(Some(new_response(vec![
        json_stringify(ident("body")),
        status_json_headers_obj(200),
    ]))));
    stmts
}

/// `{ [k: string]: JsonValue }` — the structural type `emit_call_handler_dispatch`'s
/// own multi-param branch casts `args` to before indexing it by field name.
fn index_signature_record_ty() -> TsType {
    TsType::Object(vec![TsTypeMember::index(
        "k",
        TsType::named("string"),
        TsType::named("__JsonValue"),
    )])
}

/// Emit code that converts the raw path-parameter string to the parameter's
/// declared type. For plain `String`, this is a direct assignment; for
/// refined or opaque `String`-based types we lower through `T.of(...)`
/// which performs refinement validation and returns 400 on failure.
///
/// #2252: a type from a consumed context is imported type-only, so it has no
/// `of` here. It is validated through this context's own codec,
/// `handlers.__deserialise_T(raw, "path.<p>")`, whose refinement check is the
/// owner's and whose error is the same `RefinementViolation` this answers.
fn emit_path_param_construction(
    pname: &str,
    t: &TypeRef,
    cors_const: Option<&str>,
    security_const: Option<&str>,
    rejected: Option<&str>,
    consumed_types: &std::collections::HashSet<String>,
) -> Vec<TsStmt> {
    // `pname` names the wire segment (the `__raw_`/`__r_` temps and diagnostic
    // path stay verbatim); `jname` is the reserved-word-safe binding the surface
    // call references.
    let jname = wire_ident(pname);
    let raw = format!("__raw_{pname}");
    match t {
        TypeRef::Base(BaseType::String, _) => {
            vec![const_(jname, ident(raw))]
        }
        TypeRef::Named(id) => {
            let result = format!("__r_{pname}");
            let wire_path = str_lit(format!("path.{pname}"));
            let consumed = consumed_types.contains(&id.name);
            let construct = if consumed {
                method_call(
                    ident("handlers"),
                    &format!("__deserialise_{}", id.name),
                    vec![ident(raw), wire_path.clone()],
                )
            } else {
                method_call(
                    member(ident("handlers"), id.name.clone()),
                    "of",
                    vec![ident(raw)],
                )
            };
            let mut stmts = vec![const_(result.clone(), construct)];
            // The path-param refinement `400` reflects the offending segment into
            // its JSON body, so it is stamped with the service policy (#659).
            // A codec's error is already that body (`kind`, `path`, `violation`).
            let body = if consumed {
                member(ident(result.clone()), "error")
            } else {
                json_error_kind(
                    "RefinementViolation",
                    vec![
                        ("path".to_string(), wire_path),
                        (
                            "violation".to_string(),
                            member(ident(result.clone()), "error"),
                        ),
                    ],
                )
            };
            let inner_resp = json_response(body, 400);
            let reject = stamp_rejection(inner_resp, cors_const, security_const, rejected);
            stmts.push(if_(
                strict_eq(member(ident(result.clone()), "tag"), str_lit("Err")),
                return_(Some(reject)),
            ));
            stmts.push(const_(jname, member(ident(result), "value")));
            stmts
        }
        TypeRef::Base(..)
        | TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::List(..)
        | TypeRef::Map(..)
        | TypeRef::Set(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..)
        | TypeRef::App { .. } => {
            // P7.2 (review of #1300): this used to emit `__raw_{pname}` as a
            // silent fallback ("other shapes are rejected by the static
            // check"), which was survivable only while the receiving wrapper
            // param was itself `any`/`unknown` — a raw `string` satisfies
            // anything. Now that `emit_http_wrapper`/`emit_http_oidc_wrapper`/
            // `emit_http_sum_wrapper` type that same param at its real
            // declared type (`qualified_type_ref`, `workers.rs`), the fallback
            // would hand a `string` to a `number`/whatever-typed parameter and
            // ship a project `tsc --strict` rejects — silently, since this
            // function has no way to surface that. `bynk.http.path_param_not_stringy`
            // (`bynk-check/src/context_checks.rs`) is a total guarantee: a path
            // parameter's `TypeRef` is checked to be exactly `String` or a
            // `String`-based `Named` before emission ever runs, so this arm is
            // unreachable for any program that passed the checker. Fail loudly
            // if that guarantee is ever wrong, rather than emit TypeScript that
            // cannot compile.
            panic!(
                "bynk internal error: path parameter `{pname}` reached emission with a \
                 non-string-constructible type ({t:?}) — `bynk.http.path_param_not_stringy` \
                 should have rejected this at check time"
            );
        }
    }
}

/// Strip the `Effect[HttpResult[_]]` wrapper to expose the inner payload type
/// `T`. Used to choose the right serialiser when emitting the HttpResult
/// body.
fn http_result_inner(t: &TypeRef) -> TypeRef {
    let inner = if let TypeRef::Effect(t, _) = t {
        t.as_ref()
    } else {
        t
    };
    if let TypeRef::HttpResult(payload, _) = inner {
        (**payload).clone()
    } else {
        inner.clone()
    }
}

/// v0.176 (#642): the callee side of the workers cross-context boundary. Both
/// of these used to be a *parallel* dispatch that shadowed
/// `serialisation.rs`'s — and drifted from it. `serialise_call` cast a `Bytes`
/// to `JsonValue` while `deserialise_call` base64-decoded it, the asymmetry that
/// made `bynk.types.bytes_at_workers_boundary` necessary. They are now one line
/// each over the shared dispatch, reaching this context's helpers through the
/// `handlers` namespace the entry point imports.
pub(crate) fn deserialise_call(
    t: &TypeRef,
    json_expr: &str,
    path: &str,
    ru: &RuntimeUse,
) -> TsExpr {
    deserialise_node(t, &bynk_ts::TsExpr::Ident(json_expr.to_string()), path, ru)
}

/// [`deserialise_call`] over a built node, for a subject that names a runtime type (`msg.body as
/// __JsonValue`): the import is read off the tree, which cannot see a name spelled as text.
fn deserialise_node(t: &TypeRef, json: &TsExpr, path: &str, ru: &RuntimeUse) -> TsExpr {
    crate::emitter::serialisation::deserialise_expr_via(t, json, path, "handlers.", ru)
}

fn serialise_call(t: &TypeRef, value: &str, ru: &RuntimeUse) -> TsExpr {
    crate::emitter::serialisation::serialise_expr_via(
        t,
        &bynk_ts::TsExpr::Ident(value.to_string()),
        "handlers.",
        ru,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use bynk_model::IrHttpMethod;

    #[test]
    fn param_count_counts_named_segments_only() {
        assert_eq!(param_count("/items"), 0);
        assert_eq!(param_count("/items/:id"), 1);
        assert_eq!(param_count("/a/:x/b/:y"), 2);
        // A bare `:` names nothing.
        assert_eq!(param_count("/a/:"), 0);
    }

    #[test]
    fn allowed_methods_add_head_for_get_and_always_options_in_order() {
        assert_eq!(
            derive_allowed_methods([IrHttpMethod::Get].into_iter()),
            ["GET", "HEAD", "OPTIONS"]
        );
        assert_eq!(
            derive_allowed_methods([IrHttpMethod::Post].into_iter()),
            ["OPTIONS", "POST"]
        );
        assert_eq!(
            derive_allowed_methods(
                [
                    IrHttpMethod::Put,
                    IrHttpMethod::Get,
                    IrHttpMethod::Delete,
                    IrHttpMethod::Get
                ]
                .into_iter()
            ),
            ["DELETE", "GET", "HEAD", "OPTIONS", "PUT"]
        );
        assert_eq!(derive_allowed_methods(std::iter::empty()), ["OPTIONS"]);
    }

    fn printed(stmts: &[TsStmt]) -> String {
        stmts.iter().map(|s| bynk_ts::print_stmt(s, 0)).collect()
    }

    /// Default (non-member) output: the guard is the bare prefix test, nothing is
    /// marked, and the edge-cache return is the one it has always been.
    #[test]
    fn outside_a_group_nothing_is_marked_or_gated() {
        let guard = bynk_ts::print_expr(&own_path_guard(false, "/_bynk/call/", &[]));
        assert_eq!(guard, "path.startsWith(\"/_bynk/call/\")");
        let marked = bynk_ts::print_expr(&miss(false, text_response("Not Found", 404)));
        assert!(!marked.contains("__miss"), "{marked}");
        let edge = printed(&edge_cache_return(false));
        assert!(
            !edge.contains("isMiss") && !edge.contains("__miss"),
            "{edge}"
        );
        assert!(
            edge.contains("__edgeCacheGuard(request, await __bynkFetch("),
            "{edge}"
        );
    }

    /// A member tests that the service is its own before anything reads the body
    /// or verifies a signature, so a request for another member is left untouched.
    #[test]
    fn a_member_tests_ownership_in_the_guard_before_any_gate() {
        let names = vec!["a".to_string(), "b".to_string()];
        let guard = bynk_ts::print_expr(&own_path_guard(true, "/_bynk/call/", &names));
        assert_eq!(
            guard,
            "path.startsWith(\"/_bynk/call/\") && [\"a\", \"b\"].includes(path.slice(\"/_bynk/call/\".length))"
        );
    }

    /// The router's own answers are marked in a member; the marker is a call on a
    /// module-level set, never a header.
    #[test]
    fn a_member_marks_its_routers_own_answers() {
        let marked = bynk_ts::print_expr(&miss(true, text_response("Not Found", 404)));
        assert!(
            marked.starts_with("__miss(new globalThis.Response(\"Not Found\""),
            "{marked}"
        );
        assert!(MISS_HELPER.contains("new globalThis.WeakSet"));
        assert!(
            !MISS_HELPER.contains("headers"),
            "the mark is never a header"
        );
    }

    /// The edge-cache guard can answer with a copy, so a member re-marks it.
    #[test]
    fn a_members_edge_cache_guard_keeps_a_miss_a_miss() {
        let edge = printed(&edge_cache_return(true));
        assert!(
            edge.contains("isMiss(__answer) ? __miss(__guarded) : __guarded"),
            "{edge}"
        );
    }

    /// The admin route is gated on the class and its absence is a marked miss.
    #[test]
    fn a_members_migrate_route_is_gated_on_its_class_and_marks_its_miss() {
        let migratable = vec![("Cart".to_string(), 2)];
        let route = bynk_ts::print_stmt(&migrate_route("shop.cart", true, &migratable), 0);
        assert!(route.contains("[\"Cart\"].includes("), "{route}");
        assert!(
            route.contains("return __miss(new globalThis.Response(\"Not found\""),
            "{route}"
        );
        let alone = bynk_ts::print_stmt(&migrate_route("shop.cart", false, &migratable), 0);
        assert!(
            !alone.contains("includes") && !alone.contains("__miss"),
            "{alone}"
        );
    }
}
