//! The credentials an actor-sum route's `system` drivers present: one per member, chosen per
//! call by the actor the call acts as (#2155), including an `Oidc` token and a session cookie
//! (#3936).

use super::session_driver::{SessionAgentRef, SessionDriver};
use super::*;
use bynk_ts::{TsBinaryOp, TsExpr, TsObjectEntry, TsParam, TsStmt, TsType};

/// #2155: an actor-sum route's per-call credential. The driver takes the
/// actor the call acts as (`__as`) and builds the matching credential: a
/// Bearer member's token, a `Signature` member's signature, or nothing for a
/// member with no scheme.
pub(super) struct SumAuth {
    /// Each member with a credential, in declaration order.
    pub(super) members: Vec<(String, SumCredential)>,
}

pub(super) enum SumCredential {
    /// A Bearer member: how the driver reads its secret, and the `iss`/`aud` its tokens carry (#3821).
    Bearer(TsExpr, bynk_check::actors::BearerClaims),
    /// A `Signature` member.
    Signature(SigDriver),
    /// #3936: an `Oidc` member: the expression for its trust roots, which the test issuer mints a
    /// token against.
    Oidc(TsExpr),
    /// #3936: a `SignedCookie` member: how the driver mints its session cookie.
    Cookie(SessionDriver),
}

impl SumAuth {
    /// The statements building `__auth`, the credential headers for the actor
    /// the call acts as, signing `body` where a member signs.
    pub(super) fn stmts(&self, body: &TsExpr) -> Vec<TsStmt> {
        let auth = |key: &str| index(ident("__auth"), str_lit(key));
        let mut out = vec![const_typed(
            "__auth",
            TsType::named_with_args(
                "globalThis.Record",
                vec![TsType::named("string"), TsType::named("string")],
            ),
            TsExpr::object(Vec::new()),
        )];
        for (name, cred) in &self.members {
            let set: Vec<TsStmt> = match cred {
                // #4344: beside the principal's claims (`__claims`, which `sub` and `exp` override).
                SumCredential::Bearer(secret_read, bearer_claims) => vec![assign(
                    auth("authorization"),
                    bearer_token(
                        TsExpr::object_entries(
                            [
                                TsObjectEntry::Spread(ident("__claims")),
                                TsObjectEntry::prop("sub", ident("__sub")),
                                TsObjectEntry::prop(
                                    "exp",
                                    binary(TsBinaryOp::Add, now_secs(), num_lit("3600")),
                                ),
                            ]
                            .into_iter()
                            .chain(super::system_request::bearer_claim_entries(bearer_claims))
                            .collect(),
                        ),
                        secret_read.clone(),
                    ),
                )],
                // #4344: the principal's claims (`__claims`) are the token's `claims`.
                SumCredential::Oidc(trust) => vec![assign(
                    auth("authorization"),
                    oidc_token(
                        trust.clone(),
                        object_of(vec![
                            ("sub", ident("__sub")),
                            ("claims", ident("__claims")),
                            ("kind", str_lit("valid")),
                        ]),
                    ),
                )],
                SumCredential::Cookie(session) => vec![assign(
                    auth("cookie"),
                    session.principal_pair(ident("__sub"), now_secs()),
                )],
                SumCredential::Signature(z) => {
                    let mut set = vec![
                        z.sign(body, SigVariant::Signed),
                        assign(auth(&z.seam.header), member(ident("__sig"), "signature")),
                    ];
                    if let Some(ts) = &z.seam.timestamp_header {
                        set.push(assign(auth(ts), member(ident("__sig"), "timestamp")));
                    }
                    set
                }
            };
            out.push(if_(
                strict_eq(ident("__as"), str_lit(name.as_str())),
                block(set),
            ));
        }
        out
    }

