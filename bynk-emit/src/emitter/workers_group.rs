//! A Worker group's `index.ts` (#3562, G2b step 2c-4b).
//!
//! A `[deploy.workers.<key>]` group is one Worker over several contexts. Each
//! member's entry is emitted as `<member>.index.ts` in *member mode*
//! (`workers_entry.rs`), which marks the router's own misses; the group's entry
//! is the Worker's `main` and dispatches to the members:
//!
//! - `fetch` tries the members in the order the manifest lists them. A marked
//!   response is a miss and the next member is tried; the first unmarked one is
//!   the answer (a handler's own `404` included). If every member misses, the
//!   answer is the first marked response that is not a `404` (the synthesised
//!   `405`/`204`), with the `Allow` of all such responses merged, since two
//!   members may serve one path under different methods; else the single-context
//!   `Not Found`.
//! - `scheduled` runs every member whose cron matches, `queue` goes to the member
//!   consuming `batch.queue`, `email` to the member claiming the recipient.
//! - Agent, events fan-out and container classes are re-exported by name (a
//!   member entry exports `isMiss`, so `export *` over members would collide).
//!
//! A request to the internal `/_bynk/call`, `/_bynk/event`, `/_bynk/migrate` or
//! (in a `bynk dev` build only) `/_bynk/inspect` prefix that no member claims is answered as a standalone Worker answers it
//! unauthenticated: the same method gate, then the same `413` or `401` the
//! runtime's `verifyCall` gives. Answering `404` for an unknown service while a
//! known one answers `401` would let an unsigned caller list a group's services.

use crate::ts_build::{
    arrow_of as arrow, binary, block, call, const_typed, expr_stmt, ident, if_, index, member,
    method_call, new as new_, num_lit, param, paren, ret, str_lit,
};
use bynk_check::wire_format as wf;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsProgram, TsStmt,
    TsSwitchCase, TsType, TsTypeMember, TsUnaryOp,
};

/// What the group's entry needs to know of one member.
pub struct GroupMember {
    /// The member's qualified context name.
    pub context: String,
    /// The import specifier of its `<member>.index.ts` (`./shop-orders.index.js`).
    pub specifier: String,
    /// Its cron expressions (its entry has `scheduled` iff there are any).
    pub crons: Vec<String>,
    /// The queues it consumes (its entry has `queue` iff there are any).
    pub queues: Vec<String>,
    /// The addresses its `from email` services claim (its entry has `email` iff
    /// there are any).
    pub addresses: Vec<String>,
    /// The Durable Object classes its entry exports: its agents. (The events
    /// fan-out is the group's, not a member's.)
    pub classes: Vec<String>,
}

/// The group as a whole.
pub struct GroupEntry<'a> {
    pub key: &'a str,
    pub members: &'a [GroupMember],
    /// The container classes of `containers.ts`, exported once for the Worker.
    pub container_classes: &'a [String],
    /// Whether any member publishes events: the Worker exports the one events
    /// fan-out class, from `events_fanout.ts` (#3562, G3-1).
    pub has_fanout: bool,
    /// Whether any member subscribes to events (answers `/_bynk/event/`).
    pub has_events: bool,
    /// Whether any member hosts a migratable agent (answers `/_bynk/migrate/`).
    pub has_migrate: bool,
    /// Whether this is a `bynk dev` build with agents to inspect (answers
    /// `/_bynk/inspect/`). A deploy build has no such route, in the group or a
    /// member, and the entry says nothing of it.
    pub has_inspect: bool,
}

// The node builders of this file. Each is a one-line wrapper over a `bynk_ts`
// constructor, so the entry below reads as the TypeScript it emits.

/// `globalThis.<name>`.
fn global(name: &str) -> TsExpr {
    member(ident("globalThis"), name)
}

fn as_never(expr: TsExpr) -> TsExpr {
    TsExpr::As {
        expr: Box::new(expr),
        ty: ty("never"),
    }
}

/// `(p) => body`, an expression-bodied arrow over untyped parameters.
fn lambda(params: &[&str], body: TsExpr) -> TsExpr {
    arrow(
        params.iter().map(|p| TsParam::new(*p, None)).collect(),
        false,
        TsArrowBody::Expr(Box::new(body)),
    )
}

fn ty(name: &str) -> TsType {
    TsType::named(name)
}

/// `Promise<T>`, as the handlers write it.
fn promise(inner: TsType) -> TsType {
    TsType::named_with_args("Promise", vec![inner])
}

fn response_ty() -> TsType {
    ty("globalThis.Response")
}

