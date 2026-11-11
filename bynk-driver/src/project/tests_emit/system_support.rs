//! System-suite support: HTTP and job drivers, signing, service forwarding and fan-out.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
use bynk_emit::emitter::RuntimeUse;
// #1759: every namespace a test module imports, from one definition.
use bynk_emit::emitter::emit::test_scaffold_ns as test_ns;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt,
    TsType, TsTypeMember,
};

/// #2073: whether the lowered TypeScript `src` names the identifier `name`,
/// as a whole word outside a member access (`x.Region` names a property) and
/// outside a string literal (an `expect`'s failure message quotes the source)
/// or a `//` or `/* */` comment. A template literal's text is skipped too, but
/// its `${…}` holes are code.
pub(super) fn names_ident(src: &str, name: &str) -> bool {
    let is_ident = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '$';
    // `code` is `src` with every literal character blanked, so offsets and
    // word boundaries are kept.
    let mut code = String::with_capacity(src.len());
    // `None`: code. `Some(q)`: inside a string quoted by `q`. A template's
    // hole pushes the template, and its braces, on `stack`.
    let mut quote: Option<char> = None;
    let mut stack: Vec<usize> = Vec::new(); // brace depth in each open hole
    let mut chars = src.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                if c == '\\' {
                    code.push(' ');
                    if chars.next().is_some() {
                        code.push(' ');
                    }
                    continue;
                }
                if q == '`' && c == '$' && chars.peek() == Some(&'{') {
                    chars.next();
                    code.push_str("  ");
                    stack.push(0);
                    quote = None;
                    continue;
                }
                if c == q {
                    quote = None;
                }
                code.push(' ');
            }
            None => match c {
                '"' | '\'' | '`' => {
                    quote = Some(c);
                    code.push(' ');
                }
                '/' if chars.peek() == Some(&'/') => {
                    // A line comment runs to the end of the line.
                    code.push(' ');
                    for d in chars.by_ref() {
                        if d == '\n' {
                            code.push('\n');
                            break;
                        }
                        code.push(' ');
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    // A block comment runs to the next `*/`.
                    chars.next();
                    code.push_str("  ");
                    let mut star = false;
                    for d in chars.by_ref() {
                        code.push(' ');
                        if star && d == '/' {
                            break;
                        }
                        star = d == '*';
                    }
                }
                '{' if !stack.is_empty() => {
                    *stack.last_mut().expect("non-empty") += 1;
                    code.push(c);
                }
                '}' if !stack.is_empty() => {
                    let depth = stack.last_mut().expect("non-empty");
                    if *depth == 0 {
                        // The hole closes; the template resumes.
                        stack.pop();
                        quote = Some('`');
                        code.push(' ');
                    } else {
                        *depth -= 1;
                        code.push(c);
                    }
                }
                _ => code.push(c),
            },
        }
    }
    code.match_indices(name).any(|(i, _)| {
        let before = code[..i].chars().next_back();
        let after = code[i + name.len()..].chars().next();
        !before.is_some_and(|c| is_ident(c) || c == '.') && !after.is_some_and(is_ident)
    })
}

/// #1486: `emitter/workers.rs`'s own `append_missing_bindings`'s sibling
/// duplicate — same reason that one and `emitter/workers_entry.rs`'s own copy
/// don't share a single implementation (each doc comment explains why for its
/// own file): folds `extra`'s comma-separated bindings (e.g.
/// [`bynk_emit::emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS`]) into `names` directly,
/// skipping anything already present under the same "strip an optional `type
/// ` prefix, compare bare names" dedup rule. `emit_test_module`/
/// `emit_integration_module`'s own runtime-import statement is now a real
/// `TsDecl::Import` built once every case has lowered (deferred, not spliced
/// into already-printed text the way the pre-#1486 code's own
/// `emitter::inject_runtime_imports` post-pass did — see each caller's own
/// doc comment) — this is that deferred construction's own merge step.
pub(super) fn append_missing_bindings(names: &mut Vec<String>, extra: &str) {
    fn bare(binding: &str) -> &str {
        binding
            .trim()
            .strip_prefix("type ")
            .unwrap_or(binding.trim())
    }
    let present: std::collections::HashSet<&str> = names.iter().map(|s| bare(s)).collect();
    let mut to_add: Vec<String> = Vec::new();
    for b in extra.split(',').map(str::trim) {
        if !b.is_empty() && !present.contains(bare(b)) && !to_add.iter().any(|a| bare(a) == bare(b))
        {
            to_add.push(b.to_string());
        }
    }
    names.extend(to_add);
}

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
/// `__bynkSignHs256`) is committed `.ts` source (`emitter/test_runtime/
/// jwt_signer.ts`, #1485), spliced in via [`jwt_signer_runtime_helpers`] and
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
    pub(super) claims: emitter::SystemHttpClaims,
}

/// #2135: what a `Signature`-secured route's drivers sign with — the actor's
/// declared seam and how the driver reads its secret (the case's `env` entry,
/// else the fixed test secret in `process.env`).
pub(super) struct SigDriver {
    pub(super) seam: bynk_check::actors::SignatureSeam,
    pub(super) secret_read: String,
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
    pub(super) fn headers(&self) -> String {
        let mut out = format!("{:?}: __sig.signature, ", self.seam.header);
        if let Some(ts) = &self.seam.timestamp_header {
            out.push_str(&format!("{ts:?}: __sig.timestamp, "));
        }
        out
    }

    /// `const __sig = await __bynkSignHmacSha256(<body>, <secret>, …);`.
    pub(super) fn sign(&self, body: &str, variant: SigVariant) -> TsStmt {
        let (secret, age) = match variant {
            SigVariant::Signed => (self.secret_read.clone(), 0),
            // #2155: a wrong secret derived from the real one, so it differs
            // from whatever the seam holds, `env` entry or test default.
            SigVariant::Forged => (format!("({} + \"!\")", self.secret_read), 0),
            SigVariant::Stale(age) => (self.secret_read.clone(), age),
        };
        TsStmt::const_stmt(
            TsBindingName::Ident("__sig".to_string()),
            None,
            await_expr(call(
                ident("__bynkSignHmacSha256"),
                vec![
                    ident(body),
                    ident(secret),
                    TsExpr::Lit(TsLit::Bool(self.seam.timestamp_header.is_some())),
                    num_lit(age.to_string()),
                ],
            )),
            None,
        )
    }
}

