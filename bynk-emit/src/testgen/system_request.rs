//! The request a `system` http driver builds, as nodes: its header entries, URL, request-init
//! options object and the secret reads its credentials sign with (1B.5d).
//!
//! These were text spliced into one hand-shaped `{ method: …, headers: { … }, body: … }` line per
//! driver. Each is now a node built here once, so the drivers differ by which entries they carry.

use super::*;
use bynk_ts::{TsBinaryOp, TsExpr, TsObjectEntry, TsParam, TsStmt, TsType, TsTypeMember};

/// A list of request-init (or header) object entries.
pub(super) type Entries = Vec<TsObjectEntry>;

/// #3936, #3825: the `Sec-Fetch-Site: same-origin` a driver sends by default, as a browser's own page would:
/// where a sum has a cookie member, or the handler issues a session, whose route checks the request for
/// login CSRF. Otherwise `csrf_default`, the lone cookie route's own.
pub(super) fn default_csrf(
    csrf_default: Entries,
    sum: &Option<super::system_sum::SumAuth>,
    (h, actors): (
        &bynk_syntax::ast::Handler,
        &std::collections::HashMap<String, bynk_syntax::ast::ActorDecl>,
    ),
) -> Entries {
    let cookie_arm = sum.as_ref().is_some_and(|s| {
        s.members
            .iter()
            .any(|(_, c)| matches!(c, super::system_sum::SumCredential::Cookie(_)))
    });
    if cookie_arm || !bynk_check::actors::issued_cookie_actors(h, actors).is_empty() {
        vec![header("sec-fetch-site", str_lit("same-origin"))]
    } else {
        csrf_default
    }
}

/// `"key": value`: a request header, whose key is quoted as the request writes it.
pub(super) fn header(key: &str, value: TsExpr) -> TsObjectEntry {
    TsObjectEntry::prop(format!("{key:?}"), value)
}

/// `body: <value>`: a request-init `body` entry.
pub(super) fn body_entry(value: TsExpr) -> TsObjectEntry {
    TsObjectEntry::prop("body", value)
}

/// `...({ duplex: "half" } as object)`: what a streamed request body needs on Node. The spread
/// keeps it out of `RequestInit`'s excess-property check on lib versions that lack it.
pub(super) fn duplex_half() -> TsObjectEntry {
    TsObjectEntry::Spread(TsExpr::Paren(Box::new(TsExpr::As {
        expr: Box::new(TsExpr::object(vec![(
            "duplex".to_string(),
            str_lit("half"),
        )])),
        ty: TsType::named("object"),
    })))
}

/// [`request_headers_spread`] for a typed `headers` record (#3911): the argument is the record's
/// wire form (or `Wire` header text), written as headers by `__headerPairs`; `fields` is the
/// table of renamed fields, when any field is renamed.
pub(super) fn typed_headers_spread(fields: Option<TsExpr>) -> TsObjectEntry {
    let mut args = vec![ident("headers")];
    args.extend(fields);
    TsObjectEntry::Spread(call(ident("__headerPairs"), args))
}

/// The spread of the case's request headers: `...__requestHeaderPairs(headers)` for a handler
/// that declares a `headers` parameter, else the optional trailing `__extraHeaders` the case
/// sends (#2910).
pub(super) fn request_headers_spread(declares_headers: bool) -> TsObjectEntry {
    if declares_headers {
        TsObjectEntry::Spread(call(ident("__requestHeaderPairs"), vec![ident("headers")]))
    } else {
        TsObjectEntry::Spread(TsExpr::Paren(Box::new(cond_expr(
            ident("__extraHeaders"),
            call(
                ident("__requestHeaderPairs"),
                vec![ident("__extraHeaders"), bool_lit(true)],
            ),
            TsExpr::object(Vec::new()),
        ))))
    }
}

/// `{ method: "<method>", headers: { <headers> }, <body entries> }`: a driver's request init.
pub(super) fn request_init(method: &str, headers: Entries, body: Entries) -> TsExpr {
    let mut entries = vec![
        TsObjectEntry::prop("method", str_lit(method)),
        TsObjectEntry::prop("headers", TsExpr::object_entries(headers)),
    ];
    entries.extend(body);
    TsExpr::object_entries(entries)
}

/// The request URL `https://test<path><query>`: each `:name` segment of `path` is that
/// parameter's value, and `query` (a query string expression, or none) follows the path.
pub(super) fn request_url(path: &str, query: Option<TsExpr>) -> TsExpr {
    request_url_with(path, query, |name| ident(crate::emitter::ts_ident(name)))
}

