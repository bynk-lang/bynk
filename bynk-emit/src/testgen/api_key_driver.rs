//! #3939 (the API keys track, §10): a `system` driver's key for a route an `ApiKey` actor guards.
//!
//! `by Client("acct-7")` presents a real key: the harness mints it into the participant's
//! in-process key store, under the run's pepper, once per identity, through the same
//! `<Actor>KeyOwner.mint` a handler's `Client.mint` reaches, so the real seam verifies it.
//!
//! The failure principals present a key built from a real one (`__apiKeyBadKey`):
//!
//! - `by Malformed`: a bad checksum, refused before any lookup, so the store compares nothing;
//! - `by Unknown`: a key number its store never held, then a handle no store holds, each compared
//!   once against the dummy digest (D3 B);
//! - `by Forged`: the real id with a secret the store does not hold, compared once against the
//!   record.
//!
//! Each principal's driver counts the store's comparisons (`__apiKeyCompares`, a counter over the
//! platform `Crypto` binding the in-process agents construct) and fails the case unless the seam
//! did exactly that work, so "the same work for an unknown key as for a wrong one" is pinned by the
//! harness, not by timing.

use bynk_ts::{TsBinaryOp, TsDecl, TsExpr, TsParam, TsStmt, TsType};

use super::system_jobs::async_driver;
use super::system_request::{Entries, header, request_init};
use super::test_main::sysdrive_driver;
use crate::emitter::harness_names::{SysdriveKind, sysdrive_name};
use crate::ts_build::{
    arrow_with, await_expr, binary, call, const_, ident, member, method_call, num_lit, ret,
    str_lit, strict_neq,
};

/// How a driver reaches an `ApiKey` actor's key store and presents a key.
pub(super) struct ApiKeyDriver {
    /// The participant's handlers namespace.
    ns: String,
    /// The participant's `env` field of the memoised harness.
    env: String,
    actor: String,
    prefix: String,
    /// The header the key goes in, or `None` for `Authorization: Bearer`.
    header: Option<String>,
    secret: String,
    /// The pepper as the harness reads it (a case's `env` entry, else the run's test pepper).
    secret_read: TsExpr,
}

impl ApiKeyDriver {
    /// The driver of `h`'s lone `ApiKey` actor in participant `ns`, or `None`. Its pepper variable
    /// joins `keyed_secrets`, which the harness sets to a 32-byte test key.
    pub(super) fn of(
        h: &bynk_syntax::ast::Handler,
        actors: &std::collections::HashMap<String, bynk_syntax::ast::ActorDecl>,
        ns: &str,
        secret_read_of: &dyn Fn(&str) -> TsExpr,
        keyed_secrets: &mut std::collections::BTreeSet<String>,
    ) -> Option<Self> {
        let seam = bynk_check::api_key::api_key_seam_for(h, actors)?;
        keyed_secrets.insert(seam.secret.clone());
        Some(Self {
            ns: ns.to_string(),
            env: crate::testgen::names::env_binding(ns),
            secret_read: secret_read_of(&seam.secret),
            actor: seam.actor,
            prefix: seam.prefix,
            header: seam.header,
            secret: seam.secret,
        })
    }

    /// The header entry carrying `key`: `authorization: Bearer <key>`, or the actor's header.
    fn header_entry(&self, key: TsExpr) -> bynk_ts::TsObjectEntry {
        match &self.header {
            Some(h) => header(&h.to_ascii_lowercase(), key),
            None => header(
                "authorization",
                TsExpr::template_lit(vec!["Bearer ".to_string(), String::new()], vec![key]),
            ),
        }
    }

