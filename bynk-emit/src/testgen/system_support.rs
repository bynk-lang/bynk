//! System-suite support: HTTP and job drivers, signing, service forwarding and fan-out.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use crate::emitter::RuntimeUse;
// #1759: every namespace a test module imports, from one definition.
use crate::emitter::emit::test_scaffold_ns as test_ns;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt, TsType, TsTypeMember,
};

/// testing-the-boundary Slice B: for a `system` suite whose target exposes an
/// `http` service, emit a **test-only** HS256 signer and one **driver** per
/// route. A driver builds a real `Request` (concrete path, JSON body, a signed
/// `Authorization: Bearer …`), drives the target Worker's public `fetch`, and
/// decodes the `Response` back to `HttpResult[T]`. The developer's `by
/// User("bob")` supplies the `sub`; the framework signs it (the developer never
/// hand-crafts auth — real-token ceremony is an e2e concern), and the real,
/// unmodified emitted Worker verifies it. Returns the emitted TS plus the set of
/// the target's http service names, so the case-body lowering knows to call a
/// driver (`__sysdrive_<svc>_<key>`) rather than `callService`.
/// The declared `(service, method, path)` http routes of a system target — the
/// set the lowering checks to tell a normal call from a **wrong-method** call
/// (#707): a `(method, path)` not in the set, whose *path* is, drives the `405`
/// fall-through through the generic `__sysdrive_wrongmethod_<svc>` driver.
pub(super) type DeclaredRoutes = std::collections::HashSet<(String, String, String)>;
/// #708: for each declared route with a body param, the body's zero-based
/// position among the route's positional call args and its declared type —
/// what the raw driver's mixed typed+`Wire` call-site lowering needs to
/// serialise a typed body into the raw driver's `string` slot.
pub(super) type RouteBodyMap =
    HashMap<(String, String, String), (usize, bynk_syntax::ast::TypeRef)>;

/// The emitted `emit_system_http_support` output: the driver/signer TS
/// source plus the metadata the case-body lowering needs to route and
/// convert calls.
///
/// #1479: `code` is now real [`TsStmt`]s (was pre-printed `String`) — the
/// HS256 signer block (`__bynkNow`/`__b64url`/`__bytesB64url`/
/// `__bynkSignHs256`) is committed `.ts` source (`runtime/src/testing/
/// jwt_signer.ts`, #1485), spliced in via [`TestRuntimeModule::splice`] and
/// carried as one [`bynk_ts::TsStmtKind::Raw`] statement, the same carrier
/// this track's earlier still-`String`-typed-sibling conversions already
/// used.
pub(super) struct SystemHttpSupport {
    pub(super) code: Vec<TsStmt>,
    pub(super) http_services: std::collections::HashSet<String>,
    pub(super) declared_routes: DeclaredRoutes,
    /// #708: per-route body-param position/type, for the raw driver's
    /// mixed typed+`Wire` call-site conversion.
    pub(super) route_body: RouteBodyMap,
    /// The target's type namespace (`<target>.`), `route_body`'s types
    /// resolve `serialise_*` calls through.
    pub(super) type_ns: String,
    /// #2019: the refinement-guarded routes and each refinement actor's
    /// witness claims, so a call acting as a refinement signs its claims.
    pub(super) claims: emitter::ctx::SystemHttpClaims,
}

/// #2135: what a `Signature`-secured route's drivers sign with — the actor's
/// declared seam and how the driver reads its secret (the case's `env` entry,
/// else the fixed test secret in `process.env`).
pub(super) struct SigDriver {
    pub(super) seam: bynk_check::actors::SignatureSeam,
    pub(super) secret_read: TsExpr,
}

/// #2135: which signature a driver sends.
#[derive(Clone, Copy)]
pub(super) enum SigVariant {
    /// Signed now, with the real secret.
    Signed,
    /// Signed now, with a secret the seam does not hold.
    Forged,
    /// Signed with the real secret, this many seconds in the past.
    Stale(i64),
}

impl SigDriver {
    pub(super) fn signed(&self) -> SigVariant {
        SigVariant::Signed
    }

    /// `tolerance + 60` seconds, when the actor declares a timestamp header
    /// and a tolerance: the age of a `by Stale` signature.
    pub(super) fn stale_age(&self) -> Option<i64> {
        self.seam.timestamp_header.as_ref()?;
        Some(self.seam.tolerance_secs? + 60)
    }

    /// The request headers carrying the signature (and timestamp).
    pub(super) fn headers(&self) -> Entries {
        let mut out = vec![header(
            &self.seam.header,
            member(ident("__sig"), "signature"),
        )];
        if let Some(ts) = &self.seam.timestamp_header {
            out.push(header(ts, member(ident("__sig"), "timestamp")));
        }
        out
    }

    /// `const __sig = await __bynkSignHmacSha256(<body>, <secret>, …);`.
    pub(super) fn sign(&self, body: &TsExpr, variant: SigVariant) -> TsStmt {
        let (secret, age) = match variant {
            SigVariant::Signed => (self.secret_read.clone(), 0),
            // #2155: a wrong secret derived from the real one, so it differs
            // from whatever the seam holds, `env` entry or test default.
            SigVariant::Forged => (
                TsExpr::Paren(Box::new(binary(
                    TsBinaryOp::Add,
                    self.secret_read.clone(),
                    str_lit("!"),
                ))),
                0,
            ),
            SigVariant::Stale(age) => (self.secret_read.clone(), age),
        };
        const_(
            "__sig",
            await_expr(call(
                ident("__bynkSignHmacSha256"),
                vec![
                    body.clone(),
                    secret,
                    TsExpr::Lit(TsLit::Bool(self.seam.timestamp_header.is_some())),
                    num_lit(age.to_string()),
                ],
            )),
        )
    }
}

/// #2135, #2155: a signed driver's signing statements — a lone `Signature`
/// actor's signature, or a sum's per-call `__auth` — over `body`.
pub(super) fn signing_stmts(
    lone: &Option<SigDriver>,
    sum: &Option<SumAuth>,
    body: &TsExpr,
) -> Vec<TsStmt> {
    match (lone, sum) {
        (Some(z), _) => vec![z.sign(body, z.signed())],
        (None, Some(sum)) => sum.stmts(body),
        (None, None) => Vec::new(),
    }
}

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
            SumCredential::Signature(_) => None,
        })
    }

    /// The first `Signature` member: what `by Forged`/`by Stale` sign as.
    pub(super) fn signature(&self) -> Option<&SigDriver> {
        self.members.iter().find_map(|(_, c)| match c {
            SumCredential::Signature(z) => Some(z),
            SumCredential::Bearer(_) => None,
        })
    }
}