/// [`request_url`] with each `:name` segment written by `param`, for a route whose parameter is
/// not text (#3910): an enum is in memory a `{ tag }`, written as its variant's name.
pub(super) fn request_url_with(
    path: &str,
    query: Option<TsExpr>,
    param: impl Fn(&str) -> TsExpr,
) -> TsExpr {
    let mut parts: Vec<String> = Vec::new();
    let mut exprs: Vec<TsExpr> = Vec::new();
    let mut current = String::from("https://test");
    let mut any = false;
    for seg in path.split('/').filter(|x| !x.is_empty()) {
        any = true;
        current.push('/');
        if let Some(name) = seg.strip_prefix(':') {
            parts.push(std::mem::take(&mut current));
            exprs.push(param(name));
        } else {
            current.push_str(seg);
        }
    }
    if !any {
        current.push('/');
    }
    if let Some(q) = query {
        parts.push(std::mem::take(&mut current));
        exprs.push(q);
    }
    parts.push(current);
    TsExpr::template_lit(parts, exprs)
}

/// How a driver reads an actor's secret named `secret`: the case's `env` entry (#2134) when the
/// module holds them, else the fixed test secret in `process.env` (a remote run's values come
/// from the runner in one JSON variable, `__bynkSecrets`, #2138), and `""` when neither is set:
/// `(__bynkEnv["K"] ?? (__bynkSecrets["K"] ?? process…) ?? "")`.
pub(super) fn secret_read(secret: &str, with_env: bool, remote: bool) -> TsExpr {
    let env_ty = TsType::Object(vec![TsTypeMember::optional_prop(
        "env",
        TsType::named_with_args(
            "globalThis.Record",
            vec![TsType::named("string"), TsType::named("string")],
        ),
    )]);
    let process = TsExpr::OptionalMember {
        object: Box::new(member(
            TsExpr::Paren(Box::new(TsExpr::As {
                expr: Box::new(ident("globalThis")),
                ty: TsType::Object(vec![TsTypeMember::optional_prop("process", env_ty)]),
            })),
            "process",
        )),
        property: "env".to_string(),
    };
    let process_read = TsExpr::OptionalIndex {
        object: Box::new(process),
        index: Box::new(str_lit(secret)),
    };
    let nullish = |l: TsExpr, r: TsExpr| binary(TsBinaryOp::NullishCoalescing, l, r);
    let keyed = |table: &str| index(ident(table), str_lit(secret));
    // #2138: a remote run's secret values come from the runner in one JSON variable.
    let process_read = if remote {
        TsExpr::Paren(Box::new(nullish(keyed("__bynkSecrets"), process_read)))
    } else {
        process_read
    };
    if with_env {
        TsExpr::Paren(Box::new(nullish(
            nullish(keyed("__bynkEnv"), process_read),
            str_lit(""),
        )))
    } else {
        TsExpr::Paren(Box::new(nullish(process_read, str_lit(""))))
    }
}

