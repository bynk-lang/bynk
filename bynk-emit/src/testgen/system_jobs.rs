//! The cron and queue drivers of a `system` suite's target.
//!
//! Split out of `system_support.rs` when the test-harness generation moved into
//! `bynk-emit` (1B.2a, #4028), so that no file arrives over 2,000 production lines.

use super::*;
use crate::emitter::RuntimeUse;
use crate::emitter::emit::test_scaffold_ns as test_ns;
use bynk_ts::TsStmt;

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
    let env = format!("__h.{}", env_binding(&ns));
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
                            format!("at: {}", driver_param_ty(&p.type_ref, &ns)),
                            "at".to_string(),
                        ),
                        None => (String::new(), "0".to_string()),
                    };
                    drivers.push(TsStmt::raw(
                        format!(
                            "async function __sysdrive_schedule_{sname}_{idx}({params}) {{\n  const __h = __makeHarness();\n  return __bynkScheduled<Awaited<ReturnType<typeof {ns}.{sname}.{key}>>>({expr:?}, (exec) => {worker}.scheduled({{ cron: {expr:?}, scheduledTime: {at} }}, {env}, exec));\n}}\n"
                        ),
                        None,
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
                let param = crate::emitter::ts_ident(&p.name.name);
                let body = bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                    &p.type_ref,
                    &bynk_ts::TsExpr::Ident(param.clone()),
                    &type_ns,
                    runtime_use,
                ));
                let exec_arg = if target_sends { ", exec" } else { "" };
                drivers.push(TsStmt::raw(
                    format!(
                        "async function __sysdrive_message_{sname}({param}: {ty}) {{\n  const __h = __makeHarness();\n  return __bynkDeliver<Awaited<ReturnType<typeof {ns}.{sname}.{key}>>>({name:?}, {body}, (batch, exec) => {worker}.queue(batch, {env}{exec_arg}));\n}}\n",
                        ty = driver_param_ty(&p.type_ref, &ns),
                    ),
                    None,
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
                let param = h.params.first().map_or("mail".to_string(), |p| {
                    crate::emitter::ts_ident(&p.name.name)
                });
                drivers.push(TsStmt::raw(
                    format!(
                        "async function __sysdrive_message_{sname}({param}: any) {{\n  const __h = __makeHarness();\n  return __bynkDeliverEmail<Awaited<ReturnType<typeof {ns}.{sname}.{key}>>>({param}, (message, exec) => {worker}.email(message, {env}, exec));\n}}\n"
                    ),
                    None,
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