pub(super) fn emit_system_http_support(
    target: &str,
    unit_tables: &HashMap<String, UnitTable>,
    runtime_use: &RuntimeUse,
    // #2134: whether the module holds the running case's `env` entries
    // (`__bynkEnv`), whose entry for the actor's secret is signed with first.
    with_env: bool,
    // #2138: a remote run signs with the secrets the runner's environment
    // holds, so the fixed test secrets are not assigned over them.
    remote: bool,
    // #2685: whether the typed drivers name their route to the decoder.
    smoke: bool,
) -> SystemHttpSupport {
    use bynk_model::{IrHandlerKind, lower_handler_kind_ir};
    use bynk_syntax::ast::ServiceProtocol;
    let mut http_services: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut declared: DeclaredRoutes = std::collections::HashSet::new();
    let mut route_body: RouteBodyMap = HashMap::new();
    let Some(table) = unit_tables.get(target) else {
        return SystemHttpSupport {
            code: Vec::new(),
            http_services,
            declared_routes: declared,
            route_body,
            type_ns: String::new(),
            claims: emitter::ctx::SystemHttpClaims::default(),
        };
    };
    let ns = test_ns(target);
    let binding = crate::emitter::bindings::service_binding(target).name;
    let type_ns = ns_qualifier(&ns);

    let mut routes: Vec<TsStmt> = Vec::new();
    let mut secrets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // #2135: whether some route is `Signature`-secured, so the module needs
    // the HMAC signer.
    let mut signs_hmac = false;
    let mut session_secrets = std::collections::BTreeSet::new();
    // #3052: whether some route is `Oidc`-secured, so the module needs the
    // test issuer.
    let mut has_oidc = false;
    // #2019: each refinement actor's witness claims, signed when a case acts
    // as it on a refinement-guarded route.
    let mut claims = emitter::ctx::SystemHttpClaims {
        routes: std::collections::HashSet::new(),
        sum_routes: HashMap::new(),
        oidc_routes: std::collections::HashSet::new(),
        actors: table
            .actors
            .iter()
            .filter_map(|(name, decl)| {
                let pred =
                    bynk_check::actors::parse_claim_predicate(&decl.refinement.as_ref()?.predicate)
                        .ok()?;
                Some((name.clone(), bynk_check::actors::claim_witness(&pred)?))
            })
            .collect(),
    };

    // How a driver reads an actor's secret: the case's `env` entry (#2134),
    // else the fixed test secret in `process.env`.
    let secret_read_of = |secret: &str| secret_read(secret, with_env, remote);
    let mut svc_names: Vec<&String> = table.services.keys().collect();
    svc_names.sort();
    for sname in svc_names {
        let decl = &table.services[sname];
        // M-1's domain clause (ADR 0735; crate-hygiene B4, #3012): this
        // function holds the target's symbol table, not its `TypedCommons`, so it
        // doesn't hold `bynk_model::lower_protocol_ir`'s input.
        if !matches!(decl.protocol, ServiceProtocol::Http) {
            continue;
        }
        http_services.insert(sname.clone());
        for h in &decl.handlers {
            let IrHandlerKind::Http { method, path } = lower_handler_kind_ir(&h.kind) else {
                continue;
            };
            declared.insert((sname.clone(), method.as_str().to_string(), path.clone()));
            let key = crate::emitter::http_handler_method_name_ir(method, &path);
            // Split the handler's params into path params (matching `:name` in
            // the pattern, in order) and the optional body (the remaining param).
            let path_params: Vec<&str> = path
                .split('/')
                .filter_map(|seg| seg.strip_prefix(':'))
                .collect();
            // #2341 (L4): a `headers: RequestHeaders` param is neither: the
            // drivers send it as the request's headers.
            let is_headers = |p: &&bynk_syntax::ast::Param| {
                p.name.name == "headers"
                    && matches!(p.type_ref, bynk_syntax::ast::TypeRef::RequestHeaders(_))
            };
            // #2910: a route with no `headers:` param still takes the case's
            // request headers, as an optional trailing driver parameter. They
            // are spread first, so a header the driver sets itself wins.
            let declares_headers = h.params.iter().any(|p| is_headers(&p));
            let headers_spread = request_headers_spread(declares_headers);
            // A required slot ahead of `sub`: the call lowers `undefined` there
            // when the case sends none.
            let extra_headers_param = (!declares_headers).then(|| {
                declared.insert((
                    sname.clone(),
                    method.as_str().to_string(),
                    route_with(&path, emitter::EXTRA_HEADERS_ROUTE_SUFFIX),
                ));
                TsParam::new(
                    "__extraHeaders".to_string(),
                    Some(TsType::union(vec![
                        crate::emitter::ts_types::ts_type_of(
                            &bynk_syntax::ast::TypeRef::RequestHeaders(Default::default()),
                        ),
                        TsType::named("undefined"),
                    ])),
                )
            });
            // #2854: a `query` param (unless a `:query` path segment binds the
            // name) is neither: the drivers append it to the request's URL, as
            // the raw `RequestQuery` view or as a typed record's query string.
            let is_query = |p: &&bynk_syntax::ast::Param| {
                p.name.name == "query" && !path_params.contains(&"query")
            };
            // #3277: a `cookies` param is neither: the drivers send it as the request's
            // `Cookie` header, from a typed record or a `Wire` string.
            let is_cookies = |p: &&bynk_syntax::ast::Param| {
                p.name.name == "cookies" && !path_params.contains(&"cookies")
            };
            // #3513 C: nor is a `cf: RequestCf` param: the request is real here, so it
            // carries whatever the platform supplies, and the case's argument is unused.
            let is_cf = |p: &&bynk_syntax::ast::Param| {
                p.name.name == "cf"
                    && !path_params.contains(&"cf")
                    && matches!(&p.type_ref, bynk_syntax::ast::TypeRef::Named(id) if id.name == "RequestCf")
            };
            let (_path_ps, body_ps): (Vec<_>, Vec<_>) = h
                .params
                .iter()
                .filter(|p| !is_headers(p) && !is_query(p) && !is_cookies(p) && !is_cf(p))
                .partition(|p| path_params.contains(&p.name.name.as_str()));
            // The query string the typed drivers append to the URL, and the one
            // the raw drivers append (a `Wire("?…")` text, or a `RequestQuery`).
            let (typed_query, raw_query): (Option<TsExpr>, Option<TsExpr>) =
                match h.params.iter().find(|p| is_query(p)) {
                    Some(p) => {
                        let var = crate::emitter::ts_ident(&p.name.name);
                        if let bynk_syntax::ast::TypeRef::RequestQuery(_) = &p.type_ref {
                            let s = call(ident("__requestQuerySearch"), vec![ident(var)]);
                            (Some(s.clone()), Some(s))
                        } else {
                            let record = &p.type_ref;
                            let ser = crate::emitter::serialisation::serialise_expr_via(
                                record,
                                &bynk_ts::TsExpr::Ident(var.clone()),
                                &type_ns,
                                runtime_use,
                            );
                            // #3343: a renamed field is written under its `@name`.
                            (
                                Some(call(
                                    ident("__queryString"),
                                    query_string_args(record, ser, &table.types),
                                )),
                                Some(ident(var)),
                            )
                        }
                    }
                    None => (None, None),
                };
            // #3277: the `Cookie` header the typed drivers send (a typed record's wire
            // form), and the one the raw drivers send (a `Wire` string).
            let cookies_param = h.params.iter().find(|p| is_cookies(p));
            let (typed_cookie, raw_cookie) =
                cookie_headers(cookies_param, &type_ns, &table.types, runtime_use);
            if let Some(cp) = cookies_param {
                let key = (
                    sname.clone(),
                    method.as_str().to_string(),
                    route_with(&path, emitter::COOKIES_ROUTE_SUFFIX),
                );
                register_param_route(&mut route_body, key, h, cp);
            }
            if let Some(qp) = h.params.iter().find(|p| is_query(p))
                && matches!(qp.type_ref, bynk_syntax::ast::TypeRef::Named(_))
            {
                let idx = h
                    .params
                    .iter()
                    .position(|p| p.name.name == qp.name.name)
                    .expect("query param is drawn from h.params");
                route_body.insert(
                    (
                        sname.clone(),
                        method.as_str().to_string(),
                        route_with(&path, emitter::QUERY_ROUTE_SUFFIX),
                    ),
                    (idx, qp.type_ref.clone()),
                );
            }
            // #708: record the body param's position (within the call's
            // positional args, matching `h.params` declaration order) and
            // declared type, so a mixed typed+`Wire` call can serialise a
            // typed body into the raw driver's `string` slot.
            if let Some(bp) = body_ps.first() {
                let idx = h
                    .params
                    .iter()
                    .position(|p| p.name.name == bp.name.name)
                    .expect("body param is drawn from h.params");
                route_body.insert(
                    (sname.clone(), method.as_str().to_string(), path.clone()),
                    (idx, bp.type_ref.clone()),
                );
                // #3276: a `@form` route's typed body is written as a form.
                if h.annotations.iter().any(|a| a.name.name == "form") {
                    route_body.insert(
                        (
                            sname.clone(),
                            method.as_str().to_string(),
                            route_with(&path, emitter::FORM_ROUTE_SUFFIX),
                        ),
                        (idx, bp.type_ref.clone()),
                    );
                }
            }
            // The signed Authorization header, if the handler's `by` is Bearer.
            // #2019: a refinement-guarded route's signing drivers take the
            // claims the acting actor presents, signed next to `sub`.
            let mut signs_claims = false;
            let mut lone_bearer_read: Option<TsExpr> = None;
            // #3052: the expression for an `Oidc` route's trust roots.
            let mut oidc_trust: Option<TsExpr> = None;
            let auth_header: Entries = match bynk_check::actors::bearer_seam_for(h, &table.actors) {
                Some(seam) => {
                    secrets.insert(seam.secret.clone());
                    signs_claims = seam.authorization.is_some();
                    let mut claim_entries = Vec::new();
                    if signs_claims {
                        claims.routes.insert((
                            sname.clone(),
                            method.as_str().to_string(),
                            path.clone(),
                        ));
                        claim_entries.push(TsObjectEntry::Spread(ident("__claims")));
                    }
                    claim_entries.push(TsObjectEntry::prop("sub", ident("__sub")));
                    claim_entries.push(TsObjectEntry::prop(
                        "exp",
                        binary(TsBinaryOp::Add, now_secs(), num_lit("3600")),
                    ));
                    // P7.2: `Record<string, string>`, not `emitter/workers.rs`'s own
                    // `Record<string, unknown>` marker — this expression feeds
                    // `__bynkSignHs256` directly with no `typeof` narrowing step in
                    // between (unlike workers.rs's own use), and real `process.env`
                    // values genuinely are `string | undefined`, so `string` is both
                    // accurate and what makes `?? ""` actually type as `string`.
                    // #2134: a case's `env` entry for the secret wins over the
                    // fixed test secret, as it does at the seam, which reads
                    // the participant's `env` first.
                    let secret_read = secret_read_of(&seam.secret);
                    lone_bearer_read = Some(secret_read.clone());
                    vec![header(
                        "authorization",
                        bearer_token(TsExpr::object_entries(claim_entries), secret_read),
                    )]
                }
                None => match bynk_check::actors::oidc_seam_for(h, &table.actors) {
                    Some(seam) => {
                        has_oidc = true;
                        // The actor's declared roots, replaced by its
                        // `trustFrom` variable's JSON when the case's `env`
                        // sets it, as the Worker's seam reads them.
                        let declared = object_of(vec![
                            ("issuer", str_lit(seam.issuer.clone())),
                            ("audience", str_lit(seam.audience.clone())),
                            ("jwks", str_lit(seam.jwks.clone())),
                        ]);
                        let trust = match &seam.trust_from {
                            Some(var) => call(
                                ident("__bynkOidcTrustOf"),
                                vec![declared, secret_read_of(var)],
                            ),
                            None => declared,
                        };
                        oidc_trust = Some(trust.clone());
                        signs_claims = true;
                        claims.oidc_routes.insert((
                            sname.clone(),
                            method.as_str().to_string(),
                            path.clone(),
                        ));
                        vec![header(
                            "authorization",
                            oidc_token(
                                trust,
                                object_of(vec![
                                    ("sub", ident("__sub")),
                                    ("claims", ident("__claims")),
                                    ("kind", str_lit("valid")),
                                ]),
                            ),
                        )]
                    }
                    None => Vec::new(),
                },
            };
            // #2155: an actor-sum route's credential depends on the member a
            // call acts as, so its drivers take that actor (`__as`) and build
            // the credential per call, rather than the primary member's.
            let route_key = (sname.clone(), method.as_str().to_string(), path.clone());
            let sum = sum_auth_for(h, &table.actors, &mut secrets, &secret_read_of);
            let is_sum = h.by_clause.as_ref().is_some_and(|by| by.is_sum());
            let (auth_header, signs_claims) = if is_sum {
                (Vec::new(), false)
            } else {
                (auth_header, signs_claims)
            };
            // #3386: a `SignedCookie` route's drivers send a session cookie, and by default the
            // `Sec-Fetch-Site: same-origin` a browser's own page would (a case's headers win).
            let (session, typed_cookie, raw_cookie, csrf_default) = session_route(
                h,
                &table.actors,
                &secret_read_of,
                &mut session_secrets,
                (&typed_cookie, &raw_cookie),
            );
            // #2864: the secret a `by Forged` / `by Expired` Bearer token is
            // signed against — the lone Bearer actor's, or a sum's first
            // Bearer member's.
            let bearer_read: Option<TsExpr> = if is_sum {
                sum.as_ref().and_then(SumAuth::bearer).cloned()
            } else {
                lone_bearer_read
            };
            if let Some(primary) = sum
                .as_ref()
                .and(h.by_clause.as_ref())
                .map(|by| by.primary().name.clone())
            {
                claims.sum_routes.insert(route_key.clone(), primary);
            }
            // #2135: a `Signature` route is signed as its actor declares: the
            // driver signs the exact bytes it sends, and the `Forged`/`Stale`
            // variants sign with a wrong secret or in the past. On a sum, they
            // sign as its first `Signature` member (#2155).
            let lone_actor = h.by_clause.as_ref().is_some_and(|by| !by.is_sum());
            let lone_sig = if auth_header.is_empty() && lone_actor {
                bynk_check::actors::signature_seam_for(h, &table.actors).map(|seam| {
                    secrets.insert(seam.secret.clone());
                    let secret_read = secret_read_of(&seam.secret);
                    SigDriver { seam, secret_read }
                })
            } else {
                None
            };
            let sig: Option<&SigDriver> = lone_sig
                .as_ref()
                .or_else(|| sum.as_ref().and_then(SumAuth::signature));
            // The signed drivers' own headers: a lone `Signature` actor's, or
            // a sum's per-call `__auth`.
            let sig_header: Entries = match (&lone_sig, &sum) {
                (Some(z), _) => z.headers(),
                (None, Some(_)) => vec![TsObjectEntry::Spread(ident("__auth"))],
                (None, None) => Vec::new(),
            };
            let bad_sig_header = sig.map(SigDriver::headers).unwrap_or_default();
            signs_hmac |= sig.is_some();
            // Body param → serialise; path params → the URL.
            let body_arg = body_ps.first();
            // A `@raw` route takes its body as the octets themselves, under the
            // declared media type (`application/octet-stream` for `"*/*"`): the
            // driver sends the `Bytes` as it is, with no JSON and no base64.
            let raw_type = h.raw_content_type().map(|ct| {
                if ct == "*/*" {
                    "application/octet-stream"
                } else {
                    ct
                }
            });
            // #3276: a `@form` route takes its body as a urlencoded form, written from
            // the typed record the way a browser would send it.
            let is_form = h.annotations.iter().any(|a| a.name.name == "form");
            // #3276: a `@multipart` route takes its body as a `FormData`, whose
            // boundary the request sets with its own `content-type`.
            let is_multipart = h.annotations.iter().any(|a| a.name.name == "multipart");
            let body_ident =
                |p: &bynk_syntax::ast::Param| ident(crate::emitter::ts_ident(&p.name.name));
            // The serialised wire form of a body param, a node (#1435).
            let serialised = |p: &bynk_syntax::ast::Param| {
                crate::emitter::serialisation::serialise_expr_via(
                    &p.type_ref,
                    &bynk_ts::TsExpr::Ident(crate::emitter::ts_ident(&p.name.name)),
                    &type_ns,
                    runtime_use,
                )
            };
            let (body_stmts, body_init): (Vec<TsStmt>, Entries) = match body_arg {
                Some(p) if is_multipart => {
                    let specs: Vec<TsExpr> = crate::emitter::form_body_specs(h, &table.types)
                        .unwrap_or_default()
                        .iter()
                        .map(crate::emitter::field_spec_source)
                        .map(raw_lit)
                        .collect();
                    (
                        vec![const_(
                            "__body",
                            call(
                                ident("__multipartForm"),
                                vec![
                                    serialised(p),
                                    TsExpr::AsConst(Box::new(TsExpr::array(specs))),
                                ],
                            ),
                        )],
                        vec![body_entry(ident("__body"))],
                    )
                }
                Some(p) if is_form => (
                    vec![const_(
                        "__body",
                        method_call(
                            call(
                                ident("__queryString"),
                                query_string_args(&p.type_ref, serialised(p), &table.types),
                            ),
                            "slice",
                            vec![num_lit("1")],
                        ),
                    )],
                    vec![body_entry(ident("__body"))],
                ),
                Some(p) if raw_type.is_some() && is_stream(&p.type_ref) => {
                    streamed_typed_body(body_ident(p))
                }
                Some(p) if raw_type.is_some() => (
                    vec![const_("__body", body_ident(p))],
                    vec![body_entry(TsExpr::As {
                        expr: Box::new(ident("__body")),
                        ty: TsType::named("BufferSource"),
                    })],
                ),
                Some(p) => (
                    vec![const_(
                        "__body",
                        call(
                            member(ident("globalThis.JSON"), "stringify"),
                            vec![serialised(p)],
                        ),
                    )],
                    vec![body_entry(ident("__body"))],
                ),
                None => (Vec::new(), Vec::new()),
            };
            // The response payload deserialiser: the `T` of `Effect[HttpResult[T]]`.
            let payload_deser: TsExpr = match strip_effect_httpresult(&h.return_type) {
                Some(inner) => {
                    crate::emitter::serialisation::deserialise_ref_via(inner, &type_ns, runtime_use)
                }
                None => ident(format!("{type_ns}__deserialise_unit")),
            };
            // Driver params mirror the handler's params (path params, then body).
            let driver_params: Vec<TsParam> = h
                .params
                .iter()
                .map(|p| {
                    TsParam::new(
                        crate::emitter::ts_ident(&p.name.name),
                        Some(driver_param_type(&p.type_ref)),
                    )
                })
                .chain(extra_headers_param.clone())
                .collect();
            let raw_params: Vec<TsParam> = h
                .params
                .iter()
                .map(|p| {
                    TsParam::new(
                        crate::emitter::ts_ident(&p.name.name),
                        Some(
                            if is_headers(&p)
                                || matches!(p.type_ref, bynk_syntax::ast::TypeRef::RequestQuery(_))
                            {
                                driver_param_type(&p.type_ref)
                            } else {
                                TsType::named("string")
                            },
                        ),
                    )
                })
                .chain(extra_headers_param)
                .collect();
            let content_type: Entries = match (body_arg.is_some(), raw_type) {
                (true, Some(ct)) => vec![header("content-type", str_lit(ct))],
                (true, None) if is_multipart => Vec::new(),
                (true, None) if is_form => vec![header(
                    "content-type",
                    str_lit("application/x-www-form-urlencoded"),
                )],
                (true, None) => vec![header("content-type", str_lit("application/json"))],
                (false, _) => Vec::new(),
            };
            let url = request_url(&path, typed_query.clone());
            let raw_url = request_url(&path, raw_query.clone());
            let (spread_first, spread_last) = spread_order(
                is_form || (body_arg.is_some() && !is_multipart),
                &headers_spread,
            );
            let verb = method.as_str();
            let cat = |parts: Vec<&Entries>| -> Entries {
                parts.into_iter().flatten().cloned().collect()
            };
            let typed_options = request_init(
                verb,
                cat(vec![
                    &csrf_default,
                    &spread_first,
                    &content_type,
                    &spread_last,
                    &typed_cookie,
                    &auth_header,
                    &sig_header,
                ]),
                body_init.clone(),
            );
            // #2135: the bytes a signing driver signs: what it sends.
            let typed_body = if body_arg.is_some() {
                ident("__body")
            } else {
                str_lit("")
            };
            let raw_body = match body_ps.first() {
                Some(p) => body_ident(p),
                None => str_lit(""),
            };
            // #2685: the typed driver names its route to the decoder, which a
            // smoke run checks the answer's status against: the context, the
            // service, the method and the path, as the driver's smoke table
            // keys them. It reads that table only in a smoke run.
            let typed_payload: Vec<TsExpr> = if smoke {
                vec![
                    payload_deser.clone(),
                    str_lit(format!("{target} {sname} {verb} {path}")),
                ]
            } else {
                vec![payload_deser.clone()]
            };
            let payload = vec![payload_deser.clone()];
            routes.push(sysdrive_driver(
                SysdriveKind::typed(""),
                sname,
                &key,
                driver_params.clone(),
                body_stmts.clone(),
                url.clone(),
                typed_options,
                &binding,
                "__responseToHttpResult",
                typed_payload,
                signs_claims,
                signing_stmts(&lone_sig, &sum, &typed_body),
                sum.is_some(),
            ));
            // Slice C: the raw driver for a `Wire(…)`-carrying call. Every slot is
            // a raw `string` (the wire form the boundary receives *unvalidated*):
            // path params flow into the URL, the body string is sent verbatim (no
            // `serialise`), and the response decodes to an `HttpOutcome` —
            // `Rejected(detail)` when the router refused the input before the
            // handler, `Handled(httpResult)` when it ran. Emitted only for a route
            // with a `Wire`-eligible slot (a body or a path param); a bodyless,
            // path-param-less route (e.g. `GET /cart/size`) can carry no `Wire`
            // argument, so its raw driver would be dead code.
            let raw_body_init = wire_body_init(body_ps.first().copied(), raw_type.is_some());
            if !h.params.is_empty() {
                // The `Wire` text is text, so a raw route is sent it as `text/plain`:
                // a route declaring another media type answers `415`, which is how a
                // case probes it, and a `"*/*"` route still takes the text.
                let raw_content_type: Entries = if raw_type.is_some() {
                    vec![header("content-type", str_lit("text/plain; charset=utf-8"))]
                } else if is_multipart && body_arg.is_some() {
                    // A `Wire` body is the multipart text itself, framed with this
                    // boundary (`--bynk-boundary`).
                    vec![header(
                        "content-type",
                        str_lit("multipart/form-data; boundary=bynk-boundary"),
                    )]
                } else {
                    content_type.clone()
                };
                let raw_options = request_init(
                    verb,
                    cat(vec![
                        &csrf_default,
                        &spread_first,
                        &raw_content_type,
                        &spread_last,
                        &raw_cookie,
                        &auth_header,
                        &sig_header,
                    ]),
                    raw_body_init.clone(),
                );
                routes.push(sysdrive_driver(
                    SysdriveKind::raw(""),
                    sname,
                    &key,
                    raw_params.clone(),
                    Vec::new(),
                    raw_url.clone(),
                    raw_options,
                    &binding,
                    "__responseToHttpOutcome",
                    payload.clone(),
                    signs_claims,
                    signing_stmts(&lone_sig, &sum, &raw_body),
                    sum.is_some(),
                ));
            }
            // #706: the no-auth driver for a `by Nobody` call — the same request
            // the typed driver builds, minus the `Authorization` header, so the
            // real auth seam rejects it. A `401` decodes to `Rejected(
            // Unauthorized)` (`responseToUnauthOutcome`); anything else decodes
            // normally. Emitted only for a Bearer-secured route (one that carries
            // an auth header) — an unsecured route has no seam to reject a missing
            // credential, so a `by Nobody` there is meaningless.
            let secured =
                !auth_header.is_empty() || sig.is_some() || sum.is_some() || session.is_some();
            if secured {
                let noauth_options = request_init(
                    verb,
                    cat(vec![
                        &csrf_default,
                        &vec![headers_spread.clone()],
                        &content_type,
                    ]),
                    body_init.clone(),
                );
                routes.push(sysdrive_driver(
                    SysdriveKind::typed("noauth"),
                    sname,
                    &key,
                    driver_params.clone(),
                    body_stmts.clone(),
                    url.clone(),
                    noauth_options,
                    &binding,
                    "__responseToUnauthOutcome",
                    payload.clone(),
                    false,
                    Vec::new(),
                    false,
                ));
            }
            // #821: the raw *and* no-auth driver combined, for a call mixing
            // `Wire(…)` with `by Nobody` — every slot is a raw `string` (as
            // `__sysdrive_raw`) and the `Authorization` header is dropped (as
            // `__sysdrive_noauth`), so the seam rejects the missing credential
            // before the (possibly malformed) raw body is even read.
            // `responseToUnauthOutcome` already delegates a non-`401` status to
            // `responseToHttpOutcome`'s shape-based classification, so a raw
            // body that would have been rejected on its own shape still decodes
            // correctly here. Emitted under the same conditions as both parent
            // drivers: a `Wire`-eligible slot and a Bearer-secured route.
            if !h.params.is_empty() && secured {
                let rawnoauth_options = request_init(
                    verb,
                    cat(vec![
                        &csrf_default,
                        &vec![headers_spread.clone()],
                        &content_type,
                    ]),
                    raw_body_init.clone(),
                );
                routes.push(sysdrive_driver(
                    SysdriveKind::raw("noauth"),
                    sname,
                    &key,
                    raw_params.clone(),
                    Vec::new(),
                    raw_url.clone(),
                    rawnoauth_options,
                    &binding,
                    "__responseToUnauthOutcome",
                    payload.clone(),
                    false,
                    Vec::new(),
                    false,
                ));
            }
            // #2135: `by Forged` (signed with a wrong secret) and `by Stale`
            // (signed `tolerance + 60` seconds in the past), typed and raw,
            // each decoding a `401` to `Rejected(Unauthorized)`. `Stale` only
            // where the actor declares a timestamp and a tolerance.
            if let Some(z) = sig {
                let forged_options = request_init(
                    verb,
                    cat(vec![
                        &vec![headers_spread.clone()],
                        &content_type,
                        &bad_sig_header,
                    ]),
                    body_init.clone(),
                );
                let mut variants = vec![("forged", SigVariant::Forged)];
                if let Some(age) = z.stale_age() {
                    variants.push(("stale", SigVariant::Stale(age)));
                }
                for (name, variant) in variants {
                    routes.push(sysdrive_driver(
                        SysdriveKind::typed(name),
                        sname,
                        &key,
                        driver_params.clone(),
                        body_stmts.clone(),
                        url.clone(),
                        forged_options.clone(),
                        &binding,
                        "__responseToUnauthOutcome",
                        payload.clone(),
                        false,
                        vec![z.sign(&typed_body, variant)],
                        false,
                    ));
                    if !h.params.is_empty() {
                        let options = request_init(
                            verb,
                            cat(vec![
                                &vec![headers_spread.clone()],
                                &content_type,
                                &bad_sig_header,
                            ]),
                            raw_body_init.clone(),
                        );
                        routes.push(sysdrive_driver(
                            SysdriveKind::raw(name),
                            sname,
                            &key,
                            raw_params.clone(),
                            Vec::new(),
                            raw_url.clone(),
                            options,
                            &binding,
                            "__responseToUnauthOutcome",
                            payload.clone(),
                            false,
                            vec![z.sign(&raw_body, variant)],
                            false,
                        ));
                    }
                }
            }
            // #2864: on a Bearer route, `by Forged` (a well-formed token signed
            // with a secret derived from the real one) and `by Expired` (a
            // correctly signed token whose `exp` is an hour past the test
            // clock), typed and raw, each decoding a `401` to
            // `Rejected(Unauthorized)`. A route with a `Signature` seam keeps
            // the `Signature` drivers above for `Forged`.
            // A body entry for the raw drivers of the token routes: the plain body param.
            let raw_token_body: Entries = body_ps
                .first()
                .map(|p| vec![body_entry(body_ident(p))])
                .unwrap_or_default();
            let token_header = vec![header("authorization", ident("__token"))];
            if let Some(secret_read) = bearer_read.as_ref() {
                let bearer_options = request_init(
                    verb,
                    cat(vec![
                        &vec![headers_spread.clone()],
                        &content_type,
                        &token_header,
                    ]),
                    body_init.clone(),
                );
                let raw_bearer_options = request_init(
                    verb,
                    cat(vec![
                        &vec![headers_spread.clone()],
                        &content_type,
                        &token_header,
                    ]),
                    raw_token_body.clone(),
                );
                let mut variants = vec![(
                    "expired",
                    TsExpr::Paren(Box::new(secret_read.clone())),
                    TsBinaryOp::Sub,
                )];
                if sig.is_none() {
                    variants.push((
                        "forged",
                        TsExpr::Paren(Box::new(binary(
                            TsBinaryOp::Add,
                            secret_read.clone(),
                            str_lit("!"),
                        ))),
                        TsBinaryOp::Add,
                    ));
                }
                for (name, secret, sign) in variants {
                    // A valid `sub`, so only the signature (`Forged`) or the
                    // `exp` (`Expired`) is wrong. The `sub` is fixed rather
                    // than the call's identity: `by Forged` names none.
                    let sign_stmt = const_(
                        "__token",
                        bearer_token(
                            object_of(vec![
                                ("sub", str_lit("bynk-test")),
                                ("exp", binary(sign, now_secs(), num_lit("3600"))),
                            ]),
                            secret,
                        ),
                    );
                    routes.push(sysdrive_driver(
                        SysdriveKind::typed(name),
                        sname,
                        &key,
                        driver_params.clone(),
                        body_stmts.clone(),
                        url.clone(),
                        bearer_options.clone(),
                        &binding,
                        "__responseToUnauthOutcome",
                        payload.clone(),
                        false,
                        vec![sign_stmt.clone()],
                        false,
                    ));
                    if !h.params.is_empty() {
                        routes.push(sysdrive_driver(
                            SysdriveKind::raw(name),
                            sname,
                            &key,
                            raw_params.clone(),
                            Vec::new(),
                            raw_url.clone(),
                            raw_bearer_options.clone(),
                            &binding,
                            "__responseToUnauthOutcome",
                            payload.clone(),
                            false,
                            vec![sign_stmt],
                            false,
                        ));
                    }
                }
            }
            // #3386: on a `SignedCookie` route, `by Forged` (signed with a key the seam does
            // not hold) and `by Expired` (issued before the cookie's `ttl`), each decoding a
            // `401` to `Rejected(Unauthorized)`.
            if let Some(s) = &session {
                let parts = (sname.as_str(), key.as_str(), binding.as_str(), &payload);
                let req = SessionRequest::new(verb, &content_type, &headers_spread, h);
                let (typed, raw) = (
                    (driver_params.as_slice(), &url),
                    (raw_params.as_slice(), &raw_url),
                );
                push_session_refusal_drivers(
                    &mut routes,
                    s,
                    parts,
                    (typed, raw),
                    (
                        &req,
                        &body_stmts,
                        &body_init,
                        body_ps.first().map(|p| p.name.name.as_str()),
                    ),
                );
            }
            // #3052: on an `Oidc` route, `by Forged` (signed by a key the
            // JWKS does not publish), `by Expired`, `by WrongIssuer` and
            // `by WrongAudience`, typed and raw, each decoding a `401` to
            // `Rejected(Unauthorized)`; and (#3225) `by WrongAlgorithm` and
            // `by NoSubject`.
            if let Some(trust) = oidc_trust.as_ref() {
                let oidc_options = request_init(
                    verb,
                    cat(vec![
                        &vec![headers_spread.clone()],
                        &content_type,
                        &token_header,
                    ]),
                    body_init.clone(),
                );
                let raw_oidc_options = request_init(
                    verb,
                    cat(vec![
                        &vec![headers_spread.clone()],
                        &content_type,
                        &token_header,
                    ]),
                    raw_token_body.clone(),
                );
                // #3225: `WrongAlgorithm` and `NoSubject` each present two
                // tokens. Each token has a driver of its own, and the
                // principal's driver (below) runs them in turn.
                for name in [
                    "forged",
                    "expired",
                    "wrongissuer",
                    "wrongaudience",
                    "algnone",
                    "alghs256",
                    "emptysub",
                    "nosub",
                ] {
                    // A valid `sub`, so only the one thing under test is wrong.
                    let sign_stmt = const_(
                        "__token",
                        oidc_token(
                            trust.clone(),
                            object_of(vec![
                                ("sub", str_lit("bynk-test")),
                                ("claims", TsExpr::object(Vec::new())),
                                ("kind", str_lit(name)),
                            ]),
                        ),
                    );
                    routes.push(sysdrive_driver(
                        SysdriveKind::typed(name),
                        sname,
                        &key,
                        driver_params.clone(),
                        body_stmts.clone(),
                        url.clone(),
                        oidc_options.clone(),
                        &binding,
                        "__responseToUnauthOutcome",
                        payload.clone(),
                        false,
                        vec![sign_stmt.clone()],
                        false,
                    ));
                    if !h.params.is_empty() {
                        routes.push(sysdrive_driver(
                            SysdriveKind::raw(name),
                            sname,
                            &key,
                            raw_params.clone(),
                            Vec::new(),
                            raw_url.clone(),
                            raw_oidc_options.clone(),
                            &binding,
                            "__responseToUnauthOutcome",
                            payload.clone(),
                            false,
                            vec![sign_stmt],
                            false,
                        ));
                    }
                }
                push_two_token_principals(&mut routes, sname, &key, !h.params.is_empty());
            }
        }
        // #707: the generic wrong-method driver.
        routes.push(wrong_method_driver(sname, &binding));
    }

    if http_services.is_empty() {
        return SystemHttpSupport {
            code: Vec::new(),
            http_services,
            declared_routes: declared,
            route_body,
            type_ns: String::new(),
            claims: emitter::ctx::SystemHttpClaims::default(),
        };
    }

    // #1485: the HS256 signer block — a monotonic clock the signer's `exp`
    // uses (kept out of `bundle`d runtime), plus the test-only HS256 signer
    // itself (never in the deployable app; e2e owns real auth) — now lives
    // at `runtime/src/testing/jwt_signer.ts`, the same committed-`.ts`-file
    // treatment `expectation_runtime_helpers`/`stub_runtime_helpers` already
    // get, rather than an unexplained raw string literal with `\`-continuations.
    let mut code = vec![TestRuntimeModule::JwtSigner.splice()];
    // #3052: the test issuer an `Oidc` route's drivers mint tokens with.
    if has_oidc {
        // The remote flag is the first line of the issuer's text, so the two print as one block.
        let mut remote_flag = oidc_remote_flag(remote);
        remote_flag.no_blank_before = true;
        code.push(remote_flag);
        let mut issuer = TestRuntimeModule::OidcIssuer.splice();
        issuer.no_blank_before = true;
        code.push(issuer);
    }
    if signs_hmac {
        let mut signer = TestRuntimeModule::HmacSigner.splice();
        signer.no_blank_before = true;
        code.push(signer);
    }
    // Set each secret the target's actors read, so the real Bearer seam verifies.
    // P7.2: `Record<string, string>` — same reasoning as `__bynkSignHs256`'s own
    // `secret: string` parameter in `runtime/src/testing/jwt_signer.ts`.
    let record_string_string = TsType::named_with_args(
        "globalThis.Record",
        vec![TsType::named("string"), TsType::named("string")],
    );
    let cast_globalthis_process = |process_optional: bool| -> TsExpr {
        let env_ty = TsType::Object(vec![TsTypeMember::prop(
            "env",
            record_string_string.clone(),
        )]);
        let process_member = if process_optional {
            TsTypeMember::optional_prop("process", env_ty)
        } else {
            TsTypeMember::prop("process", env_ty)
        };
        TsExpr::As {
            expr: Box::new(TsExpr::As {
                expr: Box::new(ident("globalThis")),
                ty: TsType::named("unknown"),
            }),
            ty: TsType::Object(vec![process_member]),
        }
    };
    secrets.extend(session_secrets.iter().cloned());
    for s in secrets.iter().filter(|_| !remote) {
        let assign_process = TsStmt::assign(
            member(cast_globalthis_process(false), "process"),
            TsExpr::Binary {
                op: TsBinaryOp::NullishCoalescing,
                left: Box::new(member(cast_globalthis_process(true), "process")),
                right: Box::new(TsExpr::object(vec![(
                    "env".to_string(),
                    TsExpr::object(vec![]),
                )])),
            },
            None,
        );
        let assign_secret = TsStmt::assign(
            TsExpr::Index {
                object: Box::new(member(
                    member(cast_globalthis_process(false), "process"),
                    "env",
                )),
                index: Box::new(str_lit(s.clone())),
            },
            str_lit(if session_secrets.contains(s) {
                crate::testgen::TEST_SESSION_KEY
            } else {
                "__bynk_test_secret"
            }),
            None,
        );
        code.push(assign_process);
        code.push(assign_secret);
    }
    code.extend(routes);
    SystemHttpSupport {
        code,
        http_services,
        declared_routes: declared,
        route_body,
        type_ns,
        claims,
    }
}