/// `stmt`, with no blank line before it: this entry keeps its re-exports, its handler
/// types and its member constants in tight runs, as it always wrote them.
fn tight(mut stmt: TsStmt) -> TsStmt {
    stmt.no_blank_before = true;
    stmt
}

/// `if (cond) { body }` on one line: the braced form of the one-line guards
/// this entry used to write without braces.
fn guard(cond: TsExpr, body: TsStmt) -> TsStmt {
    if_(cond, TsStmt::inline_block(vec![body], None))
}

/// `new globalThis.Response(body, init)`.
fn new_response(body: TsExpr, init: TsExpr) -> TsExpr {
    new_(global("Response"), vec![body, init])
}

/// `{ status: <n> }`.
fn status(n: u32) -> TsExpr {
    TsExpr::object(vec![("status".to_string(), num_lit(n))])
}

fn function(
    name: &str,
    is_async: bool,
    params: Vec<TsParam>,
    return_type: TsType,
    body: Vec<TsStmt>,
) -> TsStmt {
    TsStmt::decl(
        TsDecl::Function {
            name: name.to_string(),
            generics: Vec::new(),
            params,
            return_type: Some(return_type),
            body,
            is_async,
            inline: false,
        },
        None,
    )
}

/// `["a", "b"]`, as a TypeScript array literal of strings.
fn array(items: &[String]) -> TsExpr {
    TsExpr::array(items.iter().map(str_lit).collect())
}

/// `await __verifyCall(request, env, "")`.
fn verify_call() -> TsExpr {
    TsExpr::Await(Box::new(call(
        ident("__verifyCall"),
        vec![ident("request"), ident("env"), str_lit("")],
    )))
}

/// `if (request.method !== <method>) return new Response(<body>, { status: <n>,
/// <headers> });`, the method gate every internal prefix starts with.
fn method_gate(method: &str, body: TsExpr, status_code: u32, headers: Option<TsExpr>) -> TsStmt {
    let mut init = vec![("status".to_string(), num_lit(status_code))];
    if let Some(headers) = headers {
        init.push(("headers".to_string(), headers));
    }
    guard(
        binary(
            TsBinaryOp::StrictNotEq,
            member(ident("request"), "method"),
            str_lit(method),
        ),
        ret(new_response(body, TsExpr::object(init))),
    )
}

/// `if (path.startsWith(<prefix>)) { <body> }`.
fn prefix_branch(prefix: &str, mut body: Vec<TsStmt>) -> TsStmt {
    body.push(ret(call(ident("__refused"), vec![verify_call()])));
    TsStmt::if_stmt(
        method_call(ident("path"), "startsWith", vec![str_lit(prefix)]),
        block(body),
        None,
    )
}

/// The unclaimed-request fallback: what a standalone Worker answers to an
/// unauthenticated request for an internal prefix (see the module doc).
fn unclaimed(entry: &GroupEntry<'_>) -> TsStmt {
    let allow = |method: &str| TsExpr::object(vec![("allow".to_string(), str_lit(method))]);
    let mut body = vec![const_typed(
        "path",
        None,
        member(
            new_(global("URL"), vec![member(ident("request"), "url")]),
            "pathname",
        ),
    )];
    // Every Worker answers `/_bynk/call/`.
    body.push(prefix_branch(
        wf::CALL_PATH_PREFIX,
        vec![method_gate(
            wf::CALL_METHOD,
            TsExpr::Lit(TsLit::Null),
            405,
            Some(allow(wf::CALL_METHOD)),
        )],
    ));
    if entry.has_events {
        body.push(guard(
            method_call(ident("path"), "startsWith", vec![str_lit("/_bynk/event/")]),
            ret(call(ident("__refused"), vec![verify_call()])),
        ));
    }
    let caller_gate = |caller: &str, body: &str| {
        guard(
            binary(
                TsBinaryOp::StrictNotEq,
                method_call(
                    member(ident("request"), "headers"),
                    "get",
                    vec![str_lit(wf::CALLER_HEADER)],
                ),
                str_lit(caller),
            ),
            ret(new_response(str_lit(body), status(401))),
        )
    };
    if entry.has_migrate {
        let key_missing = binary(
            TsBinaryOp::StrictNotEq,
            TsExpr::Unary {
                op: TsUnaryOp::Typeof,
                expr: Box::new(index(
                    paren(TsExpr::As {
                        expr: Box::new(TsExpr::As {
                            expr: Box::new(ident("env")),
                            ty: ty("unknown"),
                        }),
                        ty: TsType::named_with_args(
                            "globalThis.Record",
                            vec![ty("string"), ty("unknown")],
                        ),
                    }),
                    str_lit(wf::MIGRATE_KEY),
                )),
            },
            str_lit("string"),
        );
        body.push(prefix_branch(
            wf::MIGRATE_PATH_PREFIX,
            vec![
                guard(
                    key_missing,
                    ret(new_response(str_lit("Not found"), status(404))),
                ),
                method_gate(
                    wf::CALL_METHOD,
                    TsExpr::Lit(TsLit::Null),
                    405,
                    Some(allow(wf::CALL_METHOD)),
                ),
                caller_gate(wf::MIGRATE_CALLER, "Unauthorized"),
            ],
        ));
    }
    if entry.has_inspect {
        body.push(prefix_branch(
            super::workers_entry::INSPECT_PREFIX,
            vec![
                method_gate("POST", str_lit("Method Not Allowed"), 405, None),
                caller_gate("bynk.dev", "Unauthorized"),
            ],
        ));
    }
    body.push(ret(new_response(str_lit("Not Found"), status(404))));
    function(
        "__unclaimed",
        true,
        vec![
            param("request", ty("globalThis.Request")),
            param("env", ty("unknown")),
        ],
        promise(response_ty()),
        body,
    )
}