    /// #4385: the statements building `__auth` for `by <Bad>(<Arm>)`: the bad credential of the
    /// member named by `__as`, of the kind `__bad` names (`"forged"`, `"expired"`, `"stale"`,
    /// `"revoked"`). The checker admits only a combination the member's seam can refuse, so each
    /// arm builds just the credentials its scheme has.
    pub(super) fn bad_stmts(&self, body: &TsExpr) -> Vec<TsStmt> {
        let auth = |key: &str| index(ident("__auth"), str_lit(key));
        let is = |bad: &str| strict_eq(ident("__bad"), str_lit(bad));
        let mut out = vec![const_typed(
            "__auth",
            TsType::named_with_args(
                "globalThis.Record",
                vec![TsType::named("string"), TsType::named("string")],
            ),
            TsExpr::object(Vec::new()),
        )];
        for (name, cred) in &self.members {
            let mut set: Vec<TsStmt> = Vec::new();
            match cred {
                SumCredential::Bearer(secret_read, bearer_claims) => {
                    for (bad, secret, sign) in [
                        (
                            "forged",
                            paren(binary(TsBinaryOp::Add, secret_read.clone(), str_lit("!"))),
                            TsBinaryOp::Add,
                        ),
                        ("expired", paren(secret_read.clone()), TsBinaryOp::Sub),
                    ] {
                        set.push(if_(
                            is(bad),
                            assign(
                                auth("authorization"),
                                bearer_token(
                                    super::system_request::bearer_bad_claims(
                                        binary(sign, now_secs(), num_lit("3600")),
                                        bearer_claims,
                                    ),
                                    secret,
                                ),
                            ),
                        ));
                    }
                }
                SumCredential::Oidc(trust) => {
                    set.push(if_(
                        or_expr(is("forged"), is("expired")),
                        assign(
                            auth("authorization"),
                            oidc_token(
                                trust.clone(),
                                object_of(vec![
                                    ("sub", str_lit("bynk-test")),
                                    ("claims", object_of(Vec::new())),
                                    ("kind", ident("__bad")),
                                ]),
                            ),
                        ),
                    ));
                }
                SumCredential::Signature(z) => {
                    let mut variants = vec![("forged", SigVariant::Forged)];
                    if let Some(age) = z.stale_age() {
                        variants.push(("stale", SigVariant::Stale(age)));
                    }
                    for (bad, variant) in variants {
                        let mut stmts = vec![
                            z.sign(body, variant),
                            assign(auth(&z.seam.header), member(ident("__sig"), "signature")),
                        ];
                        if let Some(ts) = &z.seam.timestamp_header {
                            stmts.push(assign(auth(ts), member(ident("__sig"), "timestamp")));
                        }
                        set.push(if_(is(bad), block(stmts)));
                    }
                }
                SumCredential::Cookie(session) => set.extend(session.bad_cookie_stmts(&is)),
            }
            out.push(if_(
                strict_eq(ident("__as"), str_lit(name.as_str())),
                block(set),
            ));
        }
        out
    }

    /// #2864: the first Bearer member's secret read: what `by Forged` and
    /// `by Expired` sign a token with on a sum with no `Signature` member.
    pub(super) fn bearer(&self) -> Option<(&TsExpr, &bynk_check::actors::BearerClaims)> {
        self.members.iter().find_map(|(_, c)| match c {
            SumCredential::Bearer(secret_read, claims) => Some((secret_read, claims)),
            SumCredential::Signature(_) | SumCredential::Oidc(_) | SumCredential::Cookie(_) => None,
        })
    }

    /// The first `Signature` member: what `by Forged`/`by Stale` sign as.
    pub(super) fn signature(&self) -> Option<&SigDriver> {
        self.members.iter().find_map(|(_, c)| match c {
            SumCredential::Signature(z) => Some(z),
            SumCredential::Bearer(..) | SumCredential::Oidc(_) | SumCredential::Cookie(_) => None,
        })
    }
}