/// #2135, #2155: a signed driver's signing statements — a lone `Signature`
/// actor's signature, or a sum's per-call `__auth` — over `body`.
pub(super) fn signing_stmts(
    lone: &Option<SigDriver>,
    sum: &Option<SumAuth>,
    body: &str,
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
    Bearer(String),
    /// A `Signature` member.
    Signature(SigDriver),
}

impl SumAuth {
    /// The statements building `__auth`, the credential headers for the actor
    /// the call acts as, signing `body` where a member signs.
    pub(super) fn stmts(&self, body: &str) -> Vec<TsStmt> {
        let mut out = vec![TsStmt::raw(
            "const __auth: globalThis.Record<string, string> = {};",
            None,
        )];
        for (name, cred) in &self.members {
            let set = match cred {
                SumCredential::Bearer(secret_read) => format!(
                    "__auth[\"authorization\"] = `Bearer ${{await __bynkSignHs256({{ sub: __sub, exp: __bynkNow() + 3600 }}, {secret_read})}}`;"
                ),
                SumCredential::Signature(z) => {
                    let mut set = format!(
                        "const __sig = await __bynkSignHmacSha256({body}, {}, {}, 0); __auth[{:?}] = __sig.signature;",
                        z.secret_read,
                        z.seam.timestamp_header.is_some(),
                        z.seam.header
                    );
                    if let Some(ts) = &z.seam.timestamp_header {
                        set.push_str(&format!(" __auth[{ts:?}] = __sig.timestamp;"));
                    }
                    set
                }
            };
            out.push(TsStmt::raw(
                format!("if (__as === {name:?}) {{ {set} }}"),
                None,
            ));
        }
        out
    }