/// `fetch`: the members in order (see the module doc). Every marked response
/// that is not a `404` is kept, so that if no member owns the request the answer
/// lists the methods the whole Worker serves on the path, not one member's.
fn fetch_method() -> TsObjectEntry {
    TsObjectEntry::method(
        "fetch",
        true,
        vec![
            param("request", ty("globalThis.Request")),
            param("env", ty("unknown")),
            param("ctx", ty("__Ctx")),
        ],
        Some(promise(response_ty())),
        vec![ret(call(
            ident("__fetch"),
            vec![
                ident("request"),
                call(ident("__loopback"), vec![ident("env"), ident("ctx")]),
                ident("ctx"),
            ],
        ))],
    )
}

/// The dispatcher: members in manifest order, the first answer that is not a
/// router's own miss wins. `env` already answers a call to a member in process
/// ([`loopback`]).
fn dispatch() -> TsStmt {
    let held_empty = binary(
        TsBinaryOp::StrictEq,
        member(ident("held"), "length"),
        num_lit(0),
    );
    let each_member = TsStmt::for_of(
        "member",
        ident("__members"),
        block(vec![
            const_typed(
                "response",
                None,
                TsExpr::Await(Box::new(method_call(
                    member(ident("member"), "entry"),
                    "fetch",
                    vec![ident("request"), as_never(ident("env")), ident("ctx")],
                ))),
            ),
            guard(
                TsExpr::Unary {
                    op: TsUnaryOp::Not,
                    expr: Box::new(method_call(
                        ident("member"),
                        "isMiss",
                        vec![ident("response")],
                    )),
                },
                ret(ident("response")),
            ),
            guard(
                binary(
                    TsBinaryOp::StrictNotEq,
                    member(ident("response"), "status"),
                    num_lit(404),
                ),
                expr_stmt(method_call(ident("held"), "push", vec![ident("response")])),
            ),
        ]),
        None,
    );
    function(
        "__fetch",
        true,
        vec![
            param("request", ty("globalThis.Request")),
            param("env", ty("unknown")),
            param("ctx", ty("__Ctx")),
        ],
        promise(response_ty()),
        vec![
            const_typed(
                "held",
                Some(TsType::array(response_ty())),
                TsExpr::array(vec![]),
            ),
            each_member,
            ret(TsExpr::Conditional {
                test: Box::new(held_empty),
                consequent: Box::new(TsExpr::Await(Box::new(call(
                    ident("__unclaimed"),
                    vec![ident("request"), ident("env")],
                )))),
                alternate: Box::new(call(ident("__withAllowed"), vec![ident("held")])),
            }),
        ],
    )
}

