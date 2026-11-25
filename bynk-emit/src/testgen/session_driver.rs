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
    /// The actor's identity type, which the signed-in `sub` must construct.
    identity: String,
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
            identity: seam.identity_type.clone(),
        })
    }

    /// `await <agent>.revokeAll()`: end every session of `sub` in the agent, as `by Revoked` does
    /// after its cookie is issued, so the seam must refuse the cookie.
    pub(super) fn revoke(&self, sub: TsExpr) -> Vec<TsStmt> {
        // `by Revoked` claims to test revocation, so it must not pass because the seam refused a
        // `sub` the identity type rejects before the session was ever looked at.
        let identity_ok = crate::ts_build::strict_eq(
            member(
                method_call(
                    member(ident(self.handlers_ns.clone()), self.identity.clone()),
                    "of",
                    vec![sub.clone()],
                ),
                "tag",
            ),
            str_lit("Ok"),
        );
        let refuse = TsStmt::throw_stmt(
            crate::ts_build::new(
                ident("globalThis.Error"),
                vec![str_lit(format!(
                    "`by Revoked` signs in as the test subject, which the identity type `{}` rejects, so it could not test revocation: declare an identity that accepts it",
                    self.identity
                ))],
            ),
            None,
        );
        vec![
            crate::ts_build::if_(crate::ts_build::not_expr(identity_ok), refuse),
            self.revoke_stmt(sub),
        ]
    }

    fn revoke_stmt(&self, sub: TsExpr) -> TsStmt {
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
        TsExpr::template_lit(
            vec![self.cookie.clone() + "=", String::new()],
            vec![self.issue_value(keys, sub, issued_secs)],
        )
    }

    /// `(await __issueSessionCookie(…)).value`: the signed cookie value, its session recorded in the
    /// agent first.
    fn issue_value(&self, keys: &TsExpr, sub: TsExpr, issued_secs: TsExpr) -> TsExpr {
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
        member(paren(issued), "value")
    }

    /// The pair a signed-in principal (`by User("ada")`) sends: one session per principal and case,
    /// like one browser, so a later call in the case replays the same cookie and sees a session an
    /// earlier call ended or revoked as refused. The cookies are kept on the case's harness `env`,
    /// which each case builds afresh.
    pub(super) fn principal_pair(&self, keys: &TsExpr, sub: TsExpr, issued_secs: TsExpr) -> TsExpr {
        let env = member(call(ident("__makeHarness"), Vec::new()), self.env.clone());
        let jar = |e: &TsExpr| member(e.clone(), "__sessionCookies");
        let key = ident("__k");
        let cached = |name: &str, args: Vec<TsExpr>| {
            method_call(jar(&ident("__env")), name.to_string(), args)
        };
        let body = vec![
            crate::ts_build::const_("__env", env),
            crate::ts_build::if_(
                crate::ts_build::strict_eq(jar(&ident("__env")), ident("undefined")),
                TsStmt::assign(
                    jar(&ident("__env")),
                    crate::ts_build::new(ident("Map"), Vec::new()),
                    None,
                ),
            ),
            crate::ts_build::if_(
                crate::ts_build::not_expr(cached("has", vec![key.clone()])),
                expr_stmt(cached(
                    "set",
                    vec![
                        key.clone(),
                        self.issue_value(keys, key.clone(), issued_secs),
                    ],
                )),
            ),
            crate::ts_build::return_(Some(cached("get", vec![key]))),
        ];
        let session = call(
            paren(crate::ts_build::block_arrow(
                vec![TsParam::new(
                    "__k".to_string(),
                    Some(TsType::named("string")),
                )],
                true,
                body,
            )),
            vec![sub],
        );
        TsExpr::template_lit(
            vec![self.cookie.clone() + "=", String::new()],
            vec![await_expr(session)],
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
    /// #4385: `__auth["cookie"] = <bad session cookie>` for each of `by Forged` (signed with a key
    /// the seam does not hold), `by Expired` (issued before the cookie's `ttl`) and, with
    /// `sessions = true`, `by Revoked` (a registered session, then revoked), guarded by `is(<bad>)`.
    pub(super) fn bad_cookie_stmts(&self, is: &dyn Fn(&str) -> TsExpr) -> Vec<TsStmt> {
        use crate::ts_build::{assign, if_, index};
        let cookie = || index(ident("__auth"), str_lit("cookie"));
        let sub = || str_lit("bynk-test");
        let forged_keys = TsExpr::object(vec![(
            "secret".to_string(),
            call(ident("__newSessionKey"), Vec::new()),
        )]);
        let before_ttl = binary(
            TsBinaryOp::Sub,
            binary(
                TsBinaryOp::Sub,
                call(ident("__bynkNow"), Vec::new()),
                num_lit(self.ttl_secs),
            ),
            num_lit(3600),
        );
        let now = || call(ident("__bynkNow"), Vec::new());
        let mut out = vec![
            if_(
                is("forged"),
                assign(cookie(), self.pair(&forged_keys, sub(), now())),
            ),
            if_(
                is("expired"),
                assign(cookie(), self.pair(&self.keys, sub(), before_ttl)),
            ),
        ];
        if let Some(agent) = &self.sessions {
            let mut revoked = vec![assign(cookie(), self.pair(&self.keys, sub(), now()))];
            revoked.extend(agent.revoke(sub()));
            out.push(if_(is("revoked"), crate::ts_build::block(revoked)));
        }
        out
    }

    /// The pair the acting principal sends: for a `sessions = true` actor, the case's one session
    /// for that principal (see `SessionAgentRef::principal_pair`); otherwise a fresh signed cookie.
    pub(super) fn principal_pair(&self, sub: TsExpr, issued_secs: TsExpr) -> TsExpr {
        match &self.sessions {
            Some(agent) => agent.principal_pair(&self.keys, sub, issued_secs),
            None => self.pair(&self.keys, sub, issued_secs),
        }
    }

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