    /// #2864: the first Bearer member's secret read: what `by Forged` and
    /// `by Expired` sign a token with on a sum with no `Signature` member.
    pub(super) fn bearer(&self) -> Option<&str> {
        self.members.iter().find_map(|(_, c)| match c {
            SumCredential::Bearer(secret_read) => Some(secret_read.as_str()),
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
            claims: emitter::SystemHttpClaims::default(),
        };
    };
    let ns = test_ns(target);
    let binding = bynk_emit::emitter::wrangler::service_binding(target).name;
    let type_ns = format!("{ns}.");

    let mut routes: Vec<TsStmt> = Vec::new();
    let mut secrets: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    // #2135: whether some route is `Signature`-secured, so the module needs
    // the HMAC signer.
    let mut signs_hmac = false;
    // #3052: whether some route is `Oidc`-secured, so the module needs the
    // test issuer.
    let mut has_oidc = false;
    // #2019: each refinement actor's witness claims, signed when a case acts
    // as it on a refinement-guarded route.
    let mut claims = emitter::SystemHttpClaims {
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
                Some((name.clone(), bynk_check::actors::claim_witness_js(&pred)?))
            })
            .collect(),
    };

    // How a driver reads an actor's secret: the case's `env` entry (#2134),
    // else the fixed test secret in `process.env`.
    let secret_read_of = |secret: &str| {
        let process_read = format!(
            "(globalThis as {{ process?: {{ env?: globalThis.Record<string, string> }} }}).process?.env?.[{secret:?}]"
        );
        // #2138: a remote run's secret values come from the runner in one
        // JSON variable (`__bynkSecrets`).
        let process_read = if remote {
            format!("(__bynkSecrets[{secret:?}] ?? {process_read})")
        } else {
            process_read
        };
        if with_env {
            format!("(__bynkEnv[{secret:?}] ?? {process_read} ?? \"\")")
        } else {
            format!("({process_read} ?? \"\")")
        }
    };
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
            let key = bynk_emit::emitter::http_handler_method_name_ir(method, &path);
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
            runtime_use.note_request_header_pairs();
            let headers_spread = if declares_headers {
                "...__requestHeaderPairs(headers), "
            } else {
                "...(__extraHeaders ? __requestHeaderPairs(__extraHeaders, true) : {}), "
            };
            // A required slot ahead of `sub`: the call lowers `undefined` there
            // when the case sends none.
            let extra_headers_param = (!declares_headers).then(|| {
                declared.insert((
                    sname.clone(),
                    method.as_str().to_string(),
                    format!("{path}{}", emitter::EXTRA_HEADERS_ROUTE_SUFFIX),
                ));
                TsParam {
                    name: "__extraHeaders".to_string(),
                    ty: Some(TsType::named(format!(
                        "{} | undefined",
                        bynk_emit::emitter::ts_type_ref(
                            &bynk_syntax::ast::TypeRef::RequestHeaders(Default::default())
                        )
                    ))),
                    optional: false,
                }
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
            let (_path_ps, body_ps): (Vec<_>, Vec<_>) = h
                .params
                .iter()
                .filter(|p| !is_headers(p) && !is_query(p) && !is_cookies(p))
                .partition(|p| path_params.contains(&p.name.name.as_str()));
            // The query string the typed drivers append to the URL, and the one
            // the raw drivers append (a `Wire("?…")` text, or a `RequestQuery`).
            let (typed_query, raw_query) = match h.params.iter().find(|p| is_query(p)) {
                Some(p) => {
                    let var = bynk_emit::emitter::ts_ident(&p.name.name);
                    match &p.type_ref {
                        bynk_syntax::ast::TypeRef::RequestQuery(_) => {
                            runtime_use.note_request_query_search();
                            let s = format!("${{__requestQuerySearch({var})}}");
                            (s.clone(), s)
                        }
                        record => {
                            runtime_use.note_query_string();
                            let ser = bynk_ts::print_expr(
                                &bynk_emit::emitter::serialisation::serialise_expr_via(
                                    record,
                                    &var,
                                    &type_ns,
                                    runtime_use,
                                ),
                            );
                            (format!("${{__queryString({ser})}}"), format!("${{{var}}}"))
                        }
                    }
                }
                None => (String::new(), String::new()),
            };
            // #3277: the `Cookie` header the typed drivers send (a typed record's wire
            // form), and the one the raw drivers send (a `Wire` string).
            let cookies_param = h.params.iter().find(|p| is_cookies(p));
            let (typed_cookie, raw_cookie) =
                cookie_header_texts(cookies_param, &type_ns, runtime_use);
            if let Some(cp) = cookies_param {
                let key = (
                    sname.clone(),
                    method.as_str().to_string(),
                    format!("{path}{}", emitter::COOKIES_ROUTE_SUFFIX),
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
                        format!("{path}{}", emitter::QUERY_ROUTE_SUFFIX),
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
                            format!("{path}{}", emitter::FORM_ROUTE_SUFFIX),
                        ),
                        (idx, bp.type_ref.clone()),
                    );
                }
            }
            // The concrete URL: substitute each `:name` with its param.
            let concrete_path = {
                let mut s = String::new();
                for seg in path.split('/').filter(|x| !x.is_empty()) {
                    s.push('/');
                    if let Some(name) = seg.strip_prefix(':') {
                        s.push_str(&format!("${{{}}}", bynk_emit::emitter::ts_ident(name)));
                    } else {
                        s.push_str(seg);
                    }
                }
                if s.is_empty() {
                    s.push('/');
                }
                s
            };
            // The signed Authorization header, if the handler's `by` is Bearer.
            // #2019: a refinement-guarded route's signing drivers take the
            // claims the acting actor presents, signed next to `sub`.
            let mut signs_claims = false;
            let mut lone_bearer_read: Option<String> = None;
            // #3052: the expression for an `Oidc` route's trust roots.
            let mut oidc_trust: Option<String> = None;
            let (auth_header, _secret_name) = match bynk_check::actors::bearer_seam_for(
                h,
                &table.actors,
            ) {
                Some(seam) => {
                    secrets.insert(seam.secret.clone());
                    signs_claims = seam.authorization.is_some();
                    let claims_spread = if signs_claims {
                        claims.routes.insert((
                            sname.clone(),
                            method.as_str().to_string(),
                            path.clone(),
                        ));
                        "...__claims, "
                    } else {
                        ""
                    };
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
                    (
                        format!(
                            "\"authorization\": `Bearer ${{await __bynkSignHs256({{ {claims_spread}sub: __sub, exp: __bynkNow() + 3600 }}, {secret_read})}}`, "
                        ),
                        Some(seam.secret),
                    )
                }
                None => match bynk_check::actors::oidc_seam_for(h, &table.actors) {
                    Some(seam) => {
                        has_oidc = true;
                        // The actor's declared roots, replaced by its
                        // `trustFrom` variable's JSON when the case's `env`
                        // sets it, as the Worker's seam reads them.
                        let declared = format!(
                            "{{ issuer: {:?}, audience: {:?}, jwks: {:?} }}",
                            seam.issuer, seam.audience, seam.jwks
                        );
                        let trust = match &seam.trust_from {
                            Some(var) => {
                                format!("__bynkOidcTrustOf({declared}, {})", secret_read_of(var))
                            }
                            None => declared,
                        };
                        oidc_trust = Some(trust.clone());
                        signs_claims = true;
                        claims.oidc_routes.insert((
                            sname.clone(),
                            method.as_str().to_string(),
                            path.clone(),
                        ));
                        (
                            format!(
                                "\"authorization\": `Bearer ${{await __bynkOidcToken(__bynkOidcRemote, {trust}, {{ sub: __sub, claims: __claims, kind: \"valid\" }})}}`, "
                            ),
                            None,
                        )
                    }
                    None => (String::new(), None),
                },
            };
            // #2155: an actor-sum route's credential depends on the member a
            // call acts as, so its drivers take that actor (`__as`) and build
            // the credential per call, rather than the primary member's.
            let route_key = (sname.clone(), method.as_str().to_string(), path.clone());
            let sum = bynk_check::actors::sum_members_for(h, &table.actors).and_then(|members| {
                let members: Vec<(String, SumCredential)> = members
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
            });
            let is_sum = h.by_clause.as_ref().is_some_and(|by| by.is_sum());
            let (auth_header, signs_claims) = if is_sum {
                (String::new(), false)
            } else {
                (auth_header, signs_claims)
            };
            // #2864: the secret a `by Forged` / `by Expired` Bearer token is
            // signed against — the lone Bearer actor's, or a sum's first
            // Bearer member's.
            let bearer_read: Option<String> = if is_sum {
                sum.as_ref().and_then(SumAuth::bearer).map(str::to_string)
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
            let sig_header = match (&lone_sig, &sum) {
                (Some(z), _) => z.headers(),
                (None, Some(_)) => "...__auth, ".to_string(),
                (None, None) => String::new(),
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
            let (body_stmt, body_init): (Option<TsStmt>, &str) = match body_arg {
                Some(p) if is_multipart => {
                    runtime_use.note_multipart_form();
                    let ser = bynk_ts::print_expr(
                        &bynk_emit::emitter::serialisation::serialise_expr_via(
                            &p.type_ref,
                            &bynk_emit::emitter::ts_ident(&p.name.name),
                            &type_ns,
                            runtime_use,
                        ),
                    );
                    let specs = bynk_emit::emitter::form_body_specs(h, &table.types)
                        .unwrap_or_default()
                        .iter()
                        .map(|(name, kind, list, option)| {
                            format!(
                                "{{ name: {name:?}, kind: {kind:?}, list: {list}, option: {option} }}"
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    let stmt = TsStmt::const_stmt(
                        TsBindingName::Ident("__body".to_string()),
                        None,
                        call(
                            ident("__multipartForm"),
                            vec![ident(ser), ident(format!("[{specs}] as const"))],
                        ),
                        None,
                    );
                    (Some(stmt), "body: __body, ")
                }
                Some(p) if is_form => {
                    runtime_use.note_query_string();
                    let ser = bynk_ts::print_expr(
                        &bynk_emit::emitter::serialisation::serialise_expr_via(
                            &p.type_ref,
                            &bynk_emit::emitter::ts_ident(&p.name.name),
                            &type_ns,
                            runtime_use,
                        ),
                    );
                    let stmt = TsStmt::const_stmt(
                        TsBindingName::Ident("__body".to_string()),
                        None,
                        method_call(
                            call(ident("__queryString"), vec![ident(ser)]),
                            "slice",
                            vec![num_lit("1")],
                        ),
                        None,
                    );
                    (Some(stmt), "body: __body, ")
                }
                Some(p) if raw_type.is_some() => {
                    let stmt = TsStmt::const_stmt(
                        TsBindingName::Ident("__body".to_string()),
                        None,
                        ident(bynk_emit::emitter::ts_ident(&p.name.name)),
                        None,
                    );
                    (Some(stmt), "body: __body as BufferSource, ")
                }
                Some(p) => {
                    // #1435 (Arc E slice 1): `serialise_expr_via` now returns
                    // a real `bynk_ts::TsExpr` — this call site is still
                    // `String`-based (not yet converted, out of this
                    // slice's scope), so it prints at the boundary
                    // (`bynk_ts::print_expr`), the same treatment
                    // `lower.rs`'s own call sites use.
                    let ser = bynk_ts::print_expr(
                        &bynk_emit::emitter::serialisation::serialise_expr_via(
                            &p.type_ref,
                            &bynk_emit::emitter::ts_ident(&p.name.name),
                            &type_ns,
                            runtime_use,
                        ),
                    );
                    let stmt = TsStmt::const_stmt(
                        TsBindingName::Ident("__body".to_string()),
                        None,
                        call(
                            member(ident("globalThis.JSON"), "stringify"),
                            vec![ident(ser)],
                        ),
                        None,
                    );
                    (Some(stmt), "body: __body, ")
                }
                None => (None, ""),
            };
            // The response payload deserialiser: the `T` of `Effect[HttpResult[T]]`.
            let payload_deser = match strip_effect_httpresult(&h.return_type) {
                // #1435 (Arc E slice 1): boundary-print, same treatment as
                // `ser` above.
                Some(inner) => {
                    bynk_ts::print_expr(&bynk_emit::emitter::serialisation::deserialise_ref_via(
                        inner,
                        &type_ns,
                        runtime_use,
                    ))
                }
                None => format!("{type_ns}__deserialise_unit"),
            };
            // Driver params mirror the handler's params (path params, then body).
            let driver_params: Vec<TsParam> = h
                .params
                .iter()
                .map(|p| TsParam {
                    name: bynk_emit::emitter::ts_ident(&p.name.name),
                    ty: Some(TsType::named(driver_param_ty(&p.type_ref, &ns))),
                    optional: false,
                })
                .chain(extra_headers_param.clone())
                .collect();
            let raw_params: Vec<TsParam> = h
                .params
                .iter()
                .map(|p| TsParam {
                    name: bynk_emit::emitter::ts_ident(&p.name.name),
                    ty: Some(TsType::named(
                        if is_headers(&p)
                            || matches!(p.type_ref, bynk_syntax::ast::TypeRef::RequestQuery(_))
                        {
                            driver_param_ty(&p.type_ref, &ns)
                        } else {
                            "string".to_string()
                        },
                    )),
                    optional: false,
                })
                .chain(extra_headers_param)
                .collect();
            let content_type = match (body_arg.is_some(), raw_type) {
                (true, Some(ct)) => format!("\"content-type\": {ct:?}, "),
                (true, None) if is_multipart => String::new(),
                (true, None) if is_form => {
                    "\"content-type\": \"application/x-www-form-urlencoded\", ".to_string()
                }
                (true, None) => "\"content-type\": \"application/json\", ".to_string(),
                (false, _) => String::new(),
            };
            let url = template(format!("https://test{concrete_path}{typed_query}"));
            let raw_url = template(format!("https://test{concrete_path}{raw_query}"));
            // Decision A (#1407): the request-init options object (`{ method,
            // headers: {...}, body, }`) keeps its own hand-formatted text —
            // every branch below bakes in an unconditional trailing `, ` before
            // its closing brace, and prints `{ }` (one space, not
            // `TsExpr::Object`'s own tight `{}`) when a sub-object has zero
            // entries — a shape `TsExpr::Object`'s general single-line algebra
            // (comma-separated, no trailing comma, tight `{}` for zero
            // entries) cannot reproduce byte-for-byte. The same "odd, one-off
            // shape stays text" call this track has made before (Decision B,
            // #1327; Decision C, #1359). Everything AROUND it — the function
            // declaration, its params, the `new Request(...)` call, the
            // `await ... .fetch(...)` chain, the final `return
            // responseTo*(...)` — is real `bynk_ts` structure.
            // #3276: a `@form` route's `content-type` is the driver's default, so a
            // case's own `Content-Type` (a trailing `RequestHeaders`) overrides it,
            // which is how a case probes the route's `415`.
            let (spread_first, spread_last) = if is_form {
                ("", headers_spread)
            } else {
                (headers_spread, "")
            };
            let typed_options = ident(format!(
                "{{ method: {method:?}, headers: {{ {spread_first}{content_type}{spread_last}{typed_cookie}{auth_header}{sig_header}}}, {body_init}}}",
                method = method.as_str(),
            ));
            // #2135: the bytes a signing driver signs: what it sends.
            let typed_body = if body_arg.is_some() { "__body" } else { "\"\"" };
            let raw_body = match body_ps.first() {
                Some(p) => bynk_emit::emitter::ts_ident(&p.name.name),
                None => "\"\"".to_string(),
            };
            // #2685: the typed driver names its route to the decoder, which a
            // smoke run checks the answer's status against: the context, the
            // service, the method and the path, as the driver's smoke table
            // keys them. It reads that table only in a smoke run.
            let typed_payload = if smoke {
                format!(
                    "{payload_deser}, {:?}",
                    format!("{target} {sname} {} {path}", method.as_str())
                )
            } else {
                payload_deser.clone()
            };
            routes.push(sysdrive_driver(
                "",
                sname,
                &key,
                driver_params.clone(),
                body_stmt.clone(),
                url.clone(),
                typed_options.clone(),
                &binding,
                "__responseToHttpResult",
                &typed_payload,
                signs_claims,
                signing_stmts(&lone_sig, &sum, typed_body),
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
            if !h.params.is_empty() {
                let raw_body_init = match body_ps.first() {
                    Some(p) => format!("body: {}, ", bynk_emit::emitter::ts_ident(&p.name.name)),
                    None => String::new(),
                };
                // The `Wire` text is text, so a raw route is sent it as `text/plain`:
                // a route declaring another media type answers `415`, which is how a
                // case probes it, and a `"*/*"` route still takes the text.
                let raw_content_type = if raw_type.is_some() {
                    "\"content-type\": \"text/plain; charset=utf-8\", ".to_string()
                } else if is_multipart && body_arg.is_some() {
                    // A `Wire` body is the multipart text itself, framed with this
                    // boundary (`--bynk-boundary`).
                    "\"content-type\": \"multipart/form-data; boundary=bynk-boundary\", "
                        .to_string()
                } else {
                    content_type.clone()
                };
                let raw_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {spread_first}{raw_content_type}{spread_last}{raw_cookie}{auth_header}{sig_header}}}, {raw_body_init}}}",
                    method = method.as_str(),
                ));
                routes.push(sysdrive_driver(
                    "raw_",
                    sname,
                    &key,
                    raw_params.clone(),
                    None,
                    raw_url.clone(),
                    raw_options,
                    &binding,
                    "__responseToHttpOutcome",
                    &payload_deser,
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
            let secured = !auth_header.is_empty() || sig.is_some() || sum.is_some();
            if secured {
                let noauth_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}}}, {body_init}}}",
                    method = method.as_str(),
                ));
                routes.push(sysdrive_driver(
                    "noauth_",
                    sname,
                    &key,
                    driver_params.clone(),
                    body_stmt.clone(),
                    url.clone(),
                    noauth_options,
                    &binding,
                    "__responseToUnauthOutcome",
                    &payload_deser,
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
                let raw_body_init = match body_ps.first() {
                    Some(p) => format!("body: {}, ", bynk_emit::emitter::ts_ident(&p.name.name)),
                    None => String::new(),
                };
                let rawnoauth_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}}}, {raw_body_init}}}",
                    method = method.as_str(),
                ));
                routes.push(sysdrive_driver(
                    "rawnoauth_",
                    sname,
                    &key,
                    raw_params.clone(),
                    None,
                    raw_url.clone(),
                    rawnoauth_options,
                    &binding,
                    "__responseToUnauthOutcome",
                    &payload_deser,
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
                let forged_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}{bad_sig_header}}}, {body_init}}}",
                    method = method.as_str(),
                ));
                let mut variants = vec![("forged", SigVariant::Forged)];
                if let Some(age) = z.stale_age() {
                    variants.push(("stale", SigVariant::Stale(age)));
                }
                for (name, variant) in variants {
                    routes.push(sysdrive_driver(
                        &format!("{name}_"),
                        sname,
                        &key,
                        driver_params.clone(),
                        body_stmt.clone(),
                        url.clone(),
                        forged_options.clone(),
                        &binding,
                        "__responseToUnauthOutcome",
                        &payload_deser,
                        false,
                        vec![z.sign(typed_body, variant)],
                        false,
                    ));
                    if !h.params.is_empty() {
                        let raw_body_init = match body_ps.first() {
                            Some(p) => {
                                format!("body: {}, ", bynk_emit::emitter::ts_ident(&p.name.name))
                            }
                            None => String::new(),
                        };
                        let options = ident(format!(
                            "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}{bad_sig_header}}}, {raw_body_init}}}",
                            method = method.as_str(),
                        ));
                        routes.push(sysdrive_driver(
                            &format!("raw{name}_"),
                            sname,
                            &key,
                            raw_params.clone(),
                            None,
                            raw_url.clone(),
                            options,
                            &binding,
                            "__responseToUnauthOutcome",
                            &payload_deser,
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
            if let Some(secret_read) = bearer_read.as_deref() {
                let bearer_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}\"authorization\": __token, }}, {body_init}}}",
                    method = method.as_str(),
                ));
                let raw_bearer_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}\"authorization\": __token, }}, {raw_body_init}}}",
                    method = method.as_str(),
                    raw_body_init = match body_ps.first() {
                        Some(p) =>
                            format!("body: {}, ", bynk_emit::emitter::ts_ident(&p.name.name)),
                        None => String::new(),
                    },
                ));
                let mut variants = vec![("expired", format!("({secret_read})"), "-")];
                if sig.is_none() {
                    variants.push(("forged", format!("({secret_read} + \"!\")"), "+"));
                }
                for (name, secret, sign) in variants {
                    // A valid `sub`, so only the signature (`Forged`) or the
                    // `exp` (`Expired`) is wrong. The `sub` is fixed rather
                    // than the call's identity: `by Forged` names none.
                    let sign_stmt = TsStmt::raw(
                        format!(
                            "const __token = `Bearer ${{await __bynkSignHs256({{ sub: \"bynk-test\", exp: __bynkNow() {sign} 3600 }}, {secret})}}`;"
                        ),
                        None,
                    );
                    routes.push(sysdrive_driver(
                        &format!("{name}_"),
                        sname,
                        &key,
                        driver_params.clone(),
                        body_stmt.clone(),
                        url.clone(),
                        bearer_options.clone(),
                        &binding,
                        "__responseToUnauthOutcome",
                        &payload_deser,
                        false,
                        vec![sign_stmt.clone()],
                        false,
                    ));
                    if !h.params.is_empty() {
                        routes.push(sysdrive_driver(
                            &format!("raw{name}_"),
                            sname,
                            &key,
                            raw_params.clone(),
                            None,
                            raw_url.clone(),
                            raw_bearer_options.clone(),
                            &binding,
                            "__responseToUnauthOutcome",
                            &payload_deser,
                            false,
                            vec![sign_stmt],
                            false,
                        ));
                    }
                }
            }
            // #3052: on an `Oidc` route, `by Forged` (signed by a key the
            // JWKS does not publish), `by Expired`, `by WrongIssuer` and
            // `by WrongAudience`, typed and raw, each decoding a `401` to
            // `Rejected(Unauthorized)`; and (#3225) `by WrongAlgorithm` and
            // `by NoSubject`.
            if let Some(trust) = oidc_trust.as_deref() {
                let oidc_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}\"authorization\": __token, }}, {body_init}}}",
                    method = method.as_str(),
                ));
                let raw_oidc_options = ident(format!(
                    "{{ method: {method:?}, headers: {{ {headers_spread}{content_type}\"authorization\": __token, }}, {raw_body_init}}}",
                    method = method.as_str(),
                    raw_body_init = match body_ps.first() {
                        Some(p) =>
                            format!("body: {}, ", bynk_emit::emitter::ts_ident(&p.name.name)),
                        None => String::new(),
                    },
                ));
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
                    let sign_stmt = TsStmt::raw(
                        format!(
                            "const __token = `Bearer ${{await __bynkOidcToken(__bynkOidcRemote, {trust}, {{ sub: \"bynk-test\", claims: {{}}, kind: \"{name}\" }})}}`;"
                        ),
                        None,
                    );
                    routes.push(sysdrive_driver(
                        &format!("{name}_"),
                        sname,
                        &key,
                        driver_params.clone(),
                        body_stmt.clone(),
                        url.clone(),
                        oidc_options.clone(),
                        &binding,
                        "__responseToUnauthOutcome",
                        &payload_deser,
                        false,
                        vec![sign_stmt.clone()],
                        false,
                    ));
                    if !h.params.is_empty() {
                        routes.push(sysdrive_driver(
                            &format!("raw{name}_"),
                            sname,
                            &key,
                            raw_params.clone(),
                            None,
                            raw_url.clone(),
                            raw_oidc_options.clone(),
                            &binding,
                            "__responseToUnauthOutcome",
                            &payload_deser,
                            false,
                            vec![sign_stmt],
                            false,
                        ));
                    }
                }
                // #3225: the two-token principals. The driver presents each
                // token in order and returns the first outcome that is not the
                // seam's refusal, so a seam that admits either one fails the
                // case's `Rejected(Unauthorized)`; when every token is refused
                // it returns that refusal. Typed and raw, like the others.
                let prefixes: &[&str] = if h.params.is_empty() {
                    &[""]
                } else {
                    &["", "raw"]
                };
                for (principal, tokens) in [
                    ("wrongalgorithm", ["algnone", "alghs256"]),
                    ("nosubject", ["emptysub", "nosub"]),
                ] {
                    for raw in prefixes {
                        let driver = |kind: &str| format!("__sysdrive_{raw}{kind}_{sname}_{key}");
                        routes.push(TsStmt::raw(
                            format!(
                                "async function {name}(...__a: Parameters<typeof {first}>) {{\n  const __first = await {first}(...__a);\n  if (__first.tag !== \"Rejected\" || __first.value.tag !== \"Unauthorized\") return __first;\n  return await {second}(...__a);\n}}",
                                name = driver(principal),
                                first = driver(tokens[0]),
                                second = driver(tokens[1]),
                            ),
                            None,
                        ));
                    }
                }
            }
        }
        // #707: one generic wrong-method driver per service — drives an arbitrary
        // `(method, path)` (an existing path, an undeclared method) and decodes
        // the router's `405` fall-through to `Rejected(MethodNotAllowed)`. The
        // handler never runs, so there is no body to serialise and the payload
        // deserialiser is unused (the `405` takes the `Rejected` arm). Unlike the
        // per-route drivers' own options object (Decision A above), `{ method }`
        // is a fixed single shorthand entry — never conditionally empty — so it
        // matches `TsExpr::Object`'s own real single-line rendering exactly and
        // is built as a real node, not opaque text.
        let wrongmethod = TsStmt::decl(
            TsDecl::Function {
                name: format!("__sysdrive_wrongmethod_{sname}"),
                generics: Vec::new(),
                params: vec![
                    TsParam {
                        name: "method".to_string(),
                        ty: Some(TsType::named("string")),
                        optional: false,
                    },
                    TsParam {
                        name: "path".to_string(),
                        ty: Some(TsType::named("string")),
                        optional: false,
                    },
                ],
                return_type: None,
                body: vec![
                    TsStmt::const_stmt(
                        TsBindingName::Ident("__h".to_string()),
                        None,
                        call(ident("__makeHarness"), vec![]),
                        None,
                    ),
                    TsStmt::const_stmt(
                        TsBindingName::Ident("__req".to_string()),
                        None,
                        TsExpr::New {
                            callee: Box::new(ident("globalThis.Request")),
                            args: vec![
                                TsExpr::template_lit(
                                    vec!["https://test".to_string(), String::new()],
                                    vec![ident("path")],
                                ),
                                TsExpr::object_entries(vec![TsObjectEntry::Shorthand(
                                    "method".to_string(),
                                )]),
                            ],
                        },
                        None,
                    ),
                    TsStmt::const_stmt(
                        TsBindingName::Ident("__res".to_string()),
                        None,
                        await_expr(method_call(
                            member(member(ident("__h"), "env"), &binding),
                            "fetch",
                            vec![ident("__req")],
                        )),
                        None,
                    ),
                    TsStmt::return_stmt(
                        Some(call(
                            ident("__responseToHttpOutcome"),
                            vec![
                                ident("__res"),
                                TsExpr::Arrow {
                                    params: vec![TsParam {
                                        name: "__j".to_string(),
                                        ty: Some(TsType::named("__JsonValue")),
                                        optional: false,
                                    }],
                                    is_async: false,
                                    generics: Vec::new(),
                                    return_type: None,
                                    body: Box::new(TsArrowBody::Expr(Box::new(call(
                                        ident("Ok"),
                                        vec![TsExpr::As {
                                            expr: Box::new(ident("__j")),
                                            ty: TsType::named("never"),
                                        }],
                                    )))),
                                },
                            ],
                        )),
                        None,
                    ),
                ],
                is_async: true,
                inline: false,
            },
            None,
        );
        routes.push(wrongmethod);
    }

    if http_services.is_empty() {
        return SystemHttpSupport {
            code: Vec::new(),
            http_services,
            declared_routes: declared,
            route_body,
            type_ns: String::new(),
            claims: emitter::SystemHttpClaims::default(),
        };
    }

    // #1485: the HS256 signer block — a monotonic clock the signer's `exp`
    // uses (kept out of `bundle`d runtime), plus the test-only HS256 signer
    // itself (never in the deployable app; e2e owns real auth) — now lives
    // at `emitter/test_runtime/jwt_signer.ts`, the same committed-`.ts`-file
    // treatment `expectation_runtime_helpers`/`stub_runtime_helpers` already
    // get, rather than an unexplained raw string literal with `\`-continuations.
    let mut code = vec![TsStmt::raw(jwt_signer_runtime_helpers(), None)];
    // #3052: the test issuer an `Oidc` route's drivers mint tokens with.
    if has_oidc {
        let mut issuer = TsStmt::raw(
            format!(
                "const __bynkOidcRemote: boolean = {remote};\n{}",
                include_str!("../test_runtime/oidc_issuer.ts")
            ),
            None,
        );
        issuer.no_blank_before = true;
        code.push(issuer);
    }
    if signs_hmac {
        let mut signer = TsStmt::raw(
            include_str!("../test_runtime/hmac_signer.ts").to_string(),
            None,
        );
        signer.no_blank_before = true;
        code.push(signer);
    }
    // Set each secret the target's actors read, so the real Bearer seam verifies.
    // P7.2: `Record<string, string>` — same reasoning as `__bynkSignHs256`'s own
    // `secret: string` parameter in `emitter/test_runtime/jwt_signer.ts`.
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
            str_lit("__bynk_test_secret"),
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

