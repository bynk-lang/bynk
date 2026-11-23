//! The admin and inspect routes of the Worker entry: `/_bynk/inspect/` and the migrate route.

use super::call::call_method_gate;
use super::respond::{case_, default_case, miss, own_path_guard, text_response};
use crate::ts_build::{
    and_expr, await_expr, block, call, const_, ident, if_, index, member, method_call, num_lit,
    or_expr, paren, return_, str_lit, strict_eq, strict_neq, switch_stmt,
};
use bynk_check::wire_format as wf;
use bynk_ts::{TsDecl, TsExpr, TsStmt, TsSwitchCase};

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
                    str_lit(crate::emitter::bindings::agent_binding(c).name),
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
pub(super) fn inspect_registry_export(inspect: &[String]) -> Option<TsStmt> {
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
pub(super) fn admin_routes(
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
                            crate::emitter::bindings::agent_binding(class).name,
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
                    expr: Box::new(index(
                        super::super::emit::record_cast(
                            ident("env"),
                            bynk_ts::TsType::named("unknown"),
                        ),
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

#[cfg(test)]
mod tests {
    use super::*;
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