/// A call from one member to another is a call from the Worker to itself, so it
/// need not leave the Worker (#3562, G3-2): the binding a member names the other by
/// answers in process, by running the request through the group's own dispatcher.
/// It is the same request, signed, contract-checked, admitted and decoded as one
/// over a Service Binding, so a value is copied across it, a callee's fault reaches
/// the caller as the same `CalleeFaulted`, and a refusal is the same refusal; only
/// the hop is gone. Anything the callee's `fetch` throws is the plain `500` a Service
/// Binding would hand the caller (which maps it to `Transport`), never the callee's
/// own exception. (The events fan-out's Durable Object runs elsewhere and keeps
/// the Service Binding.)
fn loopback(entry: &GroupEntry<'_>) -> Vec<TsStmt> {
    if entry.members.len() < 2 {
        return vec![function(
            "__loopback",
            false,
            vec![param("env", ty("unknown")), param("_ctx", ty("__Ctx"))],
            ty("unknown"),
            vec![ret(ident("env"))],
        )];
    }
    let names: Vec<String> = entry
        .members
        .iter()
        .map(|m| crate::emitter::bindings::service_binding(&m.context).name)
        .collect();
    let record = || TsType::named_with_args("globalThis.Record", vec![ty("string"), ty("unknown")]);
    let answers_in_process = arrow(
        vec![param("request", ty("globalThis.Request"))],
        true,
        TsArrowBody::Multiline(vec![TsStmt::try_catch(
            block(vec![ret(TsExpr::Await(Box::new(call(
                ident("__fetch"),
                vec![ident("request"), ident("inner"), ident("ctx")],
            ))))]),
            None::<String>,
            block(vec![ret(new_response(
                str_lit("Internal Server Error"),
                status(500),
            ))]),
            None,
        )]),
    );
    vec![
        const_typed(
            "__memberBindings",
            Some(TsType::readonly_array(ty("string"))),
            array(&names),
        ),
        function(
            "__loopback",
            false,
            vec![param("env", ty("unknown")), param("ctx", ty("__Ctx"))],
            ty("unknown"),
            vec![
                const_typed(
                    "inner",
                    Some(record()),
                    TsExpr::object_entries(vec![TsObjectEntry::Spread(paren(TsExpr::As {
                        expr: Box::new(ident("env")),
                        ty: record(),
                    }))]),
                ),
                TsStmt::for_of(
                    "name",
                    ident("__memberBindings"),
                    block(vec![TsStmt::assign(
                        index(ident("inner"), ident("name")),
                        TsExpr::object(vec![("fetch".to_string(), answers_in_process)]),
                        None,
                    )]),
                    None,
                ),
                ret(ident("inner")),
            ],
        ),
    ]
}

/// `__refused`: a verification failure becomes the `401`, a response stays.
fn refused() -> TsStmt {
    function(
        "__refused",
        false,
        vec![param(
            "verified",
            TsType::union(vec![ty("string"), response_ty()]),
        )],
        response_ty(),
        vec![ret(TsExpr::Conditional {
            test: Box::new(binary(
                TsBinaryOp::StrictEq,
                TsExpr::Unary {
                    op: TsUnaryOp::Typeof,
                    expr: Box::new(ident("verified")),
                },
                str_lit("string"),
            )),
            consequent: Box::new(new_response(str_lit("Unauthorized"), status(401))),
            alternate: Box::new(ident("verified")),
        })],
    )
}

/// The first held `405`/`204`, answering with the union of every member's
/// `Allow` (RFC 9110 §10.2.1: the methods the target resource supports).
fn with_allowed() -> TsStmt {
    let trimmed = method_call(
        method_call(
            paren(binary(
                TsBinaryOp::NullishCoalescing,
                method_call(member(ident("r"), "headers"), "get", vec![str_lit("allow")]),
                str_lit(""),
            )),
            "split",
            vec![str_lit(",")],
        ),
        "map",
        vec![lambda(&["m"], method_call(ident("m"), "trim", vec![]))],
    );
    let methods = method_call(
        ident("held"),
        "flatMap",
        vec![lambda(
            &["r"],
            method_call(
                trimmed,
                "filter",
                vec![lambda(
                    &["m"],
                    binary(TsBinaryOp::StrictNotEq, ident("m"), str_lit("")),
                )],
            ),
        )],
    );
    let unique_sorted = method_call(
        method_call(
            TsExpr::array(vec![TsExpr::Spread(Box::new(new_(
                global("Set"),
                vec![ident("methods")],
            )))]),
            "sort",
            vec![],
        ),
        "join",
        vec![str_lit(", ")],
    );
    function(
        "__withAllowed",
        false,
        vec![param("held", TsType::readonly_array(response_ty()))],
        response_ty(),
        vec![
            const_typed("first", None, index(ident("held"), num_lit(0))),
            guard(
                binary(
                    TsBinaryOp::StrictEq,
                    member(ident("held"), "length"),
                    num_lit(1),
                ),
                ret(ident("first")),
            ),
            const_typed("methods", None, methods),
            const_typed(
                "merged",
                None,
                new_response(member(ident("first"), "body"), ident("first")),
            ),
            expr_stmt(method_call(
                member(ident("merged"), "headers"),
                "set",
                vec![str_lit("allow"), unique_sorted],
            )),
            ret(ident("merged")),
        ],
    )
}