/// #2136: the cron and queue drivers of a `system` suite's target, and the
/// services they cover ([`emitter::SystemJobServices`]).
pub(super) struct SystemJobSupport {
    pub(super) code: Vec<TsStmt>,
    pub(super) services: emitter::SystemJobServices,
}

/// #2136: one driver per cron handler and per queue service of the `system`
/// target, calling the participant Worker's real entry point with a
/// platform-shaped event and the harness's execution-context double, which it
/// settles (ADR 0478). The runtime half lives at `emitter/test_runtime/jobs.ts`.
///
/// - `__sysdrive_schedule_<svc>_<index>(at?)` calls `scheduled({ cron,
///   scheduledTime }, env, ctx)` and yields the `Result` the test build's
///   entry stored on `ctx` (`scheduled_result_hook`). A handler without the
///   scheduled-time parameter is fired at `0`. A handler that throws fails the
///   case, as it would at `unit`.
/// - `__sysdrive_message_<q>(m)` serialises `m` as the producer does, sends it
///   through a JSON round trip, and calls `queue(batch, env)` with a
///   one-message batch whose `ack()` / `retry()` the double records, yielding
///   `Ack` or `Retry(reason)` (the reason the entry logs).
pub(super) fn emit_system_job_support(
    target: &str,
    unit_tables: &HashMap<String, UnitTable>,
    runtime_use: &RuntimeUse,
    target_sends: bool,
) -> SystemJobSupport {
    use bynk_model::{IrHandlerKind, lower_handler_kind_ir};
    use bynk_syntax::ast::ServiceProtocol;
    let mut services = emitter::SystemJobServices::default();
    let mut drivers: Vec<TsStmt> = Vec::new();
    let Some(table) = unit_tables.get(target) else {
        return SystemJobSupport {
            code: drivers,
            services,
        };
    };
    let ns = test_ns(target);
    let worker = format!("worker_{ns}");
    let env = format!("__h.env_{ns}");
    let type_ns = format!("{ns}.");
    let mut svc_names: Vec<&String> = table.services.keys().collect();
    svc_names.sort();
    for sname in svc_names {
        let decl = &table.services[sname];
        // M-1's domain clause (ADR 0735; crate-hygiene B4, #3012): this
        // function holds the target's symbol table, not its `TypedCommons`, so it
        // doesn't hold `bynk_model::lower_protocol_ir`'s input.
        match &decl.protocol {
            ServiceProtocol::Cron => {
                let mut exprs = Vec::new();
                for h in &decl.handlers {
                    let IrHandlerKind::Cron { expr } = lower_handler_kind_ir(&h.kind) else {
                        continue;
                    };
                    let idx = exprs.len();
                    let key = bynk_emit::emitter::cron_handler_method_name(sname, idx);
                    let (params, at) = match h.params.first() {
                        Some(p) => (
                            format!("at: {}", driver_param_ty(&p.type_ref, &ns)),
                            "at".to_string(),
                        ),
                        None => (String::new(), "0".to_string()),
                    };
                    drivers.push(TsStmt::raw(
                        format!(
                            "async function __sysdrive_schedule_{sname}_{idx}({params}) {{\n  const __h = __makeHarness();\n  return __bynkScheduled<Awaited<ReturnType<typeof {ns}.{sname}.{key}>>>({expr:?}, (exec) => {worker}.scheduled({{ cron: {expr:?}, scheduledTime: {at} }}, {env}, exec));\n}}\n"
                        ),
                        None,
                    ));
                    exprs.push(expr);
                }
                services.cron.insert(sname.clone(), exprs);
            }
            ServiceProtocol::Queue { name } => {
                let Some(h) = decl
                    .handlers
                    .iter()
                    .find(|h| matches!(lower_handler_kind_ir(&h.kind), IrHandlerKind::Message))
                else {
                    continue;
                };
                let Some(p) = h.params.first() else {
                    continue;
                };
                let key = bynk_emit::emitter::queue_handler_method_name(sname, 0);
                let param = bynk_emit::emitter::ts_ident(&p.name.name);
                let body =
                    bynk_ts::print_expr(&bynk_emit::emitter::serialisation::serialise_expr_via(
                        &p.type_ref,
                        &param,
                        &type_ns,
                        runtime_use,
                    ));
                let exec_arg = if target_sends { ", exec" } else { "" };
                drivers.push(TsStmt::raw(
                    format!(
                        "async function __sysdrive_message_{sname}({param}: {ty}) {{\n  const __h = __makeHarness();\n  return __bynkDeliver<Awaited<ReturnType<typeof {ns}.{sname}.{key}>>>({name:?}, {body}, (batch, exec) => {worker}.queue(batch, {env}{exec_arg}));\n}}\n",
                        ty = driver_param_ty(&p.type_ref, &ns),
                    ),
                    None,
                ));
                services.queue.insert(sname.clone());
            }
            _ => {}
        }
    }
    let mut code = Vec::new();
    if !drivers.is_empty() {
        code.push(TsStmt::raw(
            include_str!("../test_runtime/jobs.ts").to_string(),
            None,
        ));
        code.extend(drivers);
    }
    SystemJobSupport { code, services }
}

