//! #3938: a `system` driver's session for a `sessions = true` `SignedCookie` actor. The acting
//! principal's session is recorded in the actor's generated session agent by the same
//! `issueSessionCookie` the handlers' `Actor.issue` runs, so the seam finds it live.

use bynk_ts::{TsBinaryOp, TsExpr, TsParam, TsStmt, TsType};

use crate::ts_build::{
    arrow_with, as_expr, await_expr, binary, call, expr_stmt, ident, member, method_call, num_lit,
    paren, str_lit,
};

/// How a driver reaches a `sessions = true` actor's session agent: the participant's handlers
/// namespace, its `env` binding (a field of the memoised harness), the agent's factory name and the
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

    /// `await <agent>.revokeAll()`: end every session of `sub` in the agent, as `by Revoked` does
    /// after its cookie is issued, so the seam must refuse the cookie.
    pub(super) fn revoke(&self, sub: TsExpr) -> TsStmt {
        expr_stmt(await_expr(method_call(
            call(
                member(ident(self.handlers_ns.clone()), self.factory.clone()),
                vec![
                    as_expr(sub, TsType::named("never")),
                    member(call(ident("__makeHarness"), Vec::new()), self.env.clone()),
                ],
            ),
            "revokeAll",
            vec![TsExpr::object(Vec::new())],
        )))
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
                        member(call(ident("__makeHarness"), Vec::new()), agent.env.clone()),
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

/// #3386: what a `system` driver needs to mint a `SignedCookie` actor's session cookie.
pub(super) struct SessionDriver {
    /// The `{ secret: … }` keys expression the cookie is signed with.
    pub(super) keys: TsExpr,
    /// The declaring actor's name: the cookie's `aud`.
    pub(super) actor: String,
    /// The cookie's name.
    pub(super) cookie: String,
    /// How long an issued cookie is valid, in seconds.
    pub(super) ttl_secs: i64,
    /// #3938: for a `sessions = true` actor, how the acting principal's session is recorded in
    /// its session agent, as `issue` records one, so the seam finds it live.
    pub(super) sessions: Option<SessionAgentRef>,
}

impl SessionDriver {
    /// The `name=value` template literal for `sub` signed with `keys` and issued `issued_secs`
    /// (an expression, in seconds) after the epoch.
    pub(super) fn pair(&self, keys: &TsExpr, sub: TsExpr, issued_secs: TsExpr) -> TsExpr {
        use crate::ts_build::{await_expr, binary, call, num_lit, paren, str_lit};
        if let Some(agent) = &self.sessions {
            return agent.pair(keys, sub, issued_secs);
        }
        let signed = await_expr(call(
            ident("__signSessionCookie"),
            vec![
                keys.clone(),
                str_lit(self.actor.clone()),
                sub,
                num_lit(self.ttl_secs),
                binary(TsBinaryOp::Mul, paren(issued_secs), num_lit(1000)),
            ],
        ));
        TsExpr::template_lit(
            vec![format!("{}=", self.cookie), String::new()],
            vec![signed],
        )
    }
}
