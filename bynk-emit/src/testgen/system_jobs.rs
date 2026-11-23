//! The cron and queue drivers of a `system` suite's target.
//!
//! Split out of `system_support.rs` when the test-harness generation moved into
//! `bynk-emit` (1B.2a, #4028), so that no file arrives over 2,000 production lines.

use super::*;
use crate::emitter::RuntimeUse;
use crate::emitter::emit::test_scaffold_ns as test_ns;
use bynk_ts::{TsDecl, TsExpr, TsParam, TsStmt, TsType};

/// #2136: the cron and queue drivers of a `system` suite's target, and the
/// services they cover ([`emitter::ctx::SystemJobServices`]).
pub(super) struct SystemJobSupport {
    pub(super) code: Vec<TsStmt>,
    pub(super) services: emitter::ctx::SystemJobServices,
}

/// #2136: one driver per cron handler and per queue service of the `system`
/// target, calling the participant Worker's real entry point with a
/// platform-shaped event and the harness's execution-context double, which it
/// settles (ADR 0478). The runtime half lives at `runtime/src/testing/jobs.ts`.
///
/// - `__sysdrive_schedule_<svc>_<index>(at?)` calls `scheduled({ cron,
///   scheduledTime }, env, ctx)` and yields the `Result` the test build's
///   entry stored on `ctx` (`scheduled_result_hook`). A handler without the
///   scheduled-time parameter is fired at `0`. A handler that throws fails the
///   case, as it would at `unit`.
/// - `__sysdrive_message_<q>(m)` serialises `m` as the producer does, sends it
///   through a JSON round trip, and calls `queue(batch, env)` with a
///   one-message batch whose `ack()` / `retry()` the double records, yielding
///   `Ack` or `Retry(reason)` (the reason the entry logs).
pub(super) fn emit_system_job_support(
    target: &str,
    unit_tables: &HashMap<String, UnitTable>,
    runtime_use: &RuntimeUse,
    target_sends: bool,
) -> SystemJobSupport {
    use bynk_model::{IrHandlerKind, lower_handler_kind_ir};
    use bynk_syntax::ast::ServiceProtocol;
    let mut services = emitter::ctx::SystemJobServices::default();
    let mut drivers: Vec<TsStmt> = Vec::new();
    let Some(table) = unit_tables.get(target) else {
        return SystemJobSupport {
            code: drivers,
            services,
        };
    };
    let ns = test_ns(target);
    let worker = worker_binding_name(&ns);
    let env = member(ident("__h"), env_binding(&ns));
    let type_ns = ns_qualifier(&ns);
    let mut svc_names: Vec<&String> = table.services.keys().collect();
    svc_names.sort();
    for sname in svc_names {
        let decl = &table.services[sname];
        // M-1's domain clause (ADR 0735; crate-hygiene B4, #3012): this
        // function holds the target's symbol table, not its `TypedCommons`, so it
        // doesn't hold `bynk_model::lower_protocol_ir`'s input.
        match &decl.protocol {
            ServiceProtocol::Cron => {
                let mut exprs = Vec::new();
                for h in &decl.handlers {
                    let IrHandlerKind::Cron { expr } = lower_handler_kind_ir(&h.kind) else {
                        continue;
                    };
                    let idx = exprs.len();
                    let key = crate::emitter::cron_handler_method_name(sname, idx);
                    let (params, at) = match h.params.first() {
                        Some(p) => (
                            vec![TsParam::new("at", Some(driver_param_type(&p.type_ref)))],
                            ident("at"),
                        ),
                        None => (Vec::new(), num_lit("0")),
                    };
                    let scheduled = call(
                        type_app(
                            ident("__bynkScheduled"),
                            vec![awaited_return(&ns, sname, &key)],
                        ),
                        vec![
                            str_lit(expr.clone()),
                            arrow(
                                vec![TsParam::new("exec", None)],
                                method_call(
                                    ident(worker.clone()),
                                    "scheduled",
                                    vec![
                                        TsExpr::object(vec![
                                            ("cron".to_string(), str_lit(expr.clone())),
                                            ("scheduledTime".to_string(), at),
                                        ]),
                                        env.clone(),
                                        ident("exec"),
                                    ],
                                ),
                            ),
                        ],
                    );
                    drivers.push(async_driver(
                        format!("__sysdrive_schedule_{sname}_{idx}"),
                        params,
                        vec![harness_const(), ret(scheduled)],
                    ));
                    exprs.push(expr);
                }
                services.cron.insert(sname.clone(), exprs);
            }
            ServiceProtocol::Queue { name } => {
                let Some(h) = decl
                    .handlers
                    .iter()
                    .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Message))
                else {
                    continue;
                };
                let Some(p) = h.params.first() else {
                    continue;
                };
                let key = crate::emitter::queue_handler_method_name(sname, 0);
                let param_name = crate::emitter::ts_ident(&p.name.name);
                let body = crate::emitter::serialisation::serialise_expr_via(
                    &p.type_ref,
                    &TsExpr::Ident(param_name.clone()),
                    &type_ns,
                    runtime_use,
                );
                let mut forward_args = vec![ident("batch"), env.clone()];
                if target_sends {
                    forward_args.push(ident("exec"));
                }
                let delivered = call(
                    type_app(
                        ident("__bynkDeliver"),
                        vec![awaited_return(&ns, sname, &key)],
                    ),
                    vec![
                        str_lit(name.clone()),
                        body,
                        arrow(
                            vec![TsParam::new("batch", None), TsParam::new("exec", None)],
                            method_call(ident(worker.clone()), "queue", forward_args),
                        ),
                    ],
                );
                drivers.push(async_driver(
                    format!("__sysdrive_message_{sname}"),
                    vec![TsParam::new(
                        param_name,
                        Some(driver_param_type(&p.type_ref)),
                    )],
                    vec![harness_const(), ret(delivered)],
                ));
                services.queue.insert(sname.clone());
            }
            // #3328: an email service is driven as raw MIME through the Worker's
            // real `email()` entry, which parses it; the case reads back the
            // verdict the entry applied (`__bynkDeliverEmail`).
            ServiceProtocol::Email { .. } => {
                let Some(h) = decl
                    .handlers
                    .iter()
                    .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Message))
                else {
                    continue;
                };
                let key = crate::emitter::queue_handler_method_name(sname, 0);
                let param_name = h.params.first().map_or("mail".to_string(), |p| {
                    crate::emitter::ts_ident(&p.name.name)
                });
                let delivered = call(
                    type_app(
                        ident("__bynkDeliverEmail"),
                        vec![awaited_return(&ns, sname, &key)],
                    ),
                    vec![
                        ident(param_name.clone()),
                        arrow(
                            vec![TsParam::new("message", None), TsParam::new("exec", None)],
                            method_call(
                                ident(worker.clone()),
                                "email",
                                vec![ident("message"), env.clone(), ident("exec")],
                            ),
                        ),
                    ],
                );
                drivers.push(async_driver(
                    format!("__sysdrive_message_{sname}"),
                    vec![TsParam::new(param_name, Some(any_type()))],
                    vec![harness_const(), ret(delivered)],
                ));
                services.email.insert(sname.clone());
            }
            ServiceProtocol::Call
            | ServiceProtocol::Http
            | ServiceProtocol::WebSocket { .. }
            | ServiceProtocol::Events { .. } => {}
        }
    }
    let mut code = Vec::new();
    if !drivers.is_empty() {
        code.push(TestRuntimeModule::Jobs.splice());
        // #3328: only a target with an email service carries the email driver.
        if !services.email.is_empty() {
            code.push(TestRuntimeModule::JobsEmail.splice());
        }
        code.extend(drivers);
    }
    SystemJobSupport { code, services }
}

/// `async function <name>(<params>) { <body> }`: a driver the harness calls.
pub(super) fn async_driver(name: String, params: Vec<TsParam>, body: Vec<TsStmt>) -> TsStmt {
    TsStmt::decl(
        TsDecl::Function {
            name,
            generics: Vec::new(),
            params,
            return_type: None,
            body,
            is_async: true,
            inline: false,
        },
        None,
    )
}

/// `const __h = __makeHarness();`, the first statement of every driver.
pub(super) fn harness_const() -> TsStmt {
    const_("__h", call(ident("__makeHarness"), Vec::new()))
}

/// `Awaited<ReturnType<typeof <ns>.<service>.<method>>>`: what a handler's driver yields.
fn awaited_return(ns: &str, sname: &str, method: &str) -> TsType {
    let handler = member(member(ident(ns), sname), method);
    TsType::named_with_args(
        "Awaited",
        vec![TsType::named_with_args(
            "ReturnType",
            vec![TsType::type_of(handler)],
        )],
    )
}