/// Type a system-http driver parameter (v0.182): a named type is reached
/// through the target's handler namespace (`todos.AddRequest`); primitives and
/// compound shapes fall back to `unknown` — the driver only forwards the value
/// to the namespace's `serialise_*`, and the Bynk checker already typed the call.
pub(super) fn driver_param_ty(t: &bynk_syntax::ast::TypeRef, _ns: &str) -> String {
    use bynk_syntax::ast::TypeRef;
    match t {
        // A named type may embed a branded refined field (`AddRequest.title:
        // Title`), and the case body passes a plain object literal — the Bynk
        // checker already type-checked the call's args against the handler
        // (Slice A), so the driver forwards the value to the namespace's
        // `serialise_*`. Primitives stay precise.
        TypeRef::Named(_) => "any".to_string(),
        other => bynk_emit::emitter::ts_type_ref(other),
    }
}

/// Extract `T` from a handler return type `Effect[HttpResult[T]]` (v0.182).
pub(super) fn strip_effect_httpresult(
    t: &bynk_syntax::ast::TypeRef,
) -> Option<&bynk_syntax::ast::TypeRef> {
    use bynk_syntax::ast::TypeRef;
    let inner = match t {
        TypeRef::Effect(b, _) => b.as_ref(),
        other => other,
    };
    match inner {
        TypeRef::HttpResult(payload, _) => Some(payload.as_ref()),
        _ => None,
    }
}

