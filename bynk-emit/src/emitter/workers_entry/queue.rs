//! The Worker's `queue()` export: the queue routes, their dispatch and the message entries.

use super::respond::{case_, default_case, fault_detail, report_fault};
use super::wire::deserialise_call;
use crate::emitter::RuntimeUse;
use crate::ts_build::{
    as_expr, await_expr, block, call, const_, expr_stmt, ident, if_, member, method_call, return_,
    str_lit, strict_eq, switch_stmt,
};
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::TypeRef;
use bynk_ts::{TsExpr, TsObjectEntry, TsParam, TsStmt, TsSwitchCase, TsType, TsTypeMember};

/// The queue (v0.10b) and inbound-email (#3328) entry points. `queue` dispatches
/// on `batch.queue`, deserialises each message, invokes the handler, and acks on
/// `Ok` / retries on `Err` (a deserialisation failure also retries); `email`
/// hands the message to the runtime with one route per `from email` service.
pub(super) fn push_message_entries(
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
    let email_routes = super::email::collect_routes(table);
    if !email_routes.is_empty() {
        entries.push(super::email::emit_email_handler(
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
            Some(t) => deserialise_call(
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

/// One `on queue` consumer, identified by its service and per-service
/// declaration index (its `queue_<service>_<index>` method key), plus the
/// message parameter's type for wire-format deserialisation.
#[derive(Debug, Clone)]
pub(super) struct QueueRoute {
    pub(super) service: String,
    pub(super) index: usize,
    pub(super) name: String,
    pub(super) msg_type: Option<TypeRef>,
}
