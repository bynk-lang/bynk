//! #3938: a `system` driver's session for a `sessions = true` `SignedCookie` actor. The acting
//! principal's session is recorded in the actor's generated session agent by the same
//! `issueSessionCookie` the handlers' `Actor.issue` runs, so the seam finds it live.

use bynk_ts::{TsBinaryOp, TsExpr, TsParam, TsType};

use crate::ts_build::{
    arrow_with, as_expr, await_expr, binary, call, ident, member, method_call, num_lit, paren,
    str_lit,
};

/// How a driver reaches a `sessions = true` actor's session agent: the participant's handlers
/// namespace, its `env` binding (a field of the harness `__h`), the agent's factory name and the
/// actor's own seam settings `issue` takes.
pub(super) struct SessionAgentRef {
    handlers_ns: String,
    env: String,
    factory: String,
    actor: String,
    cookie: String,
    same_site: String,
    ttl_secs: i64,
}

impl SessionAgentRef {
    /// The reference for `seam`'s actor in participant `ns`, or `None` without `sessions`.
    pub(super) fn of(seam: &bynk_check::actors::SignedCookieSeam, ns: &str) -> Option<Self> {
        seam.sessions.then(|| Self {
            handlers_ns: ns.to_string(),
            env: crate::testgen::names::env_binding(ns),
            factory: crate::emitter::agent_factory_name(&bynk_check::actors::sessions_agent_name(
                &seam.actor,
            )),
            actor: seam.actor.clone(),
            cookie: seam.cookie.clone(),
            same_site: seam.same_site.clone(),
            ttl_secs: seam.ttl_secs,
        })
    }

    /// The `name=value` template literal of `sub`'s session cookie, issued `issued_secs` after the
    /// epoch with `keys`, and recorded in the agent first.
    pub(super) fn pair(&self, keys: &TsExpr, sub: TsExpr, issued_secs: TsExpr) -> TsExpr {
        let agent = self;
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
                str_lit(self.actor.clone()),
                str_lit(self.cookie.clone()),
                str_lit(self.same_site.clone()),
                sub,
                num_lit(self.ttl_secs),
                binary(TsBinaryOp::Mul, paren(issued_secs), num_lit(1000)),
                register,
            ],
        ));
        TsExpr::template_lit(
            vec![self.cookie.clone() + "=", String::new()],
            vec![member(paren(issued), "value")],
        )
    }
}