/// #2155: the credentials an actor-sum route's members present, each read against its own secret.
pub(super) fn sum_auth_for(
    (h, ns): (&bynk_syntax::ast::Handler, &str),
    actors: &HashMap<String, bynk_syntax::ast::ActorDecl>,
    secrets: &mut std::collections::BTreeSet<String>,
    secret_read_of: &dyn Fn(&str) -> TsExpr,
    (session_secrets, has_oidc): (&mut std::collections::BTreeSet<String>, &mut bool),
) -> Option<SumAuth> {
    let members: Vec<(String, SumCredential)> = bynk_check::actors::sum_members_for(h, actors)?
        .into_iter()
        .filter_map(|m| {
            let cred = match m.seam {
                bynk_check::actors::SumMemberSeam::None => return None,
                bynk_check::actors::SumMemberSeam::Bearer { secret, claims, .. } => {
                    secrets.insert(secret.clone());
                    SumCredential::Bearer(secret_read_of(&secret), claims)
                }
                bynk_check::actors::SumMemberSeam::Signature(seam) => {
                    secrets.insert(seam.secret.clone());
                    let secret_read = secret_read_of(&seam.secret);
                    SumCredential::Signature(SigDriver { seam, secret_read })
                }
                bynk_check::actors::SumMemberSeam::Oidc(seam) => {
                    *has_oidc = true;
                    let declared = object_of(vec![
                        ("issuer", str_lit(seam.issuer.clone())),
                        ("audience", str_lit(seam.audience.clone())),
                        ("jwks", str_lit(seam.jwks.clone())),
                    ]);
                    SumCredential::Oidc(match &seam.trust_from {
                        Some(var) => call(
                            ident("__bynkOidcTrustOf"),
                            vec![declared, secret_read_of(var)],
                        ),
                        None => declared,
                    })
                }
                bynk_check::actors::SumMemberSeam::SignedCookie(seam) => {
                    session_secrets.insert(seam.secret.clone());
                    SumCredential::Cookie(SessionDriver {
                        keys: TsExpr::object(vec![(
                            "secret".to_string(),
                            secret_read_of(&seam.secret),
                        )]),
                        actor: seam.actor.clone(),
                        cookie: seam.cookie.clone(),
                        ttl_secs: seam.ttl_secs,
                        sessions: SessionAgentRef::of(&seam, ns),
                    })
                }
            };
            Some((m.actor_name, cred))
        })
        .collect();
    (!members.is_empty()).then_some(SumAuth { members })
}

/// #3386, #4385: the bad-credential drivers of a route: a lone `SignedCookie` actor's (`by Forged`,
/// `by Expired`, `by Revoked`), and an actor sum's, one typed (and one raw, for a route with
/// params) that builds the bad credential of the member `__as` names, of the kind `__bad` names,
/// and decodes the seam's `401` to `Rejected(Unauthorized)`.
///
/// `typed` is the typed driver's params, URL, request options, body statements and the bytes it
/// signs, with the raw driver's params, URL, options and signed bytes when the route has params;
/// `request` is the route's handler, verb, content-type and `headers` entries, body param name and
/// body initialiser.
pub(super) fn push_credential_drivers(
    routes: &mut Vec<TsStmt>,
    (session, sum): (Option<&SessionDriver>, Option<&SumAuth>),
    (sname, key, binding, payload): (&str, &str, &str, &Vec<TsExpr>),
    (typed, raw): (TypedDriver<'_>, Option<RawDriver<'_>>),
    ((h, verb, content_type, headers_spread), body_init): (
        (&bynk_syntax::ast::Handler, &str, &Entries, &TsObjectEntry),
        &Entries,
    ),
    body_name: Option<&str>,
) {
    let (driver_params, url, options, body_stmts, body) = typed;
    if let Some(s) = session {
        let req = SessionRequest::new(verb, content_type, headers_spread, h);
        let raw_shape = raw.as_ref().map_or((&[][..], url), |r| (r.0, r.1));
        push_session_refusal_drivers(
            routes,
            s,
            (sname, key, binding, payload),
            ((driver_params, url), raw_shape),
            (&req, &body_stmts.to_vec(), body_init, body_name),
        );
    }
    let Some(sum) = sum else { return };
    let with_bad = |params: &[TsParam]| {
        let mut params = params.to_vec();
        params.push(TsParam::new(
            "__bad".to_string(),
            Some(TsType::named("string")),
        ));
        params
    };
    routes.push(sysdrive_driver(
        SysdriveKind::typed("sumbad"),
        sname,
        key,
        with_bad(driver_params),
        body_stmts.to_vec(),
        url.clone(),
        options,
        binding,
        "__responseToUnauthOutcome",
        payload.clone(),
        false,
        sum.bad_stmts(&body),
        true,
    ));
    if let Some((params, raw_url, raw_options, raw_body)) = raw {
        routes.push(sysdrive_driver(
            SysdriveKind::raw("sumbad"),
            sname,
            key,
            with_bad(params),
            Vec::new(),
            raw_url.clone(),
            raw_options,
            binding,
            "__responseToUnauthOutcome",
            payload.clone(),
            false,
            sum.bad_stmts(&raw_body),
            true,
        ));
    }
}

/// The typed driver of a route: params, URL, request options, body statements, signed bytes.
pub(super) type TypedDriver<'a> = (
    &'a Vec<TsParam>,
    &'a TsExpr,
    TsExpr,
    &'a Vec<TsStmt>,
    TsExpr,
);
/// The raw driver of a route: params, URL, request options, signed bytes.
pub(super) type RawDriver<'a> = (&'a Vec<TsParam>, &'a TsExpr, TsExpr, TsExpr);