/// `{ readonly <name>: string }`.
fn readonly_string(name: &str) -> TsType {
    TsType::Object(vec![TsTypeMember::readonly_prop(name, ty("string"))])
}

/// `<m>.<handler>(<arg> as never, __loopback(env, ctx) as never, ctx)`: a member's
/// handler, run on the loopback env.
fn run_member(member_index: usize, handler: &str, arg: &str) -> TsExpr {
    method_call(
        ident(format!("__m{member_index}")),
        handler,
        vec![
            as_never(ident(arg)),
            as_never(call(ident("__loopback"), vec![ident("env"), ident("ctx")])),
            ident("ctx"),
        ],
    )
}

/// `scheduled`: every member whose cron matches runs.
fn scheduled(members: &[(usize, &GroupMember)]) -> TsObjectEntry {
    let runs: Vec<TsExpr> = members
        .iter()
        .filter(|(_, m)| !m.crons.is_empty())
        .map(|(i, m)| {
            TsExpr::Spread(Box::new(paren(TsExpr::Conditional {
                test: Box::new(method_call(
                    array(&m.crons),
                    "includes",
                    vec![member(ident("event"), "cron")],
                )),
                consequent: Box::new(TsExpr::array(vec![run_member(*i, "scheduled", "event")])),
                alternate: Box::new(TsExpr::array(vec![])),
            })))
        })
        .collect();
    TsObjectEntry::method(
        "scheduled",
        true,
        vec![
            param("event", readonly_string("cron")),
            param("env", ty("unknown")),
            param("ctx", ty("__Ctx")),
        ],
        Some(promise(ty("void"))),
        vec![expr_stmt(TsExpr::Await(Box::new(method_call(
            global("Promise"),
            "all",
            vec![TsExpr::multiline_array(runs)],
        ))))],
    )
}

/// `queue`: the member consuming `batch.queue`.
fn queue(members: &[(usize, &GroupMember)]) -> TsObjectEntry {
    let mut cases: Vec<TsSwitchCase> = members
        .iter()
        .filter(|(_, m)| !m.queues.is_empty())
        .map(|(i, m)| TsSwitchCase {
            labels: m.queues.iter().map(str_lit).collect(),
            body: vec![ret(run_member(*i, "queue", "batch"))],
            default_braced: false,
            case_braced: false,
            span: None,
        })
        .collect();
    cases.push(TsSwitchCase {
        labels: Vec::new(),
        body: vec![TsStmt::return_stmt(None, None)],
        default_braced: false,
        case_braced: true,
        span: None,
    });
    TsObjectEntry::method(
        "queue",
        true,
        vec![
            param("batch", readonly_string("queue")),
            param("env", ty("unknown")),
            param("ctx", ty("__Ctx")),
        ],
        Some(promise(ty("void"))),
        vec![TsStmt::switch_stmt(
            member(ident("batch"), "queue"),
            cases,
            None,
        )],
    )
}

/// `email`: the member claiming the recipient, by the runtime's own rule (an
/// exact address, case-insensitively, before a `*@domain` catch-all). With no
/// claimant the first email member gets it, and its runtime rejects an unknown
/// recipient.
fn email(members: &[(usize, &GroupMember)]) -> TsObjectEntry {
    let routes: Vec<TsExpr> = members
        .iter()
        .filter(|(_, m)| !m.addresses.is_empty())
        .flat_map(|(i, m)| {
            m.addresses.iter().map(move |a| {
                TsExpr::object(vec![
                    ("address".to_string(), str_lit(a.to_lowercase())),
                    ("member".to_string(), ident(format!("__m{i}"))),
                ])
            })
        })
        .collect();
    let first = members
        .iter()
        .find(|(_, m)| !m.addresses.is_empty())
        .map_or(0, |(i, _)| *i);
    let routes_ty = TsType::readonly_array(TsType::Object(vec![
        TsTypeMember::readonly_prop("address", ty("string")),
        TsTypeMember::readonly_prop("member", ty("__Email")),
    ]));
    let by_address = |address: TsExpr| {
        method_call(
            ident("routes"),
            "find",
            vec![lambda(
                &["r"],
                binary(TsBinaryOp::StrictEq, member(ident("r"), "address"), address),
            )],
        )
    };
    let claimant = binary(
        TsBinaryOp::NullishCoalescing,
        by_address(ident("to")),
        by_address(TsExpr::template_lit(
            vec!["*@".to_string(), String::new()],
            vec![ident("domain")],
        )),
    );
    let claiming = paren(binary(
        TsBinaryOp::NullishCoalescing,
        TsExpr::OptionalMember {
            object: Box::new(ident("claimant")),
            property: "member".to_string(),
        },
        ident(format!("__m{first}")),
    ));
    TsObjectEntry::method(
        "email",
        true,
        vec![
            param("message", readonly_string("to")),
            param("env", ty("unknown")),
            param("ctx", ty("__Ctx")),
        ],
        Some(promise(ty("void"))),
        vec![
            const_typed(
                "to",
                None,
                method_call(member(ident("message"), "to"), "toLowerCase", vec![]),
            ),
            const_typed(
                "domain",
                None,
                method_call(
                    ident("to"),
                    "slice",
                    vec![binary(
                        TsBinaryOp::Add,
                        method_call(ident("to"), "lastIndexOf", vec![str_lit("@")]),
                        num_lit(1),
                    )],
                ),
            ),
            const_typed("routes", Some(routes_ty), TsExpr::multiline_array(routes)),
            const_typed("claimant", None, claimant),
            expr_stmt(TsExpr::Await(Box::new(method_call(
                claiming,
                "email",
                vec![
                    as_never(ident("message")),
                    as_never(call(ident("__loopback"), vec![ident("env"), ident("ctx")])),
                    ident("ctx"),
                ],
            )))),
        ],
    )
}