/// #3380: whether a handler parameter is a `Stream`, so a `@raw` route's `body` is
/// streamed.
fn is_stream(t: &bynk_syntax::ast::TypeRef) -> bool {
    matches!(t, bynk_syntax::ast::TypeRef::Stream(..))
}

/// #3380: the typed driver's body for a streamed `@raw` route: a `ReadableStream`
/// pulled from the case's `Stream[Bytes]`, so it goes chunked with no
/// `content-length`, and the router's counted read (not the `Content-Length`
/// fast-reject) is what a case over the cap exercises. `duplex: "half"` is what a
/// streamed request body needs on Node; the spread keeps it out of `RequestInit`'s
/// excess-property check on lib versions that lack it.
fn streamed_typed_body(chunks: TsExpr) -> (Vec<TsStmt>, Entries) {
    let pull = TsObjectEntry::method(
        "pull",
        true,
        vec![TsParam::new("c", None)],
        None,
        vec![
            const_(
                "r",
                await_expr(method_call(ident("__it"), "next", Vec::new())),
            ),
            TsStmt::if_else_same_line_stmt(
                member(ident("r"), "done"),
                expr_stmt(method_call(ident("c"), "close", Vec::new())),
                expr_stmt(method_call(
                    ident("c"),
                    "enqueue",
                    vec![member(ident("r"), "value")],
                )),
                None,
            ),
        ],
    );
    let stream = new(
        type_app(
            ident("globalThis.ReadableStream"),
            vec![TsType::named("Uint8Array")],
        ),
        vec![
            TsExpr::object_entries(vec![pull]),
            object_of(vec![("highWaterMark", num_lit("0"))]),
        ],
    );
    (
        vec![
            const_(
                "__it",
                call(
                    index(chunks, member(ident("globalThis.Symbol"), "asyncIterator")),
                    Vec::new(),
                ),
            ),
            const_("__body", stream),
        ],
        vec![body_entry(ident("__body")), duplex_half()],
    )
}

