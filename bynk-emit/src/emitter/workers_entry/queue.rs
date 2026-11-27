//! The Worker's `queue()` export: the queue routes, their dispatch and the message entries.

use super::call::caller_predicate_expr;
use super::respond::{case_, default_case, fault_detail, report_fault};
use super::wire::deserialise_call;
use crate::emitter::RuntimeUse;
use crate::ts_build::{
    arrow, as_expr, assign, await_expr, block, bool_lit, call, cond_expr, const_, const_typed,
    expr_stmt, ident, if_, member, method_call, not_expr, or_expr, paren, return_, str_lit,
    strict_eq, switch_stmt,
};
use bynk_check::symbols::UnitTable;
use bynk_syntax::ast::{ServiceProtocol, TypeRef};
use bynk_ts::{TsExpr, TsObjectEntry, TsParam, TsStmt, TsSwitchCase, TsType, TsTypeMember};

/// The queue (v0.10b) and inbound-email (#3328) entry points. `queue` dispatches
/// on `batch.queue`, deserialises each message, invokes the handler, and acks on
/// `Ok` / retries on `Err` (a deserialisation failure also retries); `email`
/// hands the message to the runtime with one route per `from email` service.
pub(super) fn push_message_entries(
    (table, layout, context, event_owners): (
        &UnitTable,
        &bynk_project::worker_layout::WorkerLayout,
        &str,
        &std::collections::BTreeMap<String, String>,
    ),
    queue_routes: &[QueueRoute],
    exec_params: &[TsParam],
    compose_args: &[TsExpr],
    runtime_use: &RuntimeUse,
    entries: &mut Vec<TsObjectEntry>,
) {
    // #3898 (3b): a subscriber's event queues are consumed by the same `queue` entry.
    let event_routes =
        super::event_queue::collect_event_queue_routes(context, table, event_owners);
    if !queue_routes.is_empty() || !event_routes.is_empty() {
        entries.push(emit_queue_handler(
            queue_routes,
            &event_routes,
            exec_params,
            compose_args,
            runtime_use,
        ));
    }
    let email_routes = super::email::collect_routes(table, layout);
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
    event_routes: &[super::event_queue::EventQueueRoute],
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
                        TsTypeMember::readonly_prop("attempts", TsType::named("number")),
                        TsTypeMember::readonly_prop("timestamp", TsType::named("globalThis.Date")),
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
    body.extend(super::event_queue::event_queue_dispatch(event_routes, ru));
    let mut cases: Vec<TsSwitchCase> = Vec::new();
    for route in queue_routes {
        let method_key = crate::emitter::queue_handler_method_name(&route.service, route.index);
        // #3899: a signed consumer decodes the body inside the envelope its admission unwrapped.
        let body = member(
            ident(if route.signed.is_some() { "__a" } else { "msg" }),
            "body",
        );
        let dser = match &route.msg_type {
            Some(t) => deserialise_call(
                t,
                &TsExpr::Paren(Box::new(as_expr(
                    body.clone(),
                    TsType::named("__JsonValue"),
                ))),
                "$",
                ru,
            ),
            // P7.2: `msg.body` is already declared `unknown` at `queue()`'s own
            // signature above — no cast needed at all when there's no declared type
            // to (dis)trust it against.
            None => as_expr(
                call(ident("Ok"), vec![body]),
                TsType::named("Result<unknown, __BoundaryError>"),
            ),
        };
        let mut try_stmts = admission_stmts(route);
        try_stmts.extend([
            const_("__r", dser),
            // #3878: an undecodable body is never retried (no retry changes the
            // bytes): it is dead-lettered or dropped visibly, then skipped.
            if_(
                strict_eq(member(ident("__r"), "tag"), str_lit("Err")),
                TsStmt::inline_block(
                    [expr_stmt(await_expr(undecodable_call(route)))]
                        .into_iter()
                        .chain(settle_stmts(route, false))
                        .chain([TsStmt::continue_stmt(None)])
                        .collect(),
                    None,
                ),
            ),
            const_(
                "result",
                await_expr(call(
                    member(ident("surface"), method_key),
                    handler_args(route),
                )),
            ),
            TsStmt::if_else_stmt(
                strict_eq(member(ident("result"), "tag"), str_lit("Ack")),
                // #3899: a signed consumer keeps the message's id for the replay window once it
                // is handled, before it is acknowledged, so a failure to keep it retries.
                ack_stmt(route),
                TsStmt::inline_block(
                    settle_stmts(route, false)
                        .into_iter()
                        .chain([
                            expr_stmt(method_call(
                                ident("globalThis.console"),
                                "error",
                                vec![
                                    str_lit(format!("queue {} retry", route.name)),
                                    // Past the `Ack` test the verdict is a `Retry` or a
                                    // `RetryAfter`: a handler cannot return `Undecodable` (#3878).
                                    member(
                                        as_expr(
                                            ident("result"),
                                            TsType::Object(vec![TsTypeMember::readonly_prop(
                                                "reason",
                                                TsType::named("string"),
                                            )]),
                                        ),
                                        "reason",
                                    ),
                                ],
                            )),
                            // #3881: a `RetryAfter` retries after its delay.
                            expr_stmt(call(
                                ident("__bynkRetryMessage"),
                                vec![str_lit(route.name.clone()), ident("msg"), ident("result")],
                            )),
                        ])
                        .collect(),
                    None,
                ),
                None,
            ),
        ]);
        // The catch clause's own real content packs both statements onto
        // one compact line (`console.error(...); msg.retry();`) inside the
        // usual braces-on-their-own-lines shape — `TsStmt::try_catch`
        // directly, not the `try_catch_stmt` convenience helper, since this
        // is the one real site in this module needing `InlineBlock` here
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
                ]
                .into_iter()
                .chain(release_on_throw(route))
                .collect(),
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
                block(
                    signed_decl(route)
                        .into_iter()
                        .chain([for_of_body])
                        .collect(),
                ),
                None,
            ),
            return_(None),
        ];
        cases.push(case_(str_lit(route.name.clone()), case_body));
    }
    cases.push(default_case(vec![return_(None)]));
    if !queue_routes.is_empty() {
        body.push(switch_stmt(member(ident("batch"), "queue"), cases));
    }

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