/// `type <name> = { <method>(<arg>: never, env: never, ctx: __Ctx): Promise<<ret>> };`:
/// the slice of a member's default export the group calls.
fn handler_type(name: &str, method: &str, arg: &str, ret_ty: TsType) -> TsStmt {
    tight(TsStmt::decl(
        TsDecl::TypeAlias {
            name: name.to_string(),
            type_params: Vec::new(),
            ty: TsType::Object(vec![TsTypeMember::method(
                method,
                vec![
                    param(arg, ty("never")),
                    param("env", ty("never")),
                    param("ctx", ty("__Ctx")),
                ],
                promise(ret_ty),
            )]),
        },
        None,
    ))
}

/// The member imports and the class re-exports.
fn imports_and_exports(entry: &GroupEntry<'_>, members: &[(usize, &GroupMember)]) -> Vec<TsStmt> {
    let mut stmts = Vec::new();
    for (i, m) in members {
        stmts.push(TsStmt::decl(
            TsDecl::ImportDefault {
                alias: format!("__entry{i}"),
                named: vec![format!("isMiss as __miss{i}")],
                from: m.specifier.clone(),
            },
            None,
        ));
    }
    let re_export = |names: Vec<String>, from: &str| {
        tight(TsStmt::decl(
            TsDecl::ReExport {
                names,
                from: from.to_string(),
            },
            None,
        ))
    };
    for (_, m) in members {
        if !m.classes.is_empty() {
            stmts.push(re_export(m.classes.clone(), &m.specifier));
        }
    }
    if entry.has_fanout {
        stmts.push(re_export(
            vec![crate::emitter::wrangler::EVENTS_FANOUT_CLASS_NAME.to_string()],
            "./events_fanout.js",
        ));
    }
    if !entry.container_classes.is_empty() {
        stmts.push(re_export(
            entry.container_classes.to_vec(),
            "./containers.js",
        ));
    }
    stmts
}