/// `{ key: value, … }` with plain keys.
pub(super) fn object_of(entries: Vec<(&str, TsExpr)>) -> TsExpr {
    TsExpr::object(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

/// #3821: how a Bearer actor's test tokens get their `iss` and `aud`: the declared values, and, with
/// `trustFrom`, the expression that reads the variable the case's `env` (or the process) may set, so a
/// token passes the seam that reads the same variable.
#[derive(Clone)]
pub(super) struct BearerMint {
    pub(super) claims: bynk_check::actors::BearerClaims,
    trust: Option<TsExpr>,
}

impl BearerMint {
    pub(super) fn of(
        claims: bynk_check::actors::BearerClaims,
        secret_read_of: &dyn Fn(&str) -> TsExpr,
    ) -> Self {
        let trust = claims.trust_from.as_deref().map(secret_read_of);
        BearerMint { claims, trust }
    }
}

/// #3821: the `iss` and `aud` a token minted for the actor carries, so a valid token passes its seam.
/// `wrong` (`"iss"` or `"aud"`) names the one claim that instead names another party. An actor that
/// declares none adds none.
fn bearer_claim_entries_with(
    mint: &BearerMint,
    wrong: Option<&str>,
) -> Vec<bynk_ts::TsObjectEntry> {
    let claims = &mint.claims;
    let Some(read) = &mint.trust else {
        return [
            ("iss", &claims.issuer, "iss"),
            ("aud", &claims.audience, "aud"),
        ]
        .into_iter()
        .filter_map(|(k, v, key)| {
            let v = v.as_ref()?;
            let v = if wrong == Some(key) {
                [v.as_str(), "-not-the-expected"].concat()
            } else {
                v.clone()
            };
            Some(bynk_ts::TsObjectEntry::prop(k.to_string(), str_lit(v)))
        })
        .collect();
    };
    let lit = |v: &Option<String>| v.clone().map(str_lit).unwrap_or_else(super::names::absent);
    let which = wrong.map(str_lit).unwrap_or_else(super::names::absent);
    vec![bynk_ts::TsObjectEntry::Spread(call(
        ident("__bynkBearerClaimsOf"),
        vec![
            object_of(vec![
                ("iss", lit(&claims.issuer)),
                ("aud", lit(&claims.audience)),
            ]),
            read.clone(),
            which,
        ],
    ))]
}

pub(super) fn bearer_claim_entries(mint: &BearerMint) -> Vec<bynk_ts::TsObjectEntry> {
    bearer_claim_entries_with(mint, None)
}

/// #3821: the claims of a `by Forged` or `by Expired` Bearer token: a valid `sub`, the `exp` that makes
/// it expired (or an hour ahead), and the actor's declared `iss`/`aud`, so only the signature or the
/// expiry is wrong.
pub(super) fn bearer_bad_claims(exp: TsExpr, mint: &BearerMint) -> TsExpr {
    TsExpr::object_entries(
        [
            bynk_ts::TsObjectEntry::prop("sub".to_string(), str_lit("bynk-test")),
            bynk_ts::TsObjectEntry::prop("exp".to_string(), exp),
        ]
        .into_iter()
        .chain(bearer_claim_entries(mint))
        .collect(),
    )
}

/// #3821: the claims of a `by WrongIssuer` or `by WrongAudience` Bearer token: correctly signed and
/// unexpired, with the declared `iss` and `aud` except the one under test, which names another party.
pub(super) fn bearer_wrong_claims(exp: TsExpr, mint: &BearerMint, wrong_issuer: bool) -> TsExpr {
    let mut entries = vec![
        bynk_ts::TsObjectEntry::prop("sub".to_string(), str_lit("bynk-test")),
        bynk_ts::TsObjectEntry::prop("exp".to_string(), exp),
    ];
    entries.extend(bearer_claim_entries_with(
        mint,
        Some(if wrong_issuer { "iss" } else { "aud" }),
    ));
    TsExpr::object_entries(entries)
}

/// #2864, #3821: the bad Bearer tokens a route's drivers present, as `(driver, secret, exp sign)`:
/// `Expired` (the real secret, an `exp` an hour past), `Forged` (a derived wrong secret, unless the route
/// has a `Signature` seam that takes the name), and, where the actor declares an `issuer` or an `audience`,
/// `WrongIssuer` and `WrongAudience` (the real secret, a future `exp`, another party's claim).
pub(super) fn bearer_variants(
    secret_read: &TsExpr,
    claims: &bynk_check::actors::BearerClaims,
    forged: bool,
) -> Vec<(&'static str, TsExpr, TsBinaryOp)> {
    let real = || TsExpr::Paren(Box::new(secret_read.clone()));
    let mut variants = vec![("expired", real(), TsBinaryOp::Sub)];
    if forged {
        let wrong = binary(TsBinaryOp::Add, secret_read.clone(), str_lit("!"));
        variants.push(("forged", TsExpr::Paren(Box::new(wrong)), TsBinaryOp::Add));
    }
    if claims.issuer.is_some() {
        variants.push(("wrongissuer", real(), TsBinaryOp::Add));
    }
    if claims.audience.is_some() {
        variants.push(("wrongaudience", real(), TsBinaryOp::Add));
    }
    variants
}

/// #3821: the claims of the bad Bearer token a driver `name` presents.
pub(super) fn bearer_variant_claims(name: &str, exp: TsExpr, mint: &BearerMint) -> TsExpr {
    match name {
        "wrongissuer" => bearer_wrong_claims(exp, mint, true),
        "wrongaudience" => bearer_wrong_claims(exp, mint, false),
        _ => bearer_bad_claims(exp, mint),
    }
}

/// `` `Bearer ${await __bynkSignHs256(<claims>, <secret>)}` ``: a signed Bearer token.
pub(super) fn bearer_token(claims: TsExpr, secret: TsExpr) -> TsExpr {
    TsExpr::template_lit(
        vec!["Bearer ".to_string(), String::new()],
        vec![await_expr(call(
            ident("__bynkSignHs256"),
            vec![claims, secret],
        ))],
    )
}

/// `` `Bearer ${await __bynkOidcToken(__bynkOidcRemote, <trust>, <token>)}` ``: a token minted by
/// the test issuer.
pub(super) fn oidc_token(trust: TsExpr, token: TsExpr) -> TsExpr {
    TsExpr::template_lit(
        vec!["Bearer ".to_string(), String::new()],
        vec![await_expr(call(
            ident("__bynkOidcToken"),
            vec![ident("__bynkOidcRemote"), trust, token],
        ))],
    )
}

/// `any`: the type of a driver parameter that forwards its value to a serialiser, which the
/// checker has already typed (the one spelling, as the harness has no better one for it).
pub(super) fn any_type() -> TsType {
    TsType::named("any")
}

/// #707: one generic wrong-method driver per service — drives an arbitrary `(method, path)` (an
/// existing path, an undeclared method) and decodes the router's `405` fall-through to
/// `Rejected(MethodNotAllowed)`. The handler never runs, so there is no body to serialise and the
/// payload deserialiser is unused (the `405` takes the `Rejected` arm).
pub(super) fn wrong_method_driver(sname: &str, binding: &str) -> TsStmt {
    let string = || Some(TsType::named("string"));
    let decode = arrow(
        vec![TsParam::new("__j", Some(TsType::named("__JsonValue")))],
        call(
            ident("Ok"),
            vec![as_expr(ident("__j"), TsType::named("never"))],
        ),
    );
    async_driver(
        wrong_method_driver_name(sname),
        vec![
            TsParam::new("method", string()),
            TsParam::new("path", string()),
        ],
        vec![
            harness_const(),
            const_(
                "__req",
                new(
                    ident("globalThis.Request"),
                    vec![
                        TsExpr::template_lit(
                            vec!["https://test".to_string(), String::new()],
                            vec![ident("path")],
                        ),
                        TsExpr::object_entries(vec![TsObjectEntry::Shorthand(
                            "method".to_string(),
                        )]),
                    ],
                ),
            ),
            const_(
                "__res",
                await_expr(method_call(
                    member(member(ident("__h"), "env"), binding),
                    "fetch",
                    vec![ident("__req")],
                )),
            ),
            ret(call(
                ident("__responseToHttpOutcome"),
                vec![ident("__res"), decode],
            )),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn printed(e: &TsExpr) -> String {
        bynk_ts::print_expr(e)
    }

    const PROCESS: &str = "(globalThis as { process?: { env?: globalThis.Record<string, string> } }).process?.env?.[\"K\"]";

    /// The three shapes of a secret read: the fixed test secret, the case's `env` entry before
    /// it, and (a remote run) the runner's `__bynkSecrets` between the two.
    #[test]
    fn a_secret_read_is_the_env_then_the_runner_then_the_process() {
        assert_eq!(
            printed(&secret_read("K", false, false)),
            format!("({PROCESS} ?? \"\")")
        );
        assert_eq!(
            printed(&secret_read("K", true, false)),
            format!("(__bynkEnv[\"K\"] ?? {PROCESS} ?? \"\")")
        );
        assert_eq!(
            printed(&secret_read("K", false, true)),
            format!("((__bynkSecrets[\"K\"] ?? {PROCESS}) ?? \"\")")
        );
        assert_eq!(
            printed(&secret_read("K", true, true)),
            format!("(__bynkEnv[\"K\"] ?? (__bynkSecrets[\"K\"] ?? {PROCESS}) ?? \"\")")
        );
    }

    /// A route's URL: each `:name` is that parameter's value, the root path is `/`, and a query
    /// string follows the path.
    #[test]
    fn a_request_url_substitutes_params_and_appends_the_query() {
        assert_eq!(printed(&request_url("/", None)), "`https://test/`");
        assert_eq!(
            printed(&request_url("/orders/:id", None)),
            "`https://test/orders/${id}`"
        );
        assert_eq!(
            printed(&request_url("/", Some(ident("q")))),
            "`https://test/${q}`"
        );
        assert_eq!(
            printed(&request_url("/a/:b/c", Some(ident("q")))),
            "`https://test/a/${b}/c${q}`"
        );
    }
}