    /// `await __apiKeyForTest(…)`: `sub`'s real key, minted once per harness into the store.
    fn real_key(&self, sub: TsExpr) -> TsExpr {
        let env = member(call(ident("__makeHarness"), Vec::new()), self.env.clone());
        let factory = crate::emitter::agent_factory_name(
            &bynk_check::api_key::key_owner_agent_name(&self.actor),
        );
        let req = |f: &str| member(ident("__req"), f);
        let now = binary(
            TsBinaryOp::Mul,
            call(ident("__bynkNow"), Vec::new()),
            num_lit(1000),
        );
        let store = arrow_with(
            vec![TsParam::new("__req".to_string(), None)],
            true,
            method_call(
                call(
                    member(ident(self.ns.clone()), factory),
                    vec![
                        crate::ts_build::as_expr(sub.clone(), TsType::named("never")),
                        env.clone(),
                    ],
                ),
                "mint",
                vec![
                    req("handle"),
                    req("keyNo"),
                    req("digest"),
                    req("kid"),
                    str_lit("bynk-test"),
                    now.clone(),
                    num_lit(bynk_project::api_key_agents::KEY_CAP),
                    // `KeyOwner.mint` calls the key store, so its `deps` carry the `env`.
                    TsExpr::object(vec![("env".to_string(), env.clone())]),
                ],
            ),
        );
        await_expr(call(
            ident("__apiKeyForTest"),
            vec![
                env,
                str_lit(self.actor.clone()),
                sub,
                TsExpr::object(vec![("secret".to_string(), self.secret_read.clone())]),
                str_lit(self.secret.clone()),
                str_lit(self.prefix.clone()),
                now,
                store,
            ],
        ))
    }

    /// The header `by <Actor>(sub)` sends: `sub`'s real key.
    pub(super) fn principal_header(&self, sub: TsExpr) -> bynk_ts::TsObjectEntry {
        self.header_entry(self.real_key(sub))
    }
}