/// The request-init `body:` entry a `Wire` driver sends for a route's body slot (a
/// raw `string`). #3380: for a streamed `@raw` route it is one streamed chunk with
/// no `content-length`, so a case can drive the counted overrun (a `"*/*"` route
/// takes the `text/plain` it is sent).
fn wire_body_init(body: Option<&bynk_syntax::ast::Param>, raw: bool) -> Entries {
    let Some(p) = body else {
        return Vec::new();
    };
    let value = ident(crate::emitter::ts_ident(&p.name.name));
    if raw && is_stream(&p.type_ref) {
        let start = TsObjectEntry::method(
            "start",
            false,
            vec![TsParam::new("c", None)],
            None,
            vec![
                expr_stmt(method_call(
                    ident("c"),
                    "enqueue",
                    vec![method_call(
                        new(ident("globalThis.TextEncoder"), Vec::new()),
                        "encode",
                        vec![value],
                    )],
                )),
                expr_stmt(method_call(ident("c"), "close", Vec::new())),
            ],
        )
        .inlined();
        vec![
            body_entry(new(
                type_app(
                    ident("globalThis.ReadableStream"),
                    vec![TsType::named("Uint8Array")],
                ),
                vec![TsExpr::object_entries(vec![start])],
            )),
            duplex_half(),
        ]
    } else {
        vec![body_entry(value)]
    }
}