/// The handler types, each member's typed view of its entry, and the member table.
fn member_table(members: &[(usize, &GroupMember)]) -> Vec<TsStmt> {
    let has = |f: &dyn Fn(&GroupMember) -> bool| members.iter().any(|(_, m)| f(m));
    let mut stmts = vec![
        TsStmt::decl(
            TsDecl::TypeAlias {
                name: "__Ctx".to_string(),
                type_params: Vec::new(),
                ty: TsType::Object(vec![TsTypeMember::method(
                    "waitUntil",
                    vec![param(
                        "promise",
                        TsType::named_with_args("globalThis.Promise", vec![ty("unknown")]),
                    )],
                    ty("void"),
                )]),
            },
            None,
        ),
        tight(TsStmt::decl(
            TsDecl::TypeAlias {
                name: "__Fetch".to_string(),
                type_params: Vec::new(),
                ty: TsType::Object(vec![TsTypeMember::method(
                    "fetch",
                    vec![
                        param("request", ty("globalThis.Request")),
                        param("env", ty("never")),
                        param("ctx", ty("__Ctx")),
                    ],
                    promise(response_ty()),
                )]),
            },
            None,
        )),
    ];
    if has(&|m| !m.crons.is_empty()) {
        stmts.push(handler_type(
            "__Scheduled",
            "scheduled",
            "event",
            ty("void"),
        ));
    }
    if has(&|m| !m.queues.is_empty()) {
        stmts.push(handler_type("__Queue", "queue", "batch", ty("void")));
    }
    if has(&|m| !m.addresses.is_empty()) {
        stmts.push(handler_type("__Email", "email", "message", ty("void")));
    }
    for (i, m) in members {
        let mut handlers = vec![ty("__Fetch")];
        if !m.crons.is_empty() {
            handlers.push(ty("__Scheduled"));
        }
        if !m.queues.is_empty() {
            handlers.push(ty("__Queue"));
        }
        if !m.addresses.is_empty() {
            handlers.push(ty("__Email"));
        }
        stmts.push(tight(const_typed(
            format!("__m{i}"),
            Some(TsType::intersection(handlers)),
            ident(format!("__entry{i}")),
        )));
    }
    let row_ty = TsType::readonly_array(TsType::Object(vec![
        TsTypeMember::readonly_prop("entry", ty("__Fetch")),
        TsTypeMember::readonly_prop(
            "isMiss",
            TsType::Fn {
                rest: false,
                params: vec![response_ty()],
                names: vec!["response".to_string()],
                ret: Box::new(ty("boolean")),
            },
        ),
    ]));
    let rows = members
        .iter()
        .map(|(i, _)| {
            TsExpr::object(vec![
                ("entry".to_string(), ident(format!("__m{i}"))),
                ("isMiss".to_string(), ident(format!("__miss{i}"))),
            ])
        })
        .collect();
    stmts.push(const_typed(
        "__members",
        Some(row_ty),
        TsExpr::multiline_array(rows),
    ));
    stmts
}