/// A route's request shape, bar the credential: the typed driver's params, URL, body statements
/// and body, the raw driver's params, URL and body when the route takes params, and the verb,
/// content-type and `headers` entries.
pub(super) struct KeyRoute<'a> {
    pub(super) typed: (&'a Vec<TsParam>, &'a TsExpr, &'a Vec<TsStmt>, &'a Entries),
    pub(super) raw: Option<(&'a Vec<TsParam>, &'a TsExpr, &'a Entries)>,
    pub(super) request: (&'a str, &'a Entries, &'a bynk_ts::TsObjectEntry),
}

/// The bad keys, each with the store comparisons the seam must make for it: every one, and those
/// against the dummy digest.
const BAD_KEYS: [(&str, (i64, i64)); 4] = [
    ("malformed", (0, 0)),
    ("unknownkey", (1, 1)),
    ("unknownhandle", (1, 1)),
    ("forged", (1, 0)),
];

/// The principals and the bad keys each presents in turn.
const PRINCIPALS: [(&str, &str, &[&str]); 3] = [
    ("malformed", "Malformed", &["malformed"]),
    ("unknown", "Unknown", &["unknownkey", "unknownhandle"]),
    ("forged", "Forged", &["forged"]),
];

/// The failure drivers of an `ApiKey` route: one per bad key (typed, and raw when the route takes
/// params), each decoding the seam's `401` to `Rejected(Unauthorized)`, and one per principal that
/// presents its keys in turn, checks the store's comparisons after each, and returns the first
/// outcome that is not the seam's refusal (else that refusal).
pub(super) fn push_api_key_drivers(
    routes: &mut Vec<TsStmt>,
    driver: &ApiKeyDriver,
    (sname, key, binding, payload): (&str, &str, &str, &Vec<TsExpr>),
    route: &KeyRoute<'_>,
) {
    let (verb, content_type, headers_spread) = route.request;
    let options = |body: &Entries| {
        let mut headers: Entries = vec![headers_spread.clone()];
        headers.extend(content_type.iter().cloned());
        headers.push(driver.header_entry(ident("__key")));
        request_init(verb, headers, body.clone())
    };
    for (bad, _) in BAD_KEYS {
        let name = format!("key{bad}");
        let sign = const_(
            "__key",
            call(
                ident("__apiKeyBadKey"),
                vec![
                    str_lit(bad),
                    driver.real_key(str_lit("bynk-test")),
                    str_lit(driver.prefix.clone()),
                ],
            ),
        );
        let (params, url, body_stmts, body) = route.typed;
        routes.push(sysdrive_driver(
            SysdriveKind::typed(&name),
            sname,
            key,
            params.clone(),
            body_stmts.clone(),
            url.clone(),
            options(body),
            binding,
            "__responseToUnauthOutcome",
            payload.clone(),
            false,
            vec![sign.clone()],
            false,
        ));
        if let Some((params, url, body)) = route.raw {
            routes.push(sysdrive_driver(
                SysdriveKind::raw(&name),
                sname,
                key,
                params.clone(),
                Vec::new(),
                url.clone(),
                options(body),
                binding,
                "__responseToUnauthOutcome",
                payload.clone(),
                false,
                vec![sign],
                false,
            ));
        }
    }
    let prefixes: &[bool] = if route.raw.is_some() {
        &[false, true]
    } else {
        &[false]
    };
    for (principal, written, keys) in PRINCIPALS {
        for &raw in prefixes {
            routes.push(principal_driver(
                (sname, key),
                raw,
                (principal, written),
                keys,
            ));
        }
    }
}

/// One principal's driver: each of its keys' drivers in turn, each checked for the comparisons the
/// seam must make, returning the first outcome that is not the `401` (else the last refusal).
fn principal_driver(
    (sname, key): (&str, &str),
    raw: bool,
    (principal, written): (&str, &str),
    keys: &[&str],
) -> TsStmt {
    let driver = |kind: &str| sysdrive_name(SysdriveKind { raw, kind }, sname, key);
    let first = driver(&format!("key{}", keys[0]));
    let mut body = Vec::new();
    for (i, bad) in keys.iter().enumerate() {
        let (all, dummy) = BAD_KEYS
            .iter()
            .find(|(k, _)| k == bad)
            .map(|(_, w)| *w)
            .expect("every principal's key is a bad key");
        let out = format!("__out{i}");
        let before = format!("__before{i}");
        body.push(const_(
            &before,
            TsExpr::object_entries(vec![bynk_ts::TsObjectEntry::Spread(ident(
                "__apiKeyCompares",
            ))]),
        ));
        body.push(const_(
            &out,
            await_expr(call(
                ident(driver(&format!("key{bad}"))),
                vec![TsExpr::Spread(Box::new(ident("__a")))],
            )),
        ));
        body.push(crate::ts_build::expr_stmt(call(
            ident("__apiKeyExpectCompares"),
            vec![
                ident("__apiKeyCompares"),
                ident(before),
                TsExpr::object(vec![
                    ("all".to_string(), num_lit(all)),
                    ("dummy".to_string(), num_lit(dummy)),
                ]),
                str_lit(written),
            ],
        )));
        let admitted = binary(
            TsBinaryOp::Or,
            strict_neq(member(ident(out.clone()), "tag"), str_lit("Rejected")),
            strict_neq(
                member(member(ident(out.clone()), "value"), "tag"),
                str_lit("Unauthorized"),
            ),
        );
        if i + 1 < keys.len() {
            body.push(crate::ts_build::if_(admitted, ret(ident(out))));
        } else {
            body.push(ret(ident(out)));
        }
    }
    async_driver(
        driver(principal),
        vec![TsParam {
            rest: true,
            ..TsParam::new(
                "__a",
                Some(TsType::named_with_args(
                    "Parameters",
                    vec![TsType::type_of(ident(first))],
                )),
            )
        }],
        body,
    )
}

/// Whether the suite's target declares an `ApiKey` actor.
pub(super) fn suite_has_api_key(table: Option<&bynk_check::symbols::UnitTable>) -> bool {
    table.is_some_and(|t| t.actors.values().any(bynk_check::api_key::is_api_key_actor))
}

/// The module preamble a suite whose target has an `ApiKey` route needs: the platform `Crypto`
/// binding the in-process agents construct, and the counter over its comparisons.
pub(super) fn preamble(binding_specifier: String) -> Vec<TsStmt> {
    vec![
        TsStmt::decl(
            TsDecl::ImportNamespace {
                type_only: false,
                alias: "__bynkApiKeyCryptoBinding".to_string(),
                from: binding_specifier,
            },
            None,
        ),
        const_(
            "__apiKeyCompares",
            call(
                ident("__apiKeyCountCompares"),
                vec![member(ident("__bynkApiKeyCryptoBinding"), "CryptoProvider")],
            ),
        ),
    ]
}
