//! #3938: a `system` driver's session for a `sessions = true` `SignedCookie` actor. The acting
//! principal's session is recorded in the actor's generated session agent by the same
//! `issueSessionCookie` the handlers' `Actor.issue` runs, so the seam finds it live.

use bynk_ts::{TsBinaryOp, TsExpr, TsParam, TsType};

use crate::ts_build::{
    arrow_with, as_expr, await_expr, binary, call, ident, member, method_call, num_lit, paren,
    str_lit,
};

/// How a driver reaches a `sessions = true` actor's session agent: the participant's handlers
/// namespace, its `env` binding (a field of the harness `__h`) and the agent's factory name.
pub(super) struct SessionAgentRef {
    pub(super) handlers_ns: String,
    pub(super) env: String,
    pub(super) factory: String,
}

/// The `name=value` template literal of `sub`'s session cookie, issued `issued_secs` after the
/// epoch with `keys`, and recorded in the agent first. `(actor, cookie, same_site, ttl_secs)` are
/// the actor's own seam settings.
pub(super) fn registered_pair(
    keys: &TsExpr,
    (actor, cookie, same_site, ttl_secs): (&str, &str, &str, i64),
    agent: &SessionAgentRef,
    sub: TsExpr,
    issued_secs: TsExpr,
) -> TsExpr {
    let register = arrow_with(
        vec![
            TsParam::new("__sid".to_string(), Some(TsType::named("string"))),
            TsParam::new("__exp".to_string(), Some(TsType::named("number"))),
            TsParam::new("__now".to_string(), Some(TsType::named("number"))),
        ],
        true,
        method_call(
            call(
                member(ident(agent.handlers_ns.clone()), agent.factory.clone()),
                vec![
                    as_expr(sub.clone(), TsType::named("never")),
                    member(ident("__h"), agent.env.clone()),
                ],
            ),
            "add",
            vec![
                ident("__sid"),
                ident("__exp"),
                ident("__now"),
                TsExpr::object(Vec::new()),
            ],
        ),
    );
    let issued = await_expr(call(
        ident("__issueSessionCookie"),
        vec![
            keys.clone(),
            str_lit(actor.to_string()),
            str_lit(cookie.to_string()),
            str_lit(same_site.to_string()),
            sub,
            num_lit(ttl_secs),
            binary(TsBinaryOp::Mul, paren(issued_secs), num_lit(1000)),
            register,
        ],
    ));
    TsExpr::template_lit(
        vec![cookie.to_string() + "=", String::new()],
        vec![member(paren(issued), "value")],
    )
}