/// Type a system-http driver parameter (v0.182): a named type is reached
/// through the target's handler namespace (`todos.AddRequest`); primitives and
/// compound shapes fall back to `unknown` — the driver only forwards the value
/// to the namespace's `serialise_*`, and the Bynk checker already typed the call.
pub(super) fn driver_param_type(t: &bynk_syntax::ast::TypeRef) -> TsType {
    use bynk_syntax::ast::TypeRef;
    if let TypeRef::Named(_) = t {
        // A named type may embed a branded refined field (`AddRequest.title:
        // Title`), and the case body passes a plain object literal — the Bynk
        // checker already type-checked the call's args against the handler
        // (Slice A), so the driver forwards the value to the namespace's
        // `serialise_*`. Primitives stay precise.
        any_type()
    } else {
        crate::emitter::ts_types::ts_type_of(t)
    }
}

/// Extract `T` from a handler return type `Effect[HttpResult[T]]` (v0.182).
pub(super) fn strip_effect_httpresult(
    t: &bynk_syntax::ast::TypeRef,
) -> Option<&bynk_syntax::ast::TypeRef> {
    use bynk_syntax::ast::TypeRef;
    let inner = if let TypeRef::Effect(b, _) = t {
        b.as_ref()
    } else {
        t
    };
    if let TypeRef::HttpResult(payload, _) = inner {
        Some(payload.as_ref())
    } else {
        None
    }
}