/// A `{ fetch: (req: Request) => worker_X.fetch(req, env_X) } as ServiceBinding`
/// expression — the identical shape both the per-participant and root-env
/// binding wiring in [`emit_integration_harness`] build, differing only in
/// which worker/env pair they close over.
pub(super) fn service_binding_forward(worker_ident: &str, env_ident: &str, sends: bool) -> TsExpr {
    let req_param = TsParam {
        name: "req".to_string(),
        ty: Some(TsType::named("globalThis.Request")),
        optional: false,
    };
    // #1994: a Worker whose handlers send with `~>` takes the execution
    // context as `fetch`'s third argument; the harness hands it a collecting
    // double and settles the sends before the response reaches the case.
    let forward = if sends {
        call(
            ident("__bynkFetchSettled"),
            vec![TsExpr::Arrow {
                params: vec![TsParam {
                    name: "exec".to_string(),
                    ty: None,
                    optional: false,
                }],
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
/// [`bynk_emit::emitter::wrangler::foreign_service_double_body`]'s 503, as the
/// double `bynk dev` binds by default does. A case that needs another answer
/// stubs the instance's `send`.
pub(super) fn foreign_service_double(binding: &str, worker: &str) -> TsExpr {
    let response = TsExpr::New {
        callee: Box::new(ident("globalThis.Response")),
        args: vec![
            str_lit(bynk_emit::emitter::wrangler::foreign_service_double_body(
                binding, worker,
            )),
            TsExpr::object(vec![(
                "status".to_string(),
                num_lit(bynk_emit::emitter::wrangler::FOREIGN_SERVICE_DOUBLE_STATUS.to_string()),
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

/// #1959: the harness's import alias for participant `ns`'s events fan-out
/// module, shared by the import and the binding that constructs from it.
pub(super) fn fanout_alias(ns: &str) -> String {
    format!("fanout_{ns}")
}

/// The name a system test case calls a participant as: its cases lower in a
/// commons of this name ([`integration_typed_commons`]), and a cross-context
/// call stamps the calling commons' name. I2 (#1971): the harness gives every
/// participant the local key for the edge from it.
pub(super) const HARNESS_CALLER: &str = "integration";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_bindings_that_are_absent() {
        let mut names = vec!["Ok".to_string(), "Err".to_string()];
        append_missing_bindings(&mut names, emitter::BYTES_RUNTIME_IMPORTS);
        assert!(names.iter().any(|n| n == "__bynkBytesEqual"), "{names:?}");
        assert!(
            names.iter().any(|n| n == "__bynkBytesDecodeUtf8"),
            "{names:?}"
        );
    }

    #[test]
    fn skips_bindings_already_present() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
        ];
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names.iter().filter(|n| n.as_str() == "Ok").count(),
            1,
            "`Ok` was already present and must not repeat: {names:?}"
        );
        assert_eq!(
            names,
            vec!["Ok", "Err", "type Result", "type __BoundaryError"],
        );
    }

    /// The bare name is what collides, so `type BoundaryError` must match an
    /// existing `BoundaryError`.
    #[test]
    fn matches_a_type_prefixed_group_binding_against_a_bare_one() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
            "__BoundaryError".to_string(),
        ];
        let before = names.clone();
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names, before,
            "every binding was already present, so `names` is untouched"
        );
    }

    /// …and the other direction: a bare group binding against an existing
    /// `type`-prefixed one. This is the case that would regress if `bare`
    /// were applied to only one side of the comparison.
    #[test]
    fn matches_a_bare_group_binding_against_a_type_prefixed_one() {
        let mut names = vec!["type Ok".to_string(), "type Err".to_string()];
        let before = names.clone();
        append_missing_bindings(&mut names, ", Ok, Err");
        assert_eq!(
            names, before,
            "a bare binding must match an existing `type`-prefixed one: {names:?}"
        );
    }

    /// The two calls run back to back over the same `names`, so the second
    /// sees the first's own output as `present` — the overlap between the
    /// groups (`Ok`, `Err`, `type Result`) must not double up.
    #[test]
    fn composes_across_two_sequential_calls() {
        let mut names = vec![
            "Ok".to_string(),
            "Err".to_string(),
            "type Result".to_string(),
        ];
        append_missing_bindings(&mut names, emitter::BOUNDARY_CODEC_RUNTIME_IMPORTS);
        append_missing_bindings(&mut names, emitter::JSON_CODEC_RUNTIME_IMPORTS);
        assert_eq!(
            names,
            vec![
                "Ok",
                "Err",
                "type Result",
                "type __BoundaryError",
                "type __JsonValue",
                "type DecodeError",
            ],
            "the shared bindings must be added once: {names:?}"
        );
    }

    #[test]
    fn names_ident_ignores_literals_and_members_but_reads_template_holes() {
        // #2073 review: a name in a string, a template's text or after a `.`
        // is not a reference; one in a template's `${…}` hole is.
        assert!(names_ident("const r = Region.Here;", "Region"));
        assert!(!names_ident("x.Region", "Region"));
        assert!(!names_ident("__serialise_Region(v)", "Region"));
        assert!(!names_ident("fail(\"expect r == Region.Here\")", "Region"));
        assert!(!names_ident("fail('Region')", "Region"));
        assert!(!names_ident("fail(`expected Region`)", "Region"));
        assert!(names_ident("fail(`got ${show(Region.Here)}`)", "Region"));
        assert!(names_ident(
            "fail(`a ${ { k: 1 }.k } b`) + Region.Here",
            "Region"
        ));
        assert!(!names_ident("fail(`a ${ { k: 1 }.k } Region`)", "Region"));
        assert!(!names_ident("fail(\"a \\\" Region\")", "Region"));
        assert!(!names_ident("x(); // Region\n", "Region"));
        assert!(names_ident("x(); // c\nRegion.Here", "Region"));
        assert!(!names_ident("/* Region */ x()", "Region"));
        assert!(names_ident("/* a */ Region.Here", "Region"));
    }
}

/// #3277: the `Cookie` header the typed system drivers send (a typed record's wire
/// form) and the one the raw drivers send (a `Wire` string), for a handler's
/// `cookies` parameter; empty when it has none.
fn cookie_header_texts(
    param: Option<&bynk_syntax::ast::Param>,
    type_ns: &str,
    runtime_use: &RuntimeUse,
) -> (String, String) {
    let Some(p) = param else {
        return (String::new(), String::new());
    };
    let var = bynk_emit::emitter::ts_ident(&p.name.name);
    runtime_use.note_cookie_string();
    let ser = bynk_ts::print_expr(&bynk_emit::emitter::serialisation::serialise_expr_via(
        &p.type_ref,
        &var,
        type_ns,
        runtime_use,
    ));
    (
        format!("\"cookie\": __cookieString({ser}), "),
        format!("\"cookie\": {var}, "),
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