/// The arguments of the handler call: the decoded message, and (#3880) the
/// delivery metadata, each in the position the handler declared it. Cloudflare's
/// `attempts` starts at 1 and `timestamp` is when the message was sent.
fn handler_args(route: &QueueRoute) -> Vec<TsExpr> {
    let message = member(ident("__r"), "value");
    // #3899: a signed sum's resolved actor comes first, as a `call` sum's does.
    let who = route
        .signed
        .as_ref()
        .is_some_and(|s| s.sum.is_some())
        .then(|| ident("__who"));
    let rest = match route.delivery_at {
        None => vec![message],
        Some(at) => {
            let delivery = TsExpr::object(vec![
                ("attempts".to_string(), member(ident("msg"), "attempts")),
                (
                    "sentAt".to_string(),
                    method_call(member(ident("msg"), "timestamp"), "getTime", vec![]),
                ),
            ]);
            if at == 0 {
                vec![delivery, message]
            } else {
                vec![message, delivery]
            }
        }
    };
    who.into_iter().chain(rest).collect()
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
    /// #3880: the position of the handler's `QueueDelivery` parameter, if it has one.
    pub(super) delivery_at: Option<usize>,
    /// #3878: the consumer's dead-letter queue, if its `delivery { }` names one.
    pub(super) dead_letter: Option<String>,
    /// #3899: set when the consumer admits only signed messages.
    pub(super) signed: Option<SignedRoute>,
}

/// #3899: a signed consumer's admission: the foreign senders it admits as (actor, secret name), and
/// the queues that dead-letter into it, whose messages it also verifies (they were signed for their
/// source queue, not this one).
#[derive(Debug, Clone)]
pub(super) struct SignedRoute {
    pub(super) secrets: Vec<(String, String)>,
    /// The `Producer` refinements it admits, as (actor, predicate over the sending context): Bynk
    /// producers sign under their edge key rather than a declared secret.
    pub(super) producers: Vec<(String, bynk_check::actors::CallerPredicate)>,
    pub(super) sources: Vec<String>,
    /// For a sum (`by who: Legacy | Partner`), its members: the handler is passed which one signed.
    pub(super) sum: Option<Vec<bynk_check::actors::CallSumMember>>,
}