/// A `{ fetch: (req: Request) => worker_X.fetch(req, env_X) } as ServiceBinding`
/// expression — the identical shape both the per-participant and root-env
/// binding wiring in [`emit_integration_harness`] build, differing only in
/// which worker/env pair they close over.
pub(super) fn service_binding_forward(worker_ident: &str, env_ident: &str, sends: bool) -> TsExpr {
    let req_param = TsParam::new("req".to_string(), Some(TsType::named("globalThis.Request")));
    // #1994: a Worker whose handlers send with `detach` takes the execution
    // context as `fetch`'s third argument; the harness hands it a collecting
    // double and settles the sends before the response reaches the case.
    let forward = if sends {
        call(
            ident("__bynkFetchSettled"),
            vec![TsExpr::Arrow {
                params: vec![TsParam::new("exec".to_string(), None)],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(method_call(
                    ident(worker_ident),
                    "fetch",
                    vec![ident("req"), ident(env_ident), ident("exec")],
                )))),
            }],
        )
    } else {
        method_call(
            ident(worker_ident),
            "fetch",
            vec![ident("req"), ident(env_ident)],
        )
    };
    TsExpr::As {
        expr: Box::new(TsExpr::object(vec![(
            "fetch".to_string(),
            TsExpr::Arrow {
                params: vec![req_param],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(forward))),
            },
        )])),
        ty: TsType::named("__ServiceBinding"),
    }
}

