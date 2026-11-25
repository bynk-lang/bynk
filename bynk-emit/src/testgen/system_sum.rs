//! The credentials an actor-sum route's `system` drivers present: one per member, chosen per
//! call by the actor the call acts as (#2155), including an `Oidc` token and a session cookie
//! (#3936).

use super::session_driver::{SessionAgentRef, SessionDriver};
use super::*;
use bynk_ts::{TsBinaryOp, TsExpr, TsStmt, TsType};

/// #2155: an actor-sum route's per-call credential. The driver takes the
/// actor the call acts as (`__as`) and builds the matching credential: a
/// Bearer member's token, a `Signature` member's signature, or nothing for a
/// member with no scheme.
pub(super) struct SumAuth {
    /// Each member with a credential, in declaration order.
    pub(super) members: Vec<(String, SumCredential)>,
}

pub(super) enum SumCredential {
    /// A Bearer member: how the driver reads its secret.
    Bearer(TsExpr),
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
                SumCredential::Bearer(secret_read) => vec![assign(
                    auth("authorization"),
                    bearer_token(
                        object_of(vec![
                            ("sub", ident("__sub")),
                            ("exp", binary(TsBinaryOp::Add, now_secs(), num_lit("3600"))),
                        ]),
                        secret_read.clone(),
                    ),
                )],
                SumCredential::Oidc(trust) => vec![assign(
                    auth("authorization"),
                    oidc_token(
                        trust.clone(),
                        object_of(vec![
                            ("sub", ident("__sub")),
                            ("claims", object_of(Vec::new())),
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

    /// #2864: the first Bearer member's secret read: what `by Forged` and
    /// `by Expired` sign a token with on a sum with no `Signature` member.
    pub(super) fn bearer(&self) -> Option<&TsExpr> {
        self.members.iter().find_map(|(_, c)| match c {
            SumCredential::Bearer(secret_read) => Some(secret_read),
            SumCredential::Signature(_) | SumCredential::Oidc(_) | SumCredential::Cookie(_) => None,
        })
    }

    /// The first `Signature` member: what `by Forged`/`by Stale` sign as.
    pub(super) fn signature(&self) -> Option<&SigDriver> {
        self.members.iter().find_map(|(_, c)| match c {
            SumCredential::Signature(z) => Some(z),
            SumCredential::Bearer(_) | SumCredential::Oidc(_) | SumCredential::Cookie(_) => None,
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
                bynk_check::actors::SumMemberSeam::Bearer { secret, .. } => {
                    secrets.insert(secret.clone());
                    SumCredential::Bearer(secret_read_of(&secret))
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