/// `const __a = await __bynkQueueAdmit(msg.body, <queue>, [<sources>], env, { <actor>: <secret> });`
/// then, on a refusal, `__bynkQueueRefused(ctx, <queue>, __a.reason, msg); continue;`: the message
/// is acknowledged and reported and never reaches the handler. Nothing for an unsigned consumer.
fn admission_stmts(route: &QueueRoute) -> Vec<TsStmt> {
    let Some(signed) = &route.signed else {
        return Vec::new();
    };
    let sources = TsExpr::Array {
        items: signed.sources.iter().map(|q| str_lit(q.clone())).collect(),
        multiline: false,
    };
    let secrets = TsExpr::object(
        signed
            .secrets
            .iter()
            .map(|(actor, secret)| (actor.clone(), str_lit(secret.clone())))
            .collect(),
    );
    vec![
        const_(
            "__a",
            await_expr(call(
                ident("__bynkQueueAdmit"),
                vec![
                    member(ident("msg"), "body"),
                    str_lit(route.name.clone()),
                    sources,
                    ident("env"),
                    secrets,
                ]
                .into_iter()
                .chain(admits_context(signed))
                .collect(),
            )),
        ),
        if_(
            not_expr(member(ident("__a"), "ok")),
            TsStmt::inline_block(
                vec![
                    expr_stmt(call(
                        ident("__bynkQueueRefused"),
                        vec![
                            ident("ctx"),
                            str_lit(route.name.clone()),
                            ident("__a"),
                            ident("msg"),
                        ],
                    )),
                    TsStmt::continue_stmt(None),
                ],
                None,
            ),
        ),
        // Kept for the catch, which releases the claim if the handler throws.
        assign(ident("__s"), ident("__a")),
    ]
    .into_iter()
    .chain(signer_stmt(signed))
    .collect()
}

/// `(c: string) => <a refinement admits c>`: the Bynk contexts the consumer admits, when a `Producer`
/// refinement names any. The admission reads such a sender's edge key and nothing else about it.
fn admits_context(signed: &SignedRoute) -> Option<TsExpr> {
    let admitted = signed
        .producers
        .iter()
        .map(|(_, p)| paren(caller_predicate_expr(p, &ident("c"))))
        .reduce(or_expr)?;
    Some(arrow(
        vec![TsParam::new("c", Some(TsType::named("string")))],
        admitted,
    ))
}

/// `msg.ack();`, preceded for a signed consumer by keeping the message's id (`await __a.settle(true);`).
fn ack_stmt(route: &QueueRoute) -> TsStmt {
    let ack = expr_stmt(method_call(ident("msg"), "ack", vec![]));
    if route.signed.is_none() {
        return ack;
    }
    TsStmt::inline_block(
        settle_stmts(route, true).into_iter().chain([ack]).collect(),
        None,
    )
}

/// `const __who = <the signer>;` for a sum: the message's signer, which the admission already
/// verified is one of the members, as the actor the handler's `match` resolves. A declared foreign
/// sender is `{ tag: __a.from }`; a Bynk context is the first `Producer` refinement whose predicate
/// admits it, with the context's name as its identity, as a `Caller` refinement's member carries it.
fn signer_stmt(signed: &SignedRoute) -> Option<TsStmt> {
    let members = signed.sum.as_ref()?;
    let from = || member(ident("__a"), "from");
    let foreign_tags = members
        .iter()
        .filter(|m| matches!(m, bynk_check::actors::CallSumMember::Foreign(_)))
        .map(|m| format!("\"{}\"", m.actor_name()))
        .collect::<Vec<_>>()
        .join(" | ");
    let foreign = TsExpr::object(vec![(
        "tag".to_string(),
        as_expr(from(), TsType::named(foreign_tags)),
    )]);
    let who = match signed.producers.split_last() {
        None => foreign,
        Some(((last, _), rest)) => {
            let member_of = |actor: &str| {
                TsExpr::object(vec![
                    ("tag".to_string(), str_lit(actor.to_string())),
                    ("identity".to_string(), from()),
                ])
            };
            let context = rest
                .iter()
                .rev()
                .fold(member_of(last), |otherwise, (actor, pred)| {
                    cond_expr(
                        paren(caller_predicate_expr(pred, &from())),
                        member_of(actor),
                        otherwise,
                    )
                });
            if signed.secrets.is_empty() {
                context
            } else {
                cond_expr(member(ident("__a"), "context"), context, foreign)
            }
        }
    };
    Some(const_typed(
        "__who",
        crate::emitter::emit::call_sum_ts_type(members),
        who,
    ))
}

/// `let __s: { settle(handled: boolean): globalThis.Promise<void> } | undefined;` before a signed
/// consumer's `try`, so its `catch` can release the id's claim: the admission is bound inside the `try`.
fn signed_decl(route: &QueueRoute) -> Option<TsStmt> {
    route.signed.as_ref()?;
    Some(TsStmt::let_stmt(
        bynk_ts::TsBindingName::Ident("__s".to_string()),
        Some(TsType::Union {
            members: vec![
                TsType::Object(vec![TsTypeMember::method(
                    "settle",
                    vec![TsParam::new("handled", Some(TsType::named("boolean")))],
                    TsType::named_with_args("globalThis.Promise", vec![TsType::named("void")]),
                )]),
                TsType::named("undefined"),
            ],
            multiline: false,
        }),
        None,
        None,
    ))
}