/// Interop I5 (#1972): a Service Binding standing in for a foreign Worker in
/// the integration harness, answering every request with
/// [`crate::emitter::wrangler::foreign_service_double_body`]'s 503, as the
/// double `bynk dev` binds by default does. A case that needs another answer
/// stubs the instance's `send`.
pub(super) fn foreign_service_double(binding: &str, worker: &str) -> TsExpr {
    let response = TsExpr::New {
        callee: Box::new(ident("globalThis.Response")),
        args: vec![
            str_lit(crate::emitter::wrangler::foreign_service_double_body(
                binding, worker,
            )),
            TsExpr::object(vec![(
                "status".to_string(),
                num_lit(crate::emitter::wrangler::FOREIGN_SERVICE_DOUBLE_STATUS.to_string()),
            )]),
        ],
    };
    TsExpr::As {
        expr: Box::new(TsExpr::object(vec![(
            "fetch".to_string(),
            TsExpr::Arrow {
                params: Vec::new(),
                is_async: true,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Expr(Box::new(response))),
            },
        )])),
        ty: TsType::named("__ServiceBinding"),
    }
}

/// The name a system test case calls a participant as: its cases lower in a
/// commons of this name ([`integration_typed_commons`]), and a cross-context
/// call stamps the calling commons' name. I2 (#1971): the harness gives every
/// participant the local key for the edge from it.
pub(super) const HARNESS_CALLER: &str = "integration";

/// #3277, #3386 (1B.3c): a route's `Cookie` header value, held as the node its `cookies`
/// parameter contributes, so a `SignedCookie` actor's session cookie joins it as a second node
/// and not by editing the rendered entry.
struct CookieHeader {
    /// The parameter's part of the header; `None` when the handler takes no `cookies`.
    param: Option<TsExpr>,
}

impl CookieHeader {
    /// The request-init entry `"cookie": <value>` (none when there is no value), the value
    /// being the parameter's part, the `session` cookie's, or the two joined with `"; "`.
    fn render(&self, session: Option<&TsExpr>) -> Entries {
        let value = match (&self.param, session) {
            (None, None) => return Vec::new(),
            (Some(only), None) | (None, Some(only)) => only.clone(),
            (Some(param), Some(session)) => crate::ts_build::method_call(
                TsExpr::array(vec![param.clone(), session.clone()]),
                "join",
                vec![crate::ts_build::str_lit("; ")],
            ),
        };
        vec![header("cookie", value)]
    }
}

/// #3277: the `Cookie` header the typed system drivers send (a typed record's wire
/// form) and the one the raw drivers send (a `Wire` string), for a handler's
/// `cookies` parameter; empty when it has none.
fn cookie_headers(
    param: Option<&bynk_syntax::ast::Param>,
    type_ns: &str,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
    runtime_use: &RuntimeUse,
) -> (CookieHeader, CookieHeader) {
    let Some(p) = param else {
        return (CookieHeader { param: None }, CookieHeader { param: None });
    };
    let var = crate::emitter::ts_ident(&p.name.name);
    // #3277: the raw view sends the header text it was built from (a `Wire`
    // string is the header text itself).
    if matches!(p.type_ref, bynk_syntax::ast::TypeRef::RequestCookies(_)) {
        let header = || CookieHeader {
            param: Some(TsExpr::Paren(Box::new(TsExpr::Conditional {
                test: Box::new(crate::ts_build::strict_eq(
                    crate::ts_build::typeof_expr(ident(var.clone())),
                    crate::ts_build::str_lit("string"),
                )),
                consequent: Box::new(ident(var.clone())),
                alternate: Box::new(crate::ts_build::call(
                    ident("__requestCookiesHeader"),
                    vec![ident(var.clone())],
                )),
            }))),
        };
        return (header(), header());
    }
    let ser = crate::emitter::serialisation::serialise_expr_via(
        &p.type_ref,
        &TsExpr::Ident(var.clone()),
        type_ns,
        runtime_use,
    );
    // #3343: a renamed field is written under its `@name`.
    let mut args = vec![ser];
    args.extend(
        crate::emitter::renamed_specs_source(&p.type_ref, types).map(crate::ts_build::raw_lit),
    );
    (
        CookieHeader {
            param: Some(crate::ts_build::call(ident("__cookieString"), args)),
        },
        CookieHeader {
            param: Some(ident(var)),
        },
    )
}

/// Record that the handler's parameter `param` is typed, so the system driver and
/// the call lowering know its position and type.
fn register_param_route(
    route_body: &mut RouteBodyMap,
    key: (String, String, String),
    h: &bynk_syntax::ast::Handler,
    param: &bynk_syntax::ast::Param,
) {
    let idx = h
        .params
        .iter()
        .position(|p| p.name.name == param.name.name)
        .expect("the parameter is drawn from the handler's own");
    route_body.insert(key, (idx, param.type_ref.clone()));
}

/// #3343: the arguments of `__queryString` for a typed record's wire form `ser`: the
/// record's field table too when a field is renamed by `@name`.
fn query_string_args(
    type_ref: &bynk_syntax::ast::TypeRef,
    ser: TsExpr,
    types: &std::collections::HashMap<String, std::sync::Arc<bynk_syntax::ast::TypeDecl>>,
) -> Vec<TsExpr> {
    let mut args = vec![ser];
    if let Some(table) = crate::emitter::renamed_specs_source(type_ref, types) {
        args.push(crate::ts_build::raw_lit(table));
    }
    args
}

/// #2155: the credentials an actor-sum route's members present, each read against its own secret.
fn sum_auth_for(
    h: &bynk_syntax::ast::Handler,
    actors: &HashMap<String, bynk_syntax::ast::ActorDecl>,
    secrets: &mut std::collections::BTreeSet<String>,
    secret_read_of: &dyn Fn(&str) -> TsExpr,
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
            };
            Some((m.actor_name, cred))
        })
        .collect();
    (!members.is_empty()).then_some(SumAuth { members })
}

/// #3225: the two-token principals of an `Oidc` route. The driver presents each token in order
/// and returns the first outcome that is not the seam's refusal, so a seam that admits either
/// one fails the case's `Rejected(Unauthorized)`; when every token is refused it returns that
/// refusal. Typed and raw, like the others.
fn push_two_token_principals(routes: &mut Vec<TsStmt>, sname: &str, key: &str, has_params: bool) {
    let prefixes: &[bool] = if has_params { &[false, true] } else { &[false] };
    for (principal, tokens) in [
        ("wrongalgorithm", ["algnone", "alghs256"]),
        ("nosubject", ["emptysub", "nosub"]),
    ] {
        for &raw in prefixes {
            let driver = |kind: &str| sysdrive_name(SysdriveKind { raw, kind }, sname, key);
            let first = driver(tokens[0]);
            let args = || vec![TsExpr::Spread(Box::new(ident("__a")))];
            let refused = binary(
                TsBinaryOp::Or,
                strict_neq(member(ident("__first"), "tag"), str_lit("Rejected")),
                strict_neq(
                    member(member(ident("__first"), "value"), "tag"),
                    str_lit("Unauthorized"),
                ),
            );
            routes.push(async_driver(
                driver(principal),
                vec![TsParam {
                    rest: true,
                    ..TsParam::new(
                        "__a",
                        Some(TsType::named_with_args(
                            "Parameters",
                            vec![TsType::type_of(ident(first.clone()))],
                        )),
                    )
                }],
                vec![
                    const_("__first", await_expr(call(ident(first), args()))),
                    if_(refused, ret(ident("__first"))),
                    ret(await_expr(call(ident(driver(tokens[1])), args()))),
                ],
            ));
        }
    }
}

