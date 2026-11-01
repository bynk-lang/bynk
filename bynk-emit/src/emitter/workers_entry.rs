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
use crate::emitter::ts_ident;
use crate::project::UnitTable;
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
        test: Some(test),
        body,
        default_braced: false,
        // Arc E slice 6 (#1445): every real non-`default` case in this file
        // is `{ }`-blocked, same as before `case_braced` existed.
        case_braced: true,
    }
}

fn default_case(body: Vec<TsStmt>) -> TsSwitchCase {
    TsSwitchCase {
        test: None,
        body,
        default_braced: false,
        case_braced: true,
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

/// State-migrations S4 (#2259 Decision D): the `/_bynk/migrate/<Class>`
/// dispatch. See the call site for the order of its checks.
fn migrate_route(context: &str, migratable: &[(String, i64)]) -> TsStmt {
    let not_found = || return_(Some(text_response("Not found", 404)));
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
    if_(
        method_call(
            ident("path"),
            "startsWith",
            vec![str_lit(wf::MIGRATE_PATH_PREFIX)],
        ),
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

/// `missing_bindings`'s own dedup logic (`crate::emitter`), reimplemented
/// over a `Vec<String>` of already-real import names — the tree-node analogue
/// of the old text post-pass, the same precedent `workers.rs`'s own
/// `append_missing_bindings` (#1321) already set for the identical problem:
/// the runtime import line is now a `TsDecl::Import` node built once, so the
/// post-pass that appends codec-family runtime names (#914) can no longer
/// splice into printed text (`crate::emitter::inject_runtime_imports`'s own
/// mechanism) and instead mutates the node's `names` directly, before the
/// import decl is pushed into the program. Strips an optional `type ` prefix,
/// compares bare names, so a name already present (bare or `type`-prefixed)
/// is never added twice.
fn append_missing_bindings(names: &mut Vec<String>, extra: &str) {
    fn bare(binding: &str) -> &str {
        binding
            .trim()
            .strip_prefix("type ")
            .unwrap_or(binding.trim())
    }
    let present: std::collections::HashSet<&str> = names.iter().map(|n| bare(n)).collect();
    let wanted: Vec<String> = extra
        .split(',')
        .map(str::trim)
        .filter(|b| !b.is_empty() && !present.contains(bare(b)))
        .map(str::to_string)
        .collect();
    names.extend(wanted);
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_worker_entry(
    context: &str,
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
) -> TsProgram {
    let mut program = TsProgram::new();
    // Which conditional runtime helpers the entry's own inbound/outbound codecs
    // reach for; read back below to decide the import injection.
    let runtime_use = RuntimeUse::default();
    program.push(TsStmt::comment(
        "Generated by bynkc — do not edit by hand.",
        None,
    ));
    program.push(TsStmt::comment(
        format!("Worker entry point for context `{context}`."),
        None,
    ));

    // v0.79: if any handler uses `~>`, each entry point captures the runtime's
    // execution context and threads it into `compose`, so a fire-and-forget send
    // can hand its promise to `waitUntil`. Otherwise the signatures are unchanged.
    let ctx_uses_send = table.services.values().any(|s| {
        s.handlers
            .iter()
            .any(|h| crate::emitter::block_uses_send(&h.body))
    });
    let exec_params: Vec<TsParam> = if ctx_uses_send {
        vec![TsParam {
            name: "ctx".to_string(),
            ty: Some(TsType::Object(vec![TsTypeMember::method(
                "waitUntil",
                vec![TsParam {
                    name: "promise".to_string(),
                    ty: Some(TsType::named_with_args(
                        "globalThis.Promise",
                        vec![TsType::named("unknown")],
                    )),
                    optional: false,
                }],
                TsType::named("void"),
            )])),
            optional: false,
        }]
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
        vec![TsParam {
            name: "ctx".to_string(),
            ty: Some(TsType::Object(vec![TsTypeMember::method(
                "waitUntil",
                vec![TsParam {
                    name: "promise".to_string(),
                    ty: Some(TsType::named_with_args(
                        "globalThis.Promise",
                        vec![TsType::named("unknown")],
                    )),
                    optional: false,
                }],
                TsType::named("void"),
            )])),
            optional: true,
        }]
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
            if let bynk_ir::IrHandlerKind::Http { method, path } =
                bynk_lower::lower_handler_kind_ir(&h.kind)
            {
                http_routes.push(HttpRoute {
                    service: (*sname).clone(),
                    method,
                    path,
                    handler: h.clone(),
                    // v0.47: a Bearer handler's surface wrapper runs the
                    // verification seam and needs the request passed in.
                    bearer: bynk_check::actors::bearer_seam_for(h, &table.actors).is_some(),
                    // v0.151: an `Oidc` handler's wrapper — like Bearer's — runs the
                    // verification seam and takes the request as its first argument.
                    oidc: bynk_check::actors::oidc_seam_for(h, &table.actors).is_some(),
                    signature: bynk_check::actors::signature_seam_for(h, &table.actors),
                    // v0.52: a multi-actor sum handler's wrapper owns the whole
                    // boundary (raw read, first-wins resolution, body parse), so
                    // the entry just passes `request` and skips the body parse.
                    sum: bynk_check::actors::sum_members_for(h, &table.actors).is_some(),
                    // v0.140 (ADR 0163): the handler's `@cache` freshness policy, if
                    // any — lowered to a `Cache-Control` on this GET's responses.
                    cache: bynk_lower::lower_route_cache_ir(h),
                    edge_cache,
                    vary_locale: edge_cache && needs_locale_request,
                    // v0.142 (ADR 0165): the route's effective request-body ceiling
                    // in bytes — the handler's `@limit` if any, else the service's
                    // `limits { }`, else none. `Some` drives the synthesised `413`
                    // `Content-Length` guard before the body is read.
                    max_body: effective_max_body(service.limits.as_ref(), h),
                    raw: h.raw_content_type().map(str::to_string),
                    rejected: rejected_surface(service, sname),
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
    // A context with a `GET` route synthesises `HEAD` answers, which strip the
    // built `Response` body through the runtime's `headResponse`.
    let has_get_route = http_routes
        .iter()
        .any(|r| r.method == bynk_ir::IrHttpMethod::Get);

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
            if let bynk_ir::IrHandlerKind::Cron { expr } =
                bynk_lower::lower_handler_kind_ir(&h.kind)
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
                bynk_lower::lower_handler_kind_ir(&h.kind),
                bynk_ir::IrHandlerKind::Message
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
                bynk_lower::lower_handler_kind_ir(&h.kind),
                bynk_ir::IrHandlerKind::Open
            ) {
                ws_open_routes.push((*sname, h));
            }
        }
    }

    let has_http = !http_routes.is_empty();
    // #973: whether this Worker hosts any `from Events(E)` subscriber — decides
    // whether the entry route needs `deserialiseEventEnvelope` to validate the
    // envelope before dispatching (see the `/_bynk/event/` block below).
    let has_event_services = service_names.iter().any(|sname| {
        table.services.get(*sname).is_some_and(|s| {
            s.handlers.iter().any(|h| {
                matches!(
                    bynk_lower::lower_handler_kind_ir(&h.kind),
                    bynk_ir::IrHandlerKind::Event
                )
            })
        })
    });

    let mut imports: Vec<String> = vec![
        "Ok".to_string(),
        "Err".to_string(),
        "type Result".to_string(),
        "type __JsonValue".to_string(),
        "type __BoundaryError".to_string(),
        "__boundaryError".to_string(),
        // I2 (#1971): every Worker answers `/_bynk/call/`, so every Worker
        // verifies it.
        "__verifyCall".to_string(),
        // #2193: every Worker's `fetch` catch answers through it.
        "__faultResponse".to_string(),
    ];
    if !migratable.is_empty() {
        imports.push("__sweepAgentInstances".to_string());
    }
    if has_http {
        imports.push("__matchPath".to_string());
        imports.push("__httpResultToResponse".to_string());
    }
    if has_event_services {
        imports.push("__deserialiseEventEnvelope".to_string());
    }
    // v0.139: a context that answers `HEAD` (any `GET` route) strips the built
    // response body through `headResponse`.
    if has_get_route {
        imports.push("__headResponse".to_string());
        // v0.140 (ADR 0163): every `GET` carries a weak `ETag` and is answered
        // `304` on a matching `If-None-Match` via `notModifiedIfMatch`.
        imports.push("__notModifiedIfMatch".to_string());
    }
    // v0.140 (ADR 0163): a `GET` carrying `@cache` stamps `Cache-Control` through
    // `applyCache` — imported only when some route declares one.
    if http_routes.iter().any(|r| r.cache.is_some()) {
        imports.push(
            if edge_cache {
                "__applyEdgeCache"
            } else {
                "__applyCache"
            }
            .to_string(),
        );
    }
    // L7 (#2414, Decision C): an opted-in Worker marks a `GET` route without
    // `@cache` `no-store`, and runs every answer through `edgeCacheGuard`.
    if edge_cache {
        if http_routes
            .iter()
            .any(|r| r.method == bynk_ir::IrHttpMethod::Get && r.cache.is_none())
        {
            imports.push("__noStore".to_string());
        }
        imports.push("__edgeCacheGuard".to_string());
        if needs_locale_request && http_routes.iter().any(|r| r.cache.is_some()) {
            imports.push("__varyAcceptLanguage".to_string());
        }
    }
    // v0.131: a context with a CORS-enabled service imports the CORS helpers.
    if !cors_services.is_empty() {
        imports.push("type __CorsPolicy".to_string());
        imports.push("__applyCors".to_string());
        imports.push("__corsPreflightResponse".to_string());
    }
    // v0.141: a context with any `from http` service imports the security-header
    // helper — every such service stamps at least the default `nosniff`.
    if !security_services.is_empty() {
        imports.push("type __SecurityPolicy".to_string());
        imports.push("__applySecurityHeaders".to_string());
    }
    // A capped route with a body reads it through the counting reader; a raw route
    // with a declared media type checks the request's `Content-Type`.
    let body_has = |r: &HttpRoute| r.handler.params.iter().any(|p| p.name.name == "body");
    if http_routes
        .iter()
        .any(|r| !r.sum && body_has(r) && r.max_body.is_some())
    {
        imports.push("__readBodyCapped".to_string());
    }
    if http_routes
        .iter()
        .any(|r| body_has(r) && r.raw.as_deref().is_some_and(|ct| ct != "*/*"))
    {
        imports.push("__mediaTypeMatches".to_string());
    }
    // #2341 (decision A): a route taking `headers` builds them from the request.
    if http_routes.iter().any(|r| {
        r.handler
            .params
            .iter()
            .any(|p| p.name.name == "headers" && matches!(p.type_ref, TypeRef::RequestHeaders(_)))
    }) {
        imports.push("__requestHeaders".to_string());
    }
    // #2341 (decision C): a service with `on rejected` renders its rejections.
    if http_routes.iter().any(|r| r.rejected.is_some()) {
        imports.push("__renderRejection".to_string());
    }
    // v0.51: a context with a Signature handler imports the HMAC verifier.
    if http_routes.iter().any(|r| r.signature.is_some()) {
        imports.push("__verifySignatureHmacSha256".to_string());
    }
    // The runtime import's own `names` list isn't finalised yet — #914's own
    // codec-family follow-on (here, only `Bytes`) is only known once the
    // whole body below has run and populated `runtime_use`, so this decl (and
    // the rest of the header) is held and pushed after that, in the same
    // relative order it would have printed in — the tree-node analogue of the
    // old text post-pass, the same deferral `workers.rs`'s own conversion
    // (#1321) already established for the identical problem.
    let mut header_decls: Vec<TsStmt> = vec![
        TsStmt::decl(
            TsDecl::Import {
                type_only: false,
                names: vec!["compose".to_string(), "type Env".to_string()],
                from: "./compose.js".to_string(),
            },
            None,
        ),
        TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: "handlers".to_string(),
                from: "./handlers.js".to_string(),
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
    if !agent_names.is_empty() {
        header_decls.push(TsStmt::decl(
            TsDecl::ReExport {
                names: agent_names.iter().map(|n| n.to_string()).collect(),
                from: "./handlers.js".to_string(),
            },
            None,
        ));
    }
    // Events track, slice 0 (spine #936, ADR 0284): same re-export
    // requirement, for the fan-out DO — it lives in its own file
    // (`events_fanout.ts`, not `handlers.ts`; a fan-out DO has no backing
    // `AgentDecl` for `emit_agent` to emit it from).
    if uses_emit {
        header_decls.push(TsStmt::decl(
            TsDecl::ReExport {
                names: vec![crate::emitter::wrangler::EVENTS_FANOUT_CLASS_NAME.to_string()],
                from: "./events_fanout.js".to_string(),
            },
            None,
        ));
    }

    let mut fetch_params: Vec<TsParam> = vec![
        TsParam {
            name: "request".to_string(),
            ty: Some(TsType::named("globalThis.Request")),
            optional: false,
        },
        TsParam {
            name: "env".to_string(),
            ty: Some(TsType::named("Env")),
            optional: false,
        },
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
    let mut call_cases: Vec<TsSwitchCase> = Vec::new();
    for sname in &service_names {
        let service = table.services.get(*sname).unwrap();
        let Some(h) = service.handlers.iter().find(|h| {
            matches!(
                bynk_lower::lower_handler_kind_ir(&h.kind),
                bynk_ir::IrHandlerKind::Call
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
            &runtime_use,
        ));
        call_cases.push(case_(str_lit(sname.to_string()), case_body));
    }
    call_cases.push(default_case(vec![return_(Some(text_response(
        CallCheck::UnknownService.text_body().unwrap_or_default(),
        check_status(CallCheck::UnknownService),
    )))]));
    // #1969: the call dispatch makes the published wire format's checks
    // (`bynk_check::wire_format::CALL_CHECKS`) in their published order:
    // method, body size and signature (I2), wire version, then (per service,
    // above) the service, its contract, the body's JSON and the arguments.
    try_body.push(if_(
        method_call(
            ident("path"),
            "startsWith",
            vec![str_lit(wf::CALL_PATH_PREFIX)],
        ),
        block(
            std::iter::once(call_method_gate())
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
                .collect(),
        ),
    ));
    try_body.push(TsStmt::blank(None));

    // 1.25. State-migrations S4 (#2259 Decision D): `bynk migrate sweep`'s
    // wake. Present only while a sweep has set its key: without it the route
    // is `404`, as if absent. It admits the sweep's caller name alone, so a
    // context holding an edge key to this Worker cannot use it, and then
    // verifies the signature under the sweep's key.
    if !migratable.is_empty() {
        try_body.push(migrate_route(context, migratable));
        try_body.push(TsStmt::blank(None));
    }

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
                        bynk_lower::lower_handler_kind_ir(&h.kind),
                        bynk_ir::IrHandlerKind::Event
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
                        bynk_lower::lower_handler_kind_ir(&h.kind),
                        bynk_ir::IrHandlerKind::Event
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
        event_cases.push(default_case(vec![return_(Some(text_response(
            "Not found",
            404,
        )))]));
        event_body.push(switch_stmt(ident("servicePath"), event_cases));

        try_body.push(if_(
            method_call(ident("path"), "startsWith", vec![str_lit("/_bynk/event/")]),
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
        fallthrough_body.push(return_(Some(stamped)));
        try_body.push(if_(match_cond, block(fallthrough_body)));
    }

    try_body.push(return_(Some(text_response("Not Found", 404))));

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
        vec![
            expr_stmt(method_call(
                ident("globalThis.console"),
                "error",
                vec![
                    TsExpr::template_lit(
                        vec![format!("{context} "), " faulted".to_string()],
                        vec![ident("__route")],
                    ),
                    fault_detail("e"),
                ],
            )),
            report_fault(),
            return_(Some(call(
                ident("__faultResponse"),
                vec![str_lit(context.to_string()), ident("__route")],
            ))),
        ],
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
        vec![return_(Some(call(
            ident("__edgeCacheGuard"),
            vec![
                ident("request"),
                await_expr(call(
                    ident("__bynkFetch"),
                    vec![ident("request"), ident("env"), ident("ctx")],
                )),
            ],
        )))]
    } else {
        fetch_body
    };
    let mut default_entries: Vec<TsObjectEntry> = vec![TsObjectEntry::Method {
        name: "fetch".to_string(),
        is_async: true,
        generics: Vec::new(),
        params: fetch_params,
        return_type: fetch_return,
        doc: None,
        inline: false,
        body: fetch_body,
    }];

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

    // v0.10b: queue (consumer) entry point. Dispatches on `batch.queue`,
    // deserialises each message, invokes the handler, and acks on `Ok` /
    // retries on `Err` (a deserialisation failure also retries).
    if !queue_routes.is_empty() {
        default_entries.push(emit_queue_handler(
            &queue_routes,
            &fault_exec_params,
            &other_compose_args,
            &runtime_use,
        ));
    }

    // v0.176 (#642): a `Bytes` may now cross a workers boundary, so the
    // entry's inbound codec can reference the base64 helpers — fold in
    // whatever the body above actually reached for, the tree-node analogue
    // of the old text post-pass (`append_missing_bindings`'s own doc
    // explains why it can't reuse `crate::emitter::inject_runtime_imports`
    // unchanged). This is why the runtime import decl was held rather than
    // pushed immediately after being computed: `runtime_use.bytes()` is only
    // known now, after every `deserialise_call`/`serialise_call` site in
    // `fetch`/`scheduled`/`queue`'s own bodies above has run.
    if runtime_use.bytes() {
        append_missing_bindings(&mut imports, crate::emitter::BYTES_RUNTIME_IMPORTS);
    }
    if runtime_use.bytes_ops() {
        append_missing_bindings(&mut imports, crate::emitter::BYTES_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.time_ops() {
        append_missing_bindings(&mut imports, crate::emitter::TIME_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.settle_all() {
        append_missing_bindings(&mut imports, crate::emitter::SETTLE_ALL_RUNTIME_IMPORTS);
    }
    if runtime_use.url_ops() {
        append_missing_bindings(&mut imports, crate::emitter::URL_OPS_RUNTIME_IMPORTS);
    }
    if runtime_use.request_headers() {
        append_missing_bindings(
            &mut imports,
            crate::emitter::REQUEST_HEADERS_RUNTIME_IMPORTS,
        );
    }
    if runtime_use.eq() {
        append_missing_bindings(&mut imports, crate::emitter::EQ_RUNTIME_IMPORTS);
    }
    if runtime_use.int() {
        append_missing_bindings(&mut imports, crate::emitter::INT_RUNTIME_IMPORTS);
    }
    append_missing_bindings(&mut imports, &runtime_use.cmp_imports());
    for stmt in
        crate::emitter::runtime_files::runtime_import_stmts(false, &imports, "../../runtime.js")
    {
        program.push(stmt);
    }
    for decl in header_decls {
        program.push(decl);
    }
    program.push(TsStmt::raw(REPORT_FAULT_HELPER, None));
    program.push(TsStmt::decl(
        TsDecl::ExportDefault(TsExpr::multiline_object_entries(default_entries)),
        None,
    ));

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
    let mut params = vec![TsParam {
        name: "event".to_string(),
        ty: Some(TsType::Object(vec![
            TsTypeMember::readonly_prop("cron", TsType::named("string")),
            TsTypeMember::readonly_prop("scheduledTime", TsType::named("number")),
        ])),
        optional: false,
    }];
    params.push(TsParam {
        name: "env".to_string(),
        ty: Some(TsType::named("Env")),
        optional: false,
    });
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
        params.push(TsParam {
            name: "ctx".to_string(),
            ty: Some(TsType::Object(vec![result_member])),
            optional: true,
        });
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
    let mut params = vec![TsParam {
        name: "batch".to_string(),
        ty: Some(TsType::Object(vec![
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
        optional: false,
    }];
    params.push(TsParam {
        name: "env".to_string(),
        ty: Some(TsType::named("Env")),
        optional: false,
    });
    params.extend(exec_params.iter().cloned());

    let mut body: Vec<TsStmt> = vec![const_(
        "surface",
        call(ident("compose"), compose_args.to_vec()),
    )];
    let mut cases: Vec<TsSwitchCase> = Vec::new();
    for route in queue_routes {
        let method_key = crate::emitter::queue_handler_method_name(&route.service, route.index);
        let dser = match &route.msg_type {
            Some(t) => deserialise_call(t, "(msg.body as __JsonValue)", "$", ru),
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
fn derive_allowed_methods(methods: impl Iterator<Item = bynk_ir::IrHttpMethod>) -> Vec<String> {
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
    method: bynk_ir::IrHttpMethod,
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
    cache: Option<bynk_ir::CacheIr>,
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
    /// #2341 (decision C): the compose-surface method of the service's
    /// `on rejected` (`rejected_<service>`), when it declares one. The route's
    /// synthesised rejections are rendered through it.
    rejected: Option<String>,
}

/// v0.142 (ADR 0165): resolve a route's effective request-body ceiling in bytes.
/// A route's `@limit(maxBody:)` annotation wins over the service's `limits { }`
/// default (DECISION B); with neither, the route has no cap (`None`). Project
/// validation (`bynk.http.limit_*` / `limits_*`) has already rejected a malformed
/// or misplaced `@limit`/`limits`, so a bad/absent value here simply yields no
/// cap. Only a body-taking method can carry a cap — a `@limit` on a GET/DELETE is
/// a checker error, so it never reaches a live route here. The route-annotation
/// half is `bynk-emit::ir`'s own [`bynk_lower::lower_route_limit_ir`]
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
    let method_guard = if route.method == bynk_ir::IrHttpMethod::Get {
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
            call_args.push(call(ident("__requestHeaders"), vec![ident("request")]));
            continue;
        }
        // It's a path parameter — extract from __m.params and construct. The
        // `__m.params` key is the wire segment name (verbatim); the constructed
        // binding must dodge JS reserved words for the surface call.
        let jname = ts_ident(pname);
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
        // policy just like the handled `200`. Built here, at their use site, to
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
        let counted = route.max_body;
        let raw = route.raw.is_some();
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
        if !raw {
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
            let signed = if raw {
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
            if !raw {
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
        if raw {
            // The octets, untouched: no JSON, no base64.
            guarded.push(const_("body", ident("__bytes")));
        } else {
            let dser = deserialise_call(&body_param.type_ref, "__body_json", "$", ru);
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
            guarded.push(const_("body", member(ident("__r_body"), "value")));
        }
    }

    // Invoke the handler and serialise the HttpResult. The handler is
    // wrapped on the surface so its deps are wired by `compose`. v0.47: a
    // Bearer wrapper takes the request first (it runs the verification seam).
    let surface_args: Vec<TsExpr> = if route.bearer || route.sum || route.oidc {
        // The Bearer, Oidc, and sum wrappers take the request first (they run the
        // verification seam); a sum wrapper also reads/parses the body itself,
        // so `call_args` here carries only the path params.
        let mut a = vec![ident("request")];
        a.extend(call_args.iter().cloned());
        a
    } else {
        call_args
    };
    guarded.push(const_(
        "result",
        await_expr(call(member(ident("surface"), method_key), surface_args)),
    ));
    let inner_ty = http_result_inner(&h.return_type);
    let ser_fn = crate::emitter::serialisation::serialise_ref_via(&inner_ty, "handlers.", ru);
    // v0.140 (ADR 0163): a `GET` response carries a weak `ETag` (an `Ok` body gets
    // the validator via `weakEtag`), an optional `@cache` `Cache-Control`
    // (`applyCache`), and is answered `304` when the request revalidates
    // (`notModifiedIfMatch`) — composed innermost-first so CORS still stamps the
    // `304`. Non-`GET`/unsafe methods are byte-for-byte unchanged: no validator, no
    // conditional, no freshness (DECISION B).
    let response_expr = if route.method == bynk_ir::IrHttpMethod::Get {
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
    if route.method == bynk_ir::IrHttpMethod::Get {
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
        let jname = ts_ident(pname);
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
            let jname = ts_ident(pname);
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
    let jname = ts_ident(pname);
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
        _ => {
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
    let inner = match t {
        TypeRef::Effect(t, _) => t.as_ref(),
        other => other,
    };
    match inner {
        TypeRef::HttpResult(payload, _) => (**payload).clone(),
        other => other.clone(),
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
    crate::emitter::serialisation::deserialise_expr_via(t, json_expr, path, "handlers.", ru)
}

fn serialise_call(t: &TypeRef, value: &str, ru: &RuntimeUse) -> TsExpr {
    crate::emitter::serialisation::serialise_expr_via(t, value, "handlers.", ru)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::emit_workers;
    use bynk_ir::IrHttpMethod;

    /// #2136: the test build of a context a `system` suite drives stores each
    /// cron run's `Result` on the execution context; the deployable build
    /// (suites stripped) is byte-identical to a project with no suite at all.
    #[test]
    fn scheduled_result_hook_is_test_build_only() {
        const JOBS: &str = "context ops.jobs\n\nservice ticker from cron {\n\ton schedule(\"*/5 * * * *\") (at: Int) -> Effect[Result[(), String]] {\n\t\tOk(())\n\t}\n}\n\nservice inbox from queue(\"jobs\") {\n\ton message(n: Int) -> Effect[QueueResult] {\n\t\tAck\n\t}\n}\n";
        const SUITE: &str = "suite ops.jobs as system {\n\tcase \"ticks\" {\n\t\tlet r <- ticker.schedule(\"*/5 * * * *\", 0)\n\t\texpect r is Ok(_)\n\t}\n}\n";
        let entry = "workers/ops-jobs/index.ts";
        let with_suite = [("ops/jobs.bynk", JOBS), ("tests/jobs.bynk", SUITE)];
        let test_build = crate::testkit::emit_project_with_tests(
            &with_suite,
            crate::project::BuildTarget::Workers,
            true,
        );
        assert!(
            test_build
                .text(entry)
                .contains("ctx.__bynkResult = result;")
        );
        let deploy = crate::testkit::emit_project_with_tests(
            &with_suite,
            crate::project::BuildTarget::Workers,
            false,
        );
        let bare = emit_workers(&[("ops/jobs.bynk", JOBS)]);
        assert_eq!(deploy.text(entry), bare.text(entry));
        assert!(!deploy.text(entry).contains("__bynkResult"));
    }

    /// #2137: the test build of a context a `system` harness stands up reads
    /// the harness clock (`env.__bynkClock`) in its `compose` and its agents'
    /// deps; the deployable build is byte-identical to a project with no suite.
    #[test]
    fn harness_clock_seam_is_test_build_only() {
        const HOLDS: &str = "context ck.holds\n\nconsumes bynk { Clock }\n\nagent Hold {\n\tkey id: String\n\tstore n: Cell[Int]\n\n\ton call at() -> Effect[Instant] given Clock {\n\t\tClock.now()\n\t}\n}\n\nservice api from http {\n\ton GET(\"/now\") () -> Effect[HttpResult[Bool]] by Visitor given Clock {\n\t\tlet t <- Clock.now()\n\t\tlet a <- Hold(\"h\").at()\n\t\tOk(t == a)\n\t}\n}\n";
        const SUITE: &str = "suite ck.holds as system {\n\tcase \"now\" {\n\t\tadvance Clock by 1.minutes\n\t\tlet r <- api.GET(\"/now\") by Visitor\n\t\texpect r is Ok(true)\n\t}\n}\n";
        let with_suite = [("ck/holds.bynk", HOLDS), ("tests/holds.bynk", SUITE)];
        let target = crate::project::BuildTarget::Workers;
        let test_build = crate::testkit::emit_project_with_tests(&with_suite, target, true);
        let deploy = crate::testkit::emit_project_with_tests(&with_suite, target, false);
        let bare = emit_workers(&[("ck/holds.bynk", HOLDS)]);
        for file in [
            "workers/ck-holds/compose.ts",
            "workers/ck-holds/handlers.ts",
        ] {
            assert!(
                test_build
                    .text(file)
                    .contains(".__bynkClock ?? new bynk__binding.ClockProvider()"),
                "{file}"
            );
            assert_eq!(deploy.text(file), bare.text(file), "{file}");
            assert!(!deploy.text(file).contains("__bynkClock"), "{file}");
        }
    }

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

    #[test]
    fn missing_bindings_are_appended_once_whatever_their_type_prefix() {
        let mut names = vec!["Ok".to_string(), "type Result".to_string()];
        append_missing_bindings(&mut names, "Ok, Err, Result, type __JsonValue, ,");
        assert_eq!(names, ["Ok", "type Result", "Err", "type __JsonValue"]);
    }

    /// A context with an `on call` service, a GET route with a path
    /// parameter, and a POST route.
    const SHOP: &str = "context demo.shop

service api from http {
  on GET(\"/items/:id\") (id: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(id)
  }
  on POST(\"/items\") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(body)
  }
}

service calc {
  on call(n: Int) -> Effect[Int] {
    Effect.pure(n)
  }
}
";

    fn shop_index() -> String {
        emit_workers(&[("demo/shop.bynk", SHOP)])
            .text("workers/demo-shop/index.ts")
            .to_string()
    }

    #[test]
    fn each_call_service_is_dispatched_under_the_internal_prefix() {
        let ts = shop_index();
        assert!(
            ts.contains("if (path.startsWith(\"/_bynk/call/\"))"),
            "{ts}"
        );
        assert!(ts.contains("case \"calc\": {"), "{ts}");
        assert!(ts.contains("await surface.calc(n)"), "{ts}");
        // An unknown internal service is a 404, not a fall-through to the routes.
        assert!(
            ts.contains("default:\n            return new globalThis.Response(\"Not found\", { status: 404 });"),
            "{ts}"
        );
    }

    #[test]
    fn a_call_whose_contract_hash_differs_is_refused_with_409() {
        let ts = shop_index();
        assert!(
            ts.contains("request.headers.get(\"X-Bynk-Contract\")"),
            "{ts}"
        );
        assert!(
            ts.contains("kind: \"ContractMismatch\", service: \"calc\""),
            "{ts}"
        );
        assert!(ts.contains("{ status: 409,"), "{ts}");
        // #1826: logged before the refusal, the sender's header bounded.
        assert!(
            ts.contains("globalThis.console.error(\"ContractMismatch demo.shop call calc\""),
            "{ts}"
        );
        assert!(
            ts.contains("actual: globalThis.String(__contract).slice(0, 16)"),
            "{ts}"
        );
    }

    #[test]
    fn a_call_argument_that_fails_to_decode_is_a_400() {
        let ts = shop_index();
        assert!(
            ts.contains("if (__r_n.tag === \"Err\") return new globalThis.Response(globalThis.JSON.stringify(__r_n.error), { status: 400,"),
            "{ts}"
        );
    }

    #[test]
    fn a_get_route_also_answers_head_with_the_body_stripped() {
        let ts = shop_index();
        assert!(
            ts.contains("if ((method === \"GET\" || method === \"HEAD\") && __m) {"),
            "{ts}"
        );
        assert!(
            ts.contains("return method === \"HEAD\" ? __headResponse(__response) : __response;"),
            "{ts}"
        );
    }

    #[test]
    fn a_post_route_with_unparseable_json_is_a_400_malformed_json() {
        let ts = shop_index();
        assert!(ts.contains("kind: \"MalformedJson\""), "{ts}");
    }

    #[test]
    fn a_known_path_with_an_undeclared_method_is_a_405_listing_the_allowed_ones() {
        let ts = shop_index();
        assert!(ts.contains("if (path === \"/items\") {"), "{ts}");
        assert!(ts.contains("headers: { allow: \"OPTIONS, POST\" }"), "{ts}");
        assert!(
            ts.contains("if (__matchPath(\"/items/:id\", path) !== null) {"),
            "{ts}"
        );
        assert!(
            ts.contains("headers: { allow: \"GET, HEAD, OPTIONS\" }"),
            "{ts}"
        );
        assert!(
            ts.contains("const __status = method === \"OPTIONS\" ? 204 : 405;"),
            "{ts}"
        );
    }

    #[test]
    fn exact_paths_are_tried_before_parameterised_ones_and_unknown_paths_are_404() {
        let ts = shop_index();
        let exact = ts.find("path === \"/items\") {").expect("exact route");
        let param = ts
            .find("__matchPath(\"/items/:id\", path);")
            .expect("param route");
        assert!(exact < param, "{ts}");
        assert!(
            ts.contains("return new globalThis.Response(\"Not Found\", { status: 404 });"),
            "{ts}"
        );
    }

    #[test]
    fn every_route_response_carries_the_service_security_headers() {
        let ts = shop_index();
        assert!(
            ts.contains(
                "const __security_api: __SecurityPolicy = { nosniff: true, hstsMaxAgeSecs: null };"
            ),
            "{ts}"
        );
        assert!(
            ts.contains("return __applySecurityHeaders(__httpResultToResponse(result, (__v: string) => __v as __JsonValue), __security_api);"),
            "{ts}"
        );
    }

    #[test]
    fn a_context_with_only_call_services_emits_no_route_table() {
        let src = "context demo.calc\n\nservice calc {\n  on call(n: Int) -> Effect[Int] {\n    Effect.pure(n)\n  }\n}\n";
        let ts = emit_workers(&[("demo/calc.bynk", src)])
            .text("workers/demo-calc/index.ts")
            .to_string();
        assert!(!ts.contains("__matchPath"), "{ts}");
        assert!(!ts.contains("__applySecurityHeaders"), "{ts}");
    }

    #[test]
    fn a_body_limit_rejects_an_oversized_request_with_413_before_reading_it() {
        let src = "context demo.up

service api from http {
  limits { maxBody: 1024 }

  on POST(\"/upload\") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(body)
  }
}
";
        let ts = emit_workers(&[("demo/up.bynk", src)])
            .text("workers/demo-up/index.ts")
            .to_string();
        let limit = ts.find("PayloadTooLarge").expect("a 413 branch");
        let read = ts.find("await __readBodyCapped").expect("the body read");
        assert!(
            limit < read,
            "the cap is checked before the body is read:\n{ts}"
        );
        assert!(ts.contains("1024"), "{ts}");
        assert!(ts.contains("status: 413"), "{ts}");
    }
}