/// `await __a.settle(<handled>);`: a signed consumer keeps the message's id for the replay window
/// when it was handled (`true`), and releases it when it was not (`false`), so the platform's
/// redelivery of a message that failed is admitted again. Nothing for an unsigned consumer.
fn settle_stmts(route: &QueueRoute, handled: bool) -> Vec<TsStmt> {
    if route.signed.is_none() {
        return Vec::new();
    }
    vec![expr_stmt(await_expr(method_call(
        ident("__a"),
        "settle",
        vec![bool_lit(handled)],
    )))]
}

/// `if (__s !== undefined) await __s.settle(false);`: the claim of a message whose handler threw.
fn release_on_throw(route: &QueueRoute) -> Vec<TsStmt> {
    if route.signed.is_none() {
        return Vec::new();
    }
    vec![if_(
        not_expr(strict_eq(ident("__s"), ident("undefined"))),
        expr_stmt(await_expr(method_call(
            ident("__s"),
            "settle",
            vec![bool_lit(false)],
        ))),
    )]
}

/// The binding a consumer of `queue` reads its dead-letter producer from (#3878). It is keyed
/// by the consumer's queue, so a context with several consumers, and a Worker group's members,
/// never share a name that means two queues.
pub(crate) fn dead_letter_binding(queue: &str) -> String {
    // A queue name is lowercase letters, digits and `-` (`bynk.queue.invalid_name`), so mapping
    // `-` to `_` is one-to-one: two queues never share a binding.
    format!("__BYNK_DLQ_{}", queue.to_uppercase().replace('-', "_"))
}

/// `__bynkUndecodable(ctx, env, <binding>, <queue>, <type>, __r.error, msg)`: the verdict on a
/// message whose body did not decode.
fn undecodable_call(route: &QueueRoute) -> TsExpr {
    let binding = if route.dead_letter.is_some() {
        str_lit(dead_letter_binding(&route.name))
    } else {
        ident("undefined")
    };
    call(
        ident("__bynkUndecodable"),
        vec![
            ident("ctx"),
            ident("env"),
            binding,
            str_lit(route.name.clone()),
            str_lit(route.msg_type.as_ref().map_or_else(
                || "unknown".to_string(),
                bynk_check::context_checks::ts_type_ref_display,
            )),
            member(ident("__r"), "error"),
            ident("msg"),
        ],
    )
}

/// Collect the queue consumers across the services, carrying the per-service
/// declaration index and sorting by queue name.
pub(super) fn collect_queue_routes(
    table: &UnitTable,
    service_names: &[&String],
) -> Vec<QueueRoute> {
    let mut queue_routes: Vec<QueueRoute> = Vec::new();
    let deliveries = bynk_check::symbols::queue_deliveries(table);
    for sname in service_names {
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
                let delivery_at = h
                    .params
                    .iter()
                    .position(|p| bynk_syntax::queue::is_queue_delivery(&p.type_ref));
                let msg_type = h
                    .params
                    .iter()
                    .find(|p| !bynk_syntax::queue::is_queue_delivery(&p.type_ref))
                    .map(|p| p.type_ref.clone());
                let signed =
                    bynk_check::actors::queue_admission_for(h, &service.protocol, &table.actors)
                        .map(|a| SignedRoute {
                            sum: a.sum,
                            secrets: a.foreign.into_iter().map(|f| (f.actor, f.secret)).collect(),
                            producers: a.producers,
                            sources: deliveries
                                .iter()
                                .filter(|d| d.dead_letter.as_deref() == Some(name.as_str()))
                                .map(|d| d.queue.clone())
                                .collect(),
                        });
                queue_routes.push(QueueRoute {
                    service: (*sname).clone(),
                    index: queue_idx,
                    name: name.clone(),
                    msg_type,
                    delivery_at,
                    dead_letter: deliveries
                        .iter()
                        .find(|d| &d.queue == name)
                        .and_then(|d| d.dead_letter.clone()),
                    signed,
                });
                queue_idx += 1;
            }
        }
    }
    queue_routes.sort_by(|a, b| a.name.cmp(&b.name));
    queue_routes
}
