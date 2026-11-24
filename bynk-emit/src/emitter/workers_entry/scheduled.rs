//! The Worker's `scheduled()` export: the cron routes and their dispatch.

use super::respond::{case_, default_case};
use crate::ts_build::{
    arrow, as_expr, await_expr, call, const_, expr_stmt, ident, if_, member, method_call, return_,
    str_lit, strict_neq, switch_stmt,
};
use bynk_check::symbols::UnitTable;
use bynk_ts::{TsExpr, TsObjectEntry, TsParam, TsStmt, TsSwitchCase, TsType, TsTypeMember};

/// Emit the Worker `scheduled` handler aggregating every `on cron` handler in
/// the context. `event` is typed structurally (`{ cron: string }`) to avoid a
/// dependency on `@cloudflare/workers-types`, matching how the rest of the
/// emitter hand-declares the minimal ambient shapes it needs.
///
/// #2136: with `result_hook` (the test build of a context a `system` case
/// drives), the entry takes the execution context even when no handler sends,
/// and stores each run's `Result` on it as `__bynkResult` for the harness's
/// driver to read back. The deployable entry is unchanged.
pub(super) fn emit_scheduled_handler(
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
        // Pass what each declared parameter asks for, in declaration order: the
        // scheduled fire time (epoch ms) and/or the schedule that fired.
        let args: Vec<TsExpr> = route
            .params
            .iter()
            .map(|p| match p {
                bynk_syntax::cron::CronParam::Time => member(ident("event"), "scheduledTime"),
                // `event.cron` is the schedule this case matched on, which the
                // checker proved is a valid `CronSchedule`; the brand is a type, so
                // the plain string is cast to it.
                bynk_syntax::cron::CronParam::Schedule => {
                    as_expr(member(ident("event"), "cron"), TsType::named("never"))
                }
            })
            .collect();
        // #3872: the run goes through `__bynkCronRun`, which marks an `Err` or a
        // fault as failed to the platform (`__bynkReportFault`) and logs a
        // labelled record (type and variant, never the payload).
        let run = arrow(vec![], call(member(ident("surface"), method_key), args));
        let mut case_body = vec![const_(
            "result",
            await_expr(call(
                ident("__bynkCronRun"),
                vec![
                    ident("ctx"),
                    str_lit(route.site()),
                    str_lit(route.err_type.clone()),
                    TsExpr::array(route.variants.iter().map(str_lit).collect()),
                    run,
                ],
            )),
        )];
        if result_hook {
            let store = TsStmt::assign(member(ident("ctx"), "__bynkResult"), ident("result"), None);
            case_body.push(if ctx_optional {
                if_(strict_neq(ident("ctx"), ident("undefined")), store)
            } else {
                store
            });
        }
        case_body.push(return_(None));
        cases.push(case_(str_lit(route.expr.clone()), case_body));
    }
    // #3872: an expression no handler declared (a trigger edited outside
    // `wrangler.toml`) is named in one warning instead of dropped silently.
    cases.push(default_case(vec![
        expr_stmt(method_call(
            ident("globalThis.console"),
            "warn",
            vec![
                str_lit("cron expression matches no `on schedule` handler in this context:"),
                member(ident("event"), "cron"),
            ],
        )),
        return_(None),
    ]));
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

/// One `on cron` handler, identified by its service and per-service declaration
/// index (which together form its `cron_<service>_<index>` method key).
#[derive(Debug, Clone)]
pub(super) struct CronRoute {
    pub(super) service: String,
    pub(super) index: usize,
    pub(super) expr: String,
    /// What each declared parameter receives, in declaration order (#3874): the
    /// scheduled time (`event.scheduledTime`) and/or the schedule that fired
    /// (`event.cron`).
    pub(super) params: Vec<bynk_syntax::cron::CronParam>,
    /// #3872: the handler's declared error type, named in the failure record.
    pub(super) err_type: String,
    /// #3872: the variant names of that error when it is a sum declared here; the
    /// only `tag` the record may name, so a record's own `tag` field never reaches it.
    pub(super) variants: Vec<String>,
}

impl CronRoute {
    /// The label a failed run carries: its schedule and service.
    fn site(&self) -> String {
        format!("cron `{}` ({})", self.expr, self.service)
    }
}

/// Collect the cron handlers across the services, carrying the per-service
/// declaration index and sorting by schedule expression.
pub(super) fn collect_cron_routes(table: &UnitTable, service_names: &[&String]) -> Vec<CronRoute> {
    let mut cron_routes: Vec<CronRoute> = Vec::new();
    for sname in service_names {
        let service = table.services.get(*sname).unwrap();
        let mut cron_idx = 0usize;
        for h in &service.handlers {
            // P6.31: dispatches on `IrHandlerKind`; `expr` is a plain `String`
            // in both, so no AST re-derivation is needed here (unlike the
            // `Http` arm in `http::collect_http_routes`).
            if let bynk_model::IrHandlerKind::Cron { expr } =
                bynk_model::lower_handler_kind_ir(&h.kind)
            {
                cron_routes.push(CronRoute {
                    service: (*sname).clone(),
                    index: cron_idx,
                    expr,
                    params: h
                        .params
                        .iter()
                        .filter_map(|p| bynk_syntax::cron::cron_param(&p.type_ref))
                        .collect(),
                    err_type: bynk_syntax::cron::cron_error_type(&h.return_type),
                    variants: bynk_check::symbols::sum_variant_names(
                        table,
                        &bynk_syntax::cron::cron_error_type(&h.return_type),
                    ),
                });
                cron_idx += 1;
            }
        }
    }
    cron_routes.sort_by(|a, b| a.expr.cmp(&b.expr));
    cron_routes
}