/// #3386: what a `system` driver needs to mint a `SignedCookie` actor's session cookie.
struct SessionDriver {
    /// The `{ secret: … }` keys expression the cookie is signed with.
    keys: TsExpr,
    /// The declaring actor's name: the cookie's `aud`.
    actor: String,
    /// The cookie's name.
    cookie: String,
    /// How long an issued cookie is valid, in seconds.
    ttl_secs: i64,
}

impl SessionDriver {
    /// The `name=value` template literal for `sub` signed with `keys` and issued `issued_secs`
    /// (an expression, in seconds) after the epoch.
    fn pair(&self, keys: &TsExpr, sub: TsExpr, issued_secs: TsExpr) -> TsExpr {
        use crate::ts_build::{await_expr, binary, call, num_lit, paren, str_lit};
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

/// #3386: a route's session driver, if its `by` is a `SignedCookie` actor, and the `Cookie`
/// header texts its typed and raw drivers send (the session cookie joined to any `cookies`
/// param's), and the default `Sec-Fetch-Site` header.
fn session_route(
    h: &bynk_syntax::ast::Handler,
    actors: &HashMap<String, bynk_syntax::ast::ActorDecl>,
    secret_read_of: &dyn Fn(&str) -> TsExpr,
    session_secrets: &mut std::collections::BTreeSet<String>,
    (typed_cookie, raw_cookie): (&CookieHeader, &CookieHeader),
) -> (Option<SessionDriver>, Entries, Entries, Entries) {
    let Some(seam) = bynk_check::actors::signed_cookie_seam_for(h, actors) else {
        return (
            None,
            typed_cookie.render(None),
            raw_cookie.render(None),
            Vec::new(),
        );
    };
    session_secrets.insert(seam.secret.clone());
    let session = SessionDriver {
        keys: TsExpr::object(vec![("secret".to_string(), secret_read_of(&seam.secret))]),
        actor: seam.actor,
        cookie: seam.cookie,
        ttl_secs: seam.ttl_secs,
    };
    let pair = session.pair(&session.keys, ident("__sub"), now_secs());
    (
        Some(session),
        typed_cookie.render(Some(&pair)),
        raw_cookie.render(Some(&pair)),
        vec![header("sec-fetch-site", str_lit("same-origin"))],
    )
}

/// `__bynkNow()`: the harness clock, in seconds.
fn now_secs() -> TsExpr {
    crate::ts_build::call(ident("__bynkNow"), Vec::new())
}

/// A driver's parameters and the URL it requests.
type DriverShape<'a> = (&'a [TsParam], &'a TsExpr);

/// The request a route's session-refusal drivers build, bar the credential.
struct SessionRequest<'a> {
    method: &'a str,
    content_type: &'a Entries,
    headers_spread: &'a TsObjectEntry,
    has_params: bool,
}

impl<'a> SessionRequest<'a> {
    fn new(
        method: &'a str,
        content_type: &'a Entries,
        headers_spread: &'a TsObjectEntry,
        h: &bynk_syntax::ast::Handler,
    ) -> Self {
        SessionRequest {
            method,
            content_type,
            headers_spread,
            has_params: !h.params.is_empty(),
        }
    }
}

/// #3386: the `forged_` and `expired_` drivers (typed, and raw when the route takes params)
/// of a `SignedCookie` route: a well-formed cookie signed with a key the seam does not hold,
/// and a correctly signed one issued before its `ttl`. Each decodes a `401` to
/// `Rejected(Unauthorized)`. Both carry `Sec-Fetch-Site: same-origin`, so the refusal is the
/// cookie's and not the CSRF check's.
fn push_session_refusal_drivers(
    routes: &mut Vec<TsStmt>,
    session: &SessionDriver,
    (sname, key, binding, payload): (&str, &str, &str, &Vec<TsExpr>),
    ((driver_params, url), (raw_params, raw_url)): (DriverShape<'_>, DriverShape<'_>),
    (req, body_stmts, body_init, body_name): (
        &SessionRequest<'_>,
        &Vec<TsStmt>,
        &Entries,
        Option<&str>,
    ),
) {
    let make = |body: Entries| {
        let mut headers = vec![header("sec-fetch-site", str_lit("same-origin"))];
        headers.push(req.headers_spread.clone());
        headers.extend(req.content_type.iter().cloned());
        headers.push(header("cookie", ident("__cookie")));
        request_init(req.method, headers, body)
    };
    let options = make(body_init.clone());
    let raw_body_init: Entries = body_name
        .map(|n| vec![body_entry(ident(crate::emitter::ts_ident(n)))])
        .unwrap_or_default();
    let raw_options = make(raw_body_init);
    let keys_forged = TsExpr::object(vec![(
        "secret".to_string(),
        crate::ts_build::call(ident("__newSessionKey"), Vec::new()),
    )]);
    let issued_before_ttl = crate::ts_build::binary(
        TsBinaryOp::Sub,
        crate::ts_build::binary(
            TsBinaryOp::Sub,
            now_secs(),
            crate::ts_build::num_lit(session.ttl_secs),
        ),
        crate::ts_build::num_lit(3600),
    );
    let variants = [
        (
            "forged",
            session.pair(
                &keys_forged,
                crate::ts_build::str_lit("bynk-test"),
                now_secs(),
            ),
        ),
        (
            "expired",
            session.pair(
                &session.keys,
                crate::ts_build::str_lit("bynk-test"),
                issued_before_ttl,
            ),
        ),
    ];
    for (name, pair) in variants {
        let sign_stmt = const_("__cookie", pair);
        routes.push(sysdrive_driver(
            SysdriveKind::typed(name),
            sname,
            key,
            driver_params.to_vec(),
            body_stmts.clone(),
            url.clone(),
            options.clone(),
            binding,
            "__responseToUnauthOutcome",
            payload.clone(),
            false,
            vec![sign_stmt.clone()],
            false,
        ));
        if req.has_params {
            routes.push(sysdrive_driver(
                SysdriveKind::raw(name),
                sname,
                key,
                raw_params.to_vec(),
                Vec::new(),
                raw_url.clone(),
                raw_options.clone(),
                binding,
                "__responseToUnauthOutcome",
                payload.clone(),
                false,
                vec![sign_stmt],
                false,
            ));
        }
    }
}

/// Where a case's own `RequestHeaders` sit among a route's request headers. A body
/// route's `content-type` is the driver's default, so the case's headers go last and
/// a `Content-Type` of its own overrides it (#3276 `@form`; #3912 JSON and `@raw`):
/// how a case probes the route's `415`, or a listed `@raw` type. Otherwise they go first.
fn spread_order(content_type_overridable: bool, spread: &TsObjectEntry) -> (Entries, Entries) {
    if content_type_overridable {
        (Vec::new(), vec![spread.clone()])
    } else {
        (vec![spread.clone()], Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> SessionDriver {
        SessionDriver {
            keys: TsExpr::object(vec![("secret".to_string(), ident("__key"))]),
            actor: "User".to_string(),
            cookie: "__Host-session".to_string(),
            ttl_secs: 43200,
        }
    }

    #[test]
    fn the_session_pair_is_the_template_literal_the_drivers_always_sent() {
        let s = session();
        assert_eq!(
            bynk_ts::print_expr(&s.pair(&s.keys, ident("__sub"), now_secs())),
            "`__Host-session=${await __signSessionCookie({ secret: __key }, \"User\", __sub, 43200, (__bynkNow()) * 1000)}`"
        );
    }

    #[test]
    fn a_cookie_header_is_the_param_the_session_or_the_two_joined() {
        let s = session();
        let pair = s.pair(&s.keys, ident("__sub"), now_secs());
        let none = CookieHeader { param: None };
        let some = CookieHeader {
            param: Some(ident("cookies")),
        };
        let printed = |entries: Entries| bynk_ts::print_expr(&TsExpr::object_entries(entries));
        assert_eq!(printed(none.render(None)), "{}");
        assert_eq!(printed(some.render(None)), "{ \"cookie\": cookies }");
        assert_eq!(
            printed(none.render(Some(&pair))),
            format!("{{ \"cookie\": {} }}", bynk_ts::print_expr(&pair))
        );
        assert_eq!(
            printed(some.render(Some(&pair))),
            format!(
                "{{ \"cookie\": [cookies, {}].join(\"; \") }}",
                bynk_ts::print_expr(&pair)
            )
        );
    }
}