/// The group's `index.ts`.
pub fn emit_group_entry(entry: &GroupEntry<'_>) -> TsProgram {
    let mut program = TsProgram::new();
    program.push(TsStmt::comment(
        "Generated by bynk — do not edit by hand.",
        None,
    ));
    let names: Vec<&str> = entry.members.iter().map(|m| m.context.as_str()).collect();
    program.push(TsStmt::comment(
        format!(
            "Worker entry point for group `{}`: members {} (tried in this order).",
            entry.key,
            names.join(", ")
        ),
        None,
    ));
    let members: Vec<(usize, &GroupMember)> = entry.members.iter().enumerate().collect();
    let has = |f: &dyn Fn(&GroupMember) -> bool| members.iter().any(|(_, m)| f(m));
    let (has_cron, has_queue, has_email) = (
        has(&|m| !m.crons.is_empty()),
        has(&|m| !m.queues.is_empty()),
        has(&|m| !m.addresses.is_empty()),
    );
    // The runtime import is a tree node, so the build's runtime-module pass
    // finds it, as it does a member entry's.
    for stmt in crate::emitter::runtime_files::runtime_import_stmts(
        false,
        &["__verifyCall".to_string()],
        "../../runtime.js",
    ) {
        program.push(stmt);
    }
    for stmt in imports_and_exports(entry, &members) {
        program.push(stmt);
    }
    for stmt in member_table(&members) {
        program.push(stmt);
    }
    program.push(refused());
    program.push(with_allowed());
    program.push(unclaimed(entry));
    program.push(dispatch());
    for stmt in loopback(entry) {
        program.push(stmt);
    }
    let mut handlers = vec![fetch_method()];
    if has_cron {
        handlers.push(scheduled(&members));
    }
    if has_queue {
        handlers.push(queue(&members));
    }
    if has_email {
        handlers.push(email(&members));
    }
    program.push(TsStmt::decl(
        TsDecl::ExportDefault(TsExpr::multiline_object_entries(handlers)),
        None,
    ));
    program
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(context: &str) -> GroupMember {
        GroupMember {
            context: context.into(),
            specifier: format!("./{context}.index.js"),
            crons: vec!["* * * * *".into()],
            queues: vec!["jobs".into()],
            addresses: vec!["in@example.com".into()],
            classes: vec![],
        }
    }

    fn printed(entry: &GroupEntry<'_>) -> String {
        bynk_ts::print(&emit_group_entry(entry), "x.bynk", "", "index.ts").text
    }

    fn entry_of(members: &[GroupMember]) -> GroupEntry<'_> {
        GroupEntry {
            key: "core",
            members,
            container_classes: &[],
            has_fanout: false,
            has_events: false,
            has_migrate: false,
            has_inspect: false,
        }
    }

    /// #3562, G3-2: a handler of one member reaching another goes in process, so
    /// each of `scheduled`, `queue` and `email` runs its member on the loopback env.
    #[test]
    fn every_handler_runs_its_member_on_the_loopback_env() {
        let members = [member("app.a"), member("app.b")];
        let text = printed(&entry_of(&members));
        for handler in ["scheduled", "queue", "email"] {
            let at = text
                .find(&format!("async {handler}("))
                .unwrap_or_else(|| panic!("no {handler} handler:\n{text}"));
            let body = &text[at..];
            assert!(
                body.contains("__loopback(env, ctx) as never"),
                "{handler} does not pass the loopback env:\n{text}"
            );
            assert!(
                !body.contains("(env as never"),
                "{handler} passes the raw env:\n{text}"
            );
        }
    }

    /// A callee that throws out of `fetch` is a plain `500` to its caller, as over
    /// a Service Binding, not the callee's exception.
    #[test]
    fn a_callee_that_throws_is_a_plain_500() {
        let members = [member("app.a"), member("app.b")];
        let text = printed(&entry_of(&members));
        assert!(text.contains("} catch {"), "{text}");
        assert!(text.contains("status: 500"), "{text}");
    }

    /// A single-member group has nothing to loop back to: `__loopback` is the identity,
    /// and no member-binding table is built.
    #[test]
    fn a_lone_member_has_an_identity_loopback() {
        let members = [member("app.a")];
        let text = printed(&entry_of(&members));
        assert!(
            text.contains(
                "function __loopback(env: unknown, _ctx: __Ctx): unknown {\n  return env;\n}"
            ),
            "{text}"
        );
        assert!(!text.contains("__memberBindings"), "{text}");
    }

    /// The member imports bind the default export and `isMiss` together, and every
    /// string the entry writes (specifiers, crons, queues, addresses) is escaped as
    /// a TypeScript string literal.
    #[test]
    fn member_imports_pair_the_entry_with_its_miss_marker() {
        let members = [member("app.a"), member("app.b")];
        let text = printed(&entry_of(&members));
        assert!(
            text.contains("import __entry0, { isMiss as __miss0 } from \"./app.a.index.js\";"),
            "{text}"
        );
        assert!(
            text.contains("import __entry1, { isMiss as __miss1 } from \"./app.b.index.js\";"),
            "{text}"
        );
        assert!(text.contains("{ entry: __m1, isMiss: __miss1 },"), "{text}");
    }

    /// Queues that share a member stack their `case` labels over one body, and the
    /// catch-all `default` follows.
    #[test]
    fn a_members_queues_share_one_case_body() {
        let mut a = member("app.a");
        a.queues = vec!["jobs".into(), "mail".into()];
        let members = [a];
        let text = printed(&entry_of(&members));
        assert!(
            text.contains(
                "      case \"jobs\":\n      case \"mail\":\n        return __m0.queue(batch as never, __loopback(env, ctx) as never, ctx);\n      default:\n        return;"
            ),
            "{text}"
        );
    }

    /// The internal prefixes a group answers unauthenticated depend on what its
    /// members host: `/_bynk/migrate/` and `/_bynk/inspect/` only when asked for.
    #[test]
    fn the_unclaimed_fallback_answers_only_the_prefixes_the_group_serves() {
        let members = [member("app.a")];
        let plain = printed(&entry_of(&members));
        assert!(plain.contains("async function __unclaimed("), "{plain}");
        assert!(!plain.contains("/_bynk/migrate/"), "{plain}");
        assert!(!plain.contains("/_bynk/inspect/"), "{plain}");
        let mut entry = entry_of(&members);
        entry.has_migrate = true;
        entry.has_inspect = true;
        let full = printed(&entry);
        assert!(full.contains("/_bynk/migrate/"), "{full}");
        assert!(full.contains("/_bynk/inspect/"), "{full}");
    }

    /// The entry binds every name it uses: the member imports, the handler types
    /// and the runtime's `__verifyCall` account for all of them, in every shape of
    /// group (a lone member, several; fan-out, events, migrate, inspect).
    #[test]
    fn the_entry_binds_every_name_it_uses() {
        let sets = [
            vec![member("app.a")],
            vec![member("app.a"), member("app.b"), member("app.c")],
        ];
        for members in &sets {
            for bits in 0..16u32 {
                let mut entry = entry_of(members);
                entry.has_fanout = bits & 1 != 0;
                entry.has_events = bits & 2 != 0;
                entry.has_migrate = bits & 4 != 0;
                entry.has_inspect = bits & 8 != 0;
                let unbound = bynk_ts::unbound_names(&emit_group_entry(&entry));
                assert!(
                    unbound.is_empty(),
                    "{} members, {bits:#06b}: {unbound:?}",
                    members.len()
                );
            }
        }
    }
}
