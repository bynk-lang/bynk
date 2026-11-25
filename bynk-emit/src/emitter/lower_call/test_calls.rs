//! The test-case and system-tier service-address calls of a method call (back-half paydown
//! 1A.26, #3727): a local service call inside a `bynk test` case (`svc.call(args)`, an HTTP, cron,
//! queue, event or websocket address), the `system`-tier HTTP, cron and queue drivers, and the
//! asset fetch. They run only while lowering a case body, never production code. Moved out of
//! `lower.rs` byte for byte (Decision D) and then converted: each builds a node when every part
//! is one and falls back to the same text otherwise.
//!
//! The call-site principal (`by Alice(...)`) reaches these as nodes (`LowerCtx::call_site_identity`
//! and friends, Decision B).

use bynk_check::checker::{Callee, HandlerAddress, Ty, WsAddress};
use bynk_model::IrHttpMethod;
use bynk_ts::{TsExpr, TsObjectEntry};

use super::super::harness_names::{
    SysdriveKind, route_with, sysdrive_name, wrong_method_driver_name,
};
use super::super::internal_fault::undefined_node;
use super::super::lower::{Pre, settle_helper, settle_with};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{any_ty, take};
use super::super::{Expr, ExprKind, Ident, TypeRef};
use crate::ts_build::{arrow, call, ident, member, num_lit, paren, str_lit};
use bynk_ts::TsParam;

/// #2135, #4385: the bad-credential principal a `system` call drives: the driver's name (`"forged"`,
/// `"expired"`, …) and, on an actor sum, the member it names (`by Forged(Session)`).
#[derive(Clone)]
pub(crate) struct BadDriver {
    pub(crate) driver: &'static str,
    pub(crate) arm: Option<String>,
}

// ---- Names ----

/// The module's event-delivery helper for a service (#2052).
fn event_helper_name(service: &str) -> String {
    format!("__bynkEvent_{service}")
}

/// `{ ...deps, <entries> }`.
fn deps_with(entries: Vec<TsObjectEntry>) -> TsExpr {
    let mut all = vec![TsObjectEntry::Spread(ident("deps"))];
    all.extend(entries);
    TsExpr::object_entries(all)
}

/// `(value as any)`: a test-scaffold typing choice (P7.2): the value has no reachable type here.
fn paren_as_any(value: TsExpr) -> TsExpr {
    paren(TsExpr::As {
        expr: Box::new(value),
        ty: any_ty(),
    })
}

// ---- The call-site principal as the handler's `deps` ----

/// The `deps` argument an addressed test-case call passes, and whether it is the bare `deps`.
struct DepsArg {
    value: bynk_ts::TsExpr,
    plain: bool,
}

/// I3 (#1971), #2020: `{ ...deps, who: { tag, identity? } }`, a sum handler's principal.
fn who_deps(actor: &str, identity: Option<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let tag = str_lit(actor);
    let who = |tag: TsExpr, identity: Option<TsExpr>| {
        let mut fields = vec![TsObjectEntry::prop("tag".to_string(), tag)];
        fields.extend(identity.map(|i| TsObjectEntry::prop("identity".to_string(), i)));
        deps_with(vec![TsObjectEntry::prop(
            "who".to_string(),
            TsExpr::object_entries(fields),
        )])
    };
    match identity {
        Some(identity) => {
            let c = vec![tag, identity];
            let [tag, identity] = take(c);
            who(tag, Some(identity))
        }
        None => {
            let c = vec![tag];
            let [tag] = take(c);
            who(tag, None)
        }
    }
}

/// `{ ...deps, identity: (identity as any), claims? }`; an `Oidc` handler also reads its token's
/// claims from `deps.claims` (#2380): the principal's, or none.
fn identity_deps(identity: bynk_ts::TsExpr, claims: Option<bynk_ts::TsExpr>) -> bynk_ts::TsExpr {
    let mut children = vec![identity];
    children.extend(claims);
    {
        let c = children;
        let mut nodes = c.into_iter();
        let mut entries = Vec::new();
        if let Some(identity) = nodes.next() {
            entries.push(TsObjectEntry::prop(
                "identity".to_string(),
                paren_as_any(identity),
            ));
        }
        entries.extend(nodes.map(|claims| TsObjectEntry::prop("claims".to_string(), claims)));
        deps_with(entries)
    }
}

/// The key the addressed handler's principal is looked up under (a sum handler's `deps.who`).
fn sum_handler_key(address: Option<&HandlerAddress>, service: &str) -> Option<String> {
    match address {
        Some(HandlerAddress::Http {
            verb,
            path: Some(path),
        }) => Some(crate::emitter::http_handler_method_name_ir(
            IrHttpMethod::from(*verb),
            path,
        )),
        Some(HandlerAddress::Call) => Some("call".to_string()),
        // #3051: a websocket `open` reads its principal as an `on call` or http handler does.
        Some(HandlerAddress::Ws(WsAddress::Open)) => Some("open".to_string()),
        // #3899: a signed queue consumer's sum, under its handler's method key.
        Some(HandlerAddress::Message { .. }) => {
            Some(crate::emitter::queue_handler_method_name(service, 0))
        }
        _ => None,
    }
}

/// The address the checker resolved for the test-service call `e`, if it recorded one.
fn test_address(cx: &LowerCtx, e: &Expr) -> Option<HandlerAddress> {
    match cx.commons().callee(e.id) {
        Some(Callee::TestService { address, .. }) => Some(address.clone()),
        _ => None,
    }
}

/// The `deps` argument of an addressed call: the call-site principal folded in, or `deps`.
fn test_deps(cx: &LowerCtx, service: &str, sum_key: Option<&str>) -> DepsArg {
    let plain = |value| DepsArg { value, plain: true };
    let deps = || ident("deps");
    let is_sum = sum_key.is_some_and(|k| cx.is_sum_handler(service, k));
    match (cx.call_site_actor.clone(), cx.call_site_identity.clone()) {
        (Some(actor), identity) if is_sum => DepsArg {
            value: who_deps(&actor, identity),
            plain: false,
        },
        (_, Some(_)) if sum_key.is_some_and(|k| cx.is_binderless_handler(service, k)) => {
            plain(deps())
        }
        (_, Some(identity)) => {
            let claims =
                if sum_key.is_some_and(|k| cx.is_claims_handler(service, k)) {
                    Some(cx.call_site_claims.clone().unwrap_or_else(|| {
                        call(ident("__tokenClaims"), vec![TsExpr::array(vec![])])
                    }))
                } else {
                    None
                };
            DepsArg {
                value: identity_deps(identity, claims),
                plain: false,
            }
        }
        (_, None) => plain(deps()),
    }
}

// ---- The address forms ----

/// `svc.key(rest..., deps)`: an http, cron, queue or plain call.
fn address_call(
    service: &str,
    key: &str,
    mut lowered: Vec<bynk_ts::TsExpr>,
    deps: bynk_ts::TsExpr,
) -> bynk_ts::TsExpr {
    lowered.push(deps);
    {
        let nodes = lowered;
        call(member(ident(service), key), nodes)
    }
}

fn lower_args(args: &[Expr], cx: &mut LowerCtx, pre: &mut Pre) -> Vec<bynk_ts::TsExpr> {
    args.iter().map(|a| pre.lower_node(a, cx)).collect()
}

/// A service's handlers, read once so the borrow of `cx` ends before arguments lower.
struct HandlerFacts {
    cron: Vec<String>,
    has_message: bool,
    has_event: bool,
}

fn handler_facts(cx: &LowerCtx, service: &str) -> Option<HandlerFacts> {
    cx.test_service_handlers(service)
        .map(|handlers| HandlerFacts {
            cron: handlers
                .iter()
                .filter_map(|h| {
                    // One shape is read: a cron handler's expression.
                    if let bynk_model::IrHandlerKind::Cron { expr } = h {
                        Some(expr.clone())
                    } else {
                        None
                    }
                })
                .collect(),
            has_message: handlers
                .iter()
                .any(|h| matches!(h, bynk_model::IrHandlerKind::Message)),
            has_event: handlers
                .iter()
                .any(|h| matches!(h, bynk_model::IrHandlerKind::Event)),
        })
}

/// #3328: a case has no Email Routing to mint the `Sender` identity, so a handler that binds it
/// reads the envelope sender with the verdict the case's own `authentication-results` header gives
/// (#3888, by the Worker's rule), `DmarcNone` when there is none:
/// `((__m: any) => svc.key(__m, { ...deps, identity: { ... } }))(mail)`.
fn inbound_email_call(service: &str, key: &str, mail: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let c = vec![mail];
    let [mail] = take(c);
    let sender = TsExpr::object(vec![
        ("address".to_string(), member(ident("__m"), "from")),
        (
            "dmarc".to_string(),
            call(
                ident("__dmarcVerdictOf"),
                vec![member(ident("__m"), "headers")],
            ),
        ),
    ]);
    let handler = call(
        member(ident(service), key),
        vec![
            ident("__m"),
            deps_with(vec![TsObjectEntry::prop("identity".to_string(), sender)]),
        ],
    );
    call(
        paren(arrow(vec![TsParam::new("__m", Some(any_ty()))], handler)),
        vec![mail],
    )
}

/// `svc.GET(asset)` in a case (#1883, slice 3, [DECISION F]): the checker classified it
/// (`Callee::TestAssetFetch`). The platform serves an asset before the Worker runs, so it reaches
/// no handler in either tier: the module's `__bynkAssetGet` answers from the files the target's
/// Worker ships (`asset_harness`). `None` when the call is not one.
pub(in crate::emitter) fn lower_test_asset_fetch_call(
    e: &Expr,
    receiver: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callees.get(&e.id),
            Some(bynk_check::checker::Callee::TestAssetFetch { .. })
        )
        && let Some(first) = args.first()
    {
        let asset = pre.lower_node(first, cx);
        return Some({
            let c = vec![asset];
            let [asset] = take(c);
            call(ident("__bynkAssetGet"), vec![member(asset, "url")])
        });
    }
    None
}

/// Lower a `system`-tier cron or queue address, driven through the participant Worker, or `None`
/// when the call is not one. #2136: the driver builds the platform-shaped event and settles the
/// execution context itself.
pub(in crate::emitter) fn lower_system_schedule_call(
    e: &Expr,
    receiver: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    if let ExprKind::Ident(id) = &receiver.kind {
        let address = test_address(cx, e);
        let driver = match &address {
            Some(HandlerAddress::Schedule {
                expr: Some(expr), ..
            }) => cx.system_schedule_driver(&id.name, expr),
            Some(HandlerAddress::Message { .. }) => cx.system_message_driver(&id.name),
            _ => None,
        };
        if let Some(driver) = driver {
            let skip = usize::from(matches!(address, Some(HandlerAddress::Schedule { .. })));
            let mut rest = lower_args(&args[skip..], cx, pre);
            // #3899: a signed consumer's driver takes the foreign actor the case acts as, after
            // the (optional) delivery metadata.
            if matches!(address, Some(HandlerAddress::Message { .. }))
                && cx.system_queue_signed(&id.name)
                && let Some(actor) = cx.call_site_actor.clone()
            {
                if rest.len() < 2 {
                    rest.push(ident("undefined"));
                }
                rest.push(str_lit(actor));
                // A `Producer` refinement is acted as a Bynk context: `by Orders("shop.orders")`.
                if let Some(identity) = cx.call_site_identity.clone() {
                    rest.push(identity);
                } else {
                    rest.push(ident("undefined"));
                }
            }
            return Some({
                let nodes = rest;
                call(TsExpr::Ident(driver.clone()), nodes)
            });
        }
    }
    None
}

// ---- The `system`-tier HTTP call ----

/// #707: a `(method, path)` whose path is a declared route but whose method has no handler is a
/// wrong-method call; drive the generic `405` driver.
fn wrong_method_call(service: &str, verb: IrHttpMethod, path: &str) -> bynk_ts::TsExpr {
    let name = wrong_method_driver_name(service);
    {
        let nodes = vec![str_lit(verb.as_str()), str_lit(path)];
        call(TsExpr::Ident(name.clone()), nodes)
    }
}

/// The driver prefix a call drives. #706: `by Nobody` drives the *no-auth* driver (no
/// `Authorization` header, so the real seam rejects it); it takes precedence over the body form.
/// Slice C: a `Wire(…)` argument drives the *raw* driver (input sent unvalidated, result decodes
/// to an `HttpOutcome`). #821: the two axes are independent. #2135: `by Forged` / `by Stale`
/// drive the signing driver that sends a wrong or a stale signature.
fn system_driver(cx: &LowerCtx, has_wire: bool) -> SysdriveKind<'static> {
    let kind = match (&cx.call_site_bad_signature, cx.call_site_no_credential) {
        // #4385: a bad credential naming an actor sum's member drives the sum's bad-credential driver.
        (Some(BadDriver { arm: Some(_), .. }), _) => "sumbad",
        (Some(bad), _) => bad.driver,
        (None, true) => "noauth",
        (None, false) => "",
    };
    SysdriveKind {
        raw: has_wire,
        kind,
    }
}

/// How the raw (and raw+no-auth) driver's every `string` slot is written (#708, #821): the typed
/// body, query record and cookies convert through the wire codec the typed driver uses.
struct RawSlots {
    is_raw: bool,
    body: Option<(usize, TypeRef)>,
    query: Option<(usize, TypeRef)>,
    /// #3276: a `@form` route's typed body is written as a form, not JSON.
    form: bool,
    /// #3277: the `cookies` record's position, when the route takes one.
    cookies: Option<(usize, TypeRef)>,
    /// #3911: the typed `headers` record's position, when the route takes one. Unlike the
    /// others it is converted in every call, typed or raw: the driver sends its wire form.
    headers: Option<(usize, TypeRef)>,
}

/// How a typed argument becomes its string slot.
#[derive(Clone, Copy)]
enum Wire {
    /// #3911: the wire form itself, which a driver writes as headers.
    Value,
    Cookie,
    Query,
    Form,
    Json,
}

impl RawSlots {
    fn new(cx: &LowerCtx, route: &(String, String, String), is_raw: bool) -> RawSlots {
        let at = |suffix: &str| {
            if !is_raw {
                return None;
            }
            cx.system_http_route_body(&(
                route.0.clone(),
                route.1.clone(),
                route_with(&route.2, suffix),
            ))
            .cloned()
        };
        RawSlots {
            is_raw,
            body: if is_raw {
                cx.system_http_route_body(route).cloned()
            } else {
                None
            },
            query: at(crate::emitter::QUERY_ROUTE_SUFFIX),
            form: at(crate::emitter::FORM_ROUTE_SUFFIX).is_some(),
            cookies: at(crate::emitter::COOKIES_ROUTE_SUFFIX),
            headers: cx
                .system_http_route_body(&(
                    route.0.clone(),
                    route.1.clone(),
                    route_with(&route.2, crate::emitter::HEADERS_ROUTE_SUFFIX),
                ))
                .cloned(),
        }
    }

    /// Argument `i`, as the slot the driver takes. A typed argument mixed into a raw call is
    /// converted to its `string` slot; a path param coerces via `String(...)`.
    fn slot(&self, i: usize, a: &Expr, lowered: bynk_ts::TsExpr, cx: &LowerCtx) -> bynk_ts::TsExpr {
        // #3911: a typed `headers` record reaches either driver as its wire form (a `Wire`
        // argument is header text already).
        if let Some((at, ty)) = &self.headers
            && *at == i
            && !matches!(&a.kind, ExprKind::Wire(_))
        {
            return serialised(lowered, ty, Wire::Value, cx);
        }
        if !self.is_raw || matches!(&a.kind, ExprKind::Wire(_)) {
            return lowered;
        }
        // A `headers: RequestHeaders` argument is no wire slot: it reaches the raw driver typed,
        // just as the typed driver takes it, and becomes the request's headers.
        if matches!(
            cx.commons().expr_ty(a.id).as_deref(),
            Some(Ty::RequestHeaders | Ty::RequestQuery | Ty::RequestCookies)
        ) {
            return lowered;
        }
        // #3277: a typed `cookies` record becomes the `Cookie` header text the raw driver sends.
        if let Some((at, ty)) = &self.cookies
            && *at == i
        {
            return serialised(lowered, ty, Wire::Cookie, cx);
        }
        // #2854: a typed `query` record becomes the query string the raw driver appends to the URL.
        if let Some((at, ty)) = &self.query
            && *at == i
        {
            return serialised(lowered, ty, Wire::Query, cx);
        }
        match &self.body {
            Some((at, ty)) if *at == i && self.form => serialised(lowered, ty, Wire::Form, cx),
            Some((at, ty)) if *at == i => serialised(lowered, ty, Wire::Json, cx),
            _ => call(ident("String"), vec![lowered]),
        }
    }
}

/// A typed argument serialised through the wire codec (`serialise_expr_via`). A named type's typed
/// literal is unbranded in test-scaffold code (the checker already validated it against the
/// handler's declared type), so the value is cast through `any` for `serialise_<Name>`, which
/// expects the branded shape. P7.2: deferred, not narrowed; a bare `as unknown as {ty}` broke a
/// real `tsc --strict` fixture (385, "Cannot find name").
fn serialised(
    lowered: bynk_ts::TsExpr,
    ty: &TypeRef,
    wire: Wire,
    cx: &LowerCtx,
) -> bynk_ts::TsExpr {
    let (ns, ru) = (cx.system_http_type_ns(), cx.runtime_use());
    let value = lowered;
    let ser = crate::emitter::serialisation::serialise_expr_via(ty, &paren_as_any(value), ns, ru);
    match wire {
        Wire::Value => ser,
        Wire::Cookie => call(ident("__cookieString"), vec![ser]),
        Wire::Query => call(ident("__queryString"), vec![ser]),
        Wire::Form => call(
            member(call(ident("__queryString"), vec![ser]), "slice"),
            vec![TsExpr::Lit(bynk_ts::TsLit::Num("1".to_string()))],
        ),
        Wire::Json => call(
            member(member(ident("globalThis"), "JSON"), "stringify"),
            vec![ser],
        ),
    }
}

/// Lower a `system`-tier http address, driven through a real `worker.fetch`, or `None` when the
/// call is not one. Slice B: `__sysdrive_<svc>_<key>(rest, sub)`, not a direct handler call.
pub(in crate::emitter) fn lower_system_http_call(
    e: &Expr,
    receiver: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    let ExprKind::Ident(svc) = &receiver.kind else {
        return None;
    };
    if !cx.is_system_http_service(&svc.name) {
        return None;
    }
    let Some(HandlerAddress::Http {
        verb,
        path: Some(path),
    }) = test_address(cx, e)
    else {
        return None;
    };
    let verb = IrHttpMethod::from(verb);
    let path = &path;
    let route = (svc.name.clone(), verb.as_str().to_string(), path.clone());
    if !cx.has_system_http_route(&route) {
        return Some(wrong_method_call(&svc.name, verb, path));
    }
    let key = crate::emitter::http_handler_method_name_ir(verb, path);
    // The `sub` slot: the call-site identity, else the empty string.
    let sub = cx.call_site_identity.clone().unwrap_or_else(|| str_lit(""));
    let has_wire = args[1..]
        .iter()
        .any(|a| matches!(&a.kind, ExprKind::Wire(_)));
    let driver = system_driver(cx, has_wire);
    let slots = RawSlots::new(cx, &route, driver.raw);
    let mut all: Vec<bynk_ts::TsExpr> = args[1..]
        .iter()
        .enumerate()
        .map(|(i, a)| {
            let lowered = pre.lower_node(a, cx);
            slots.slot(i, a, lowered, cx)
        })
        .collect();
    // #2910: a route that declares no `headers:` takes the case's extra request headers (the
    // call's trailing `RequestHeaders`, when the checker accepted one) in a slot ahead of `sub`.
    let extra = (
        route.0.clone(),
        route.1.clone(),
        route_with(path, crate::emitter::EXTRA_HEADERS_ROUTE_SUFFIX),
    );
    if cx.has_system_http_route(&extra) {
        let sends_extra = matches!(
            cx.commons().callees.get(&e.id),
            Some(Callee::TestService {
                extra_headers: true,
                ..
            })
        );
        // The slot is the call's trailing argument when the checker accepted one,
        // else the legitimate default: no extra headers, `undefined`.
        let extra_arg = if sends_extra {
            Some(all.pop().unwrap_or_else(|| {
                cx.missing_fact(
                    e.span,
                    "trailing `RequestHeaders` argument of a system call",
                )
            }))
        } else {
            None
        };
        let slot = extra_arg.unwrap_or_else(undefined_node);
        all.push(slot);
    }
    // #4385: the sum's bad-credential driver takes the credential kind before `sub`, the member after.
    let bad_arm = cx
        .call_site_bad_signature
        .as_ref()
        .and_then(|b| b.arm.as_ref().map(|a| (b.driver, a)));
    all.extend(bad_arm.map(|(driver, _)| str_lit(driver)));
    all.push(sub);
    all.extend(bad_arm.map(|(_, arm)| str_lit(arm)));
    // #2019: a refinement-guarded route's signing drivers take the claims the acting actor
    // presents; the no-auth drivers sign nothing. #2155: an actor-sum route's signing drivers
    // take the actor the call acts as, which picks its credential.
    if !cx.call_site_no_credential && cx.call_site_bad_signature.is_none() {
        all.extend(cx.system_http_claims_arg(&route));
        all.extend(cx.system_http_sum_actor_arg(&route));
    }
    let name = sysdrive_name(driver, &svc.name, &key);
    Some({
        let nodes = all;
        call(TsExpr::Ident(name.clone()), nodes)
    })
}

// ---- The test-case service call ----

/// #3051: the websocket lifecycle addresses. The `TestConnection` remembers the route values and
/// `deps` (the principal) from `open`; `message`/`close` rebuild the handler's argument list from
/// them. The checker encoded the shape in the callee's address. Typed from the handler's own
/// signature (`Parameters<typeof …>`), so `tsc` checks the arity, the frame position and the
/// `deps` shape against the emitted handler; only the route values, which the `TestConnection`
/// holds as `unknown`, are cast, as production's dispatch casts `__att.args[i]`.
///
/// bynk_ts::TsExpr as nodes when the `TestConnection`, the route values and the `deps` are all nodes
/// (`Parameters<typeof …>` is a `typeof` type under an indexed-access type, a number-literal
/// index for the frame positions), else the same text.
fn websocket_lifecycle_call(
    shape: &WsAddress,
    svc: &str,
    args: &[Expr],
    at: bynk_syntax::span::Span,
    deps: &bynk_ts::TsExpr,
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> bynk_ts::TsExpr {
    use bynk_ts::{TsBinaryOp, TsBindingName, TsStmt, TsType};
    // An argument the checker's arity check guarantees: absent, a missing fact.
    let tc = match args.first() {
        Some(a) => pre.lower_node(a, cx),
        None => cx.missing_fact(
            at,
            "`TestConnection` argument of a websocket lifecycle call",
        ),
    };
    let sig = |m: &str| {
        TsType::named_with_args("Parameters", vec![TsType::type_of(member(ident(svc), m))])
    };
    let const_ = |name: &str, ty: Option<TsType>, init: TsExpr| {
        TsStmt::const_stmt(TsBindingName::Ident(name.to_string()), ty, init, None)
    };
    // `(async () => { <stmts> })()`.
    let iife = |stmts: Vec<TsStmt>| {
        call(
            paren(TsExpr::Arrow {
                params: Vec::new(),
                is_async: true,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Block(stmts)),
            }),
            Vec::new(),
        )
    };
    let spread_call = |m: &str| {
        TsStmt::return_stmt(
            Some(call(
                member(ident(svc), m),
                vec![TsExpr::Spread(Box::new(ident("__a")))],
            )),
            None,
        )
    };
    // `__o.<field>[k] as <sig>[idx]`.
    let opened_cast = |field: &str, k: usize, m: &str, idx: usize| TsExpr::As {
        expr: Box::new(TsExpr::Index {
            object: Box::new(member(ident("__o"), field)),
            index: Box::new(num_lit(k)),
        }),
        ty: TsType::indexed(sig(m), TsType::number_literal(idx as u32)),
    };
    let deps_cast = |m: &str, idx: usize| TsExpr::As {
        expr: Box::new(member(ident("__o"), "deps")),
        ty: TsType::indexed(sig(m), TsType::number_literal(idx as u32)),
    };
    match shape {
        WsAddress::Open => {
            let route: Vec<bynk_ts::TsExpr> =
                args[1..].iter().map(|a| pre.lower_node(a, cx)).collect();
            let mut children = vec![tc];
            children.extend(route);
            children.push(deps.clone());
            {
                let mut c = children;
                let tc = c.remove(0);
                let deps = c.pop().expect("the deps child is last");
                let mut parts = vec![ident("__c")];
                parts.extend(c.iter().cloned());
                parts.push(deps);
                let last = TsExpr::Index {
                    object: Box::new(ident("__a")),
                    index: Box::new(TsExpr::Binary {
                        op: TsBinaryOp::Sub,
                        left: Box::new(member(ident("__a"), "length")),
                        right: Box::new(num_lit(1)),
                    }),
                };
                iife(vec![
                    const_("__c", None, tc),
                    const_("__a", Some(sig("open")), TsExpr::array(parts)),
                    TsStmt::expr_stmt(
                        call(
                            member(ident("__c"), "remember"),
                            vec![TsExpr::array(c), last],
                        ),
                        None,
                    ),
                    spread_call("open"),
                ])
            }
        }
        WsAddress::Message { frame } => {
            let (frame_idx, n) = frame.map_or((0, 1), |f| (f.frame_index, f.params));
            let frame = match args.get(1) {
                Some(a) => pre.lower_node(a, cx),
                None => cx.missing_fact(at, "frame argument of a websocket `message` call"),
            };
            {
                let c = vec![tc, frame];
                let [tc, frame] = take(c);
                let mut parts = vec![ident("__c")];
                let mut k = 0usize;
                for i in 0..n {
                    if i == frame_idx {
                        parts.push(frame.clone());
                    } else {
                        parts.push(opened_cast("route", k, "message", i + 1));
                        k += 1;
                    }
                }
                parts.push(deps_cast("message", n + 1));
                iife(vec![
                    const_("__c", None, tc),
                    const_(
                        "__o",
                        None,
                        call(member(ident("__c"), "opened"), Vec::new()),
                    ),
                    const_("__a", Some(sig("message")), TsExpr::array(parts)),
                    spread_call("message"),
                ])
            }
        }
        WsAddress::Close { params } => {
            let n = params.unwrap_or(0);
            {
                let c = vec![tc];
                let [tc] = take(c);
                let mut parts = vec![ident("__c")];
                for i in 0..n {
                    parts.push(opened_cast("route", i, "close", i + 1));
                }
                parts.push(deps_cast("close", n + 1));
                iife(vec![
                    const_("__c", None, tc),
                    const_(
                        "__o",
                        None,
                        call(member(ident("__c"), "opened"), Vec::new()),
                    ),
                    const_("__a", Some(sig("close")), TsExpr::array(parts)),
                    spread_call("close"),
                ])
            }
        }
    }
}

/// Lower a local service call inside a test `case` body, or `None` when the call is not one.
/// `svc.call(args)` lowers to `svc.call(args, deps)`; an http address `svc.<VERB>("/path", …)`
/// (#664) to the emitted handler key `svc.http_<VERB>_<path>(<rest>, <deps>)`, where `<deps>`
/// folds in the call-site principal when present.
pub(in crate::emitter) fn lower_test_service_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    let ExprKind::Ident(svc) = &receiver.kind else {
        return None;
    };
    // #2024: a case `let` may shadow a service of the target; the local wins.
    if !cx.is_test_service(&svc.name) || cx.is_local(&svc.name) {
        return None;
    }
    // #1994, #2683: a target whose handlers send with `detach`, or that runs on virtual time,
    // settles each addressed call, so the case's expectations see the sends' effects. Read
    // before anything lowers.
    let helper = settle_helper(cx);
    let address = test_address(cx, e);
    let sum_key = sum_handler_key(address.as_ref(), &svc.name);
    let DepsArg { value: deps, plain } = test_deps(cx, &svc.name, sum_key.as_deref());
    let settle = |built: bynk_ts::TsExpr| Some(settle_with(helper, built));
    // http address: the method is an HTTP verb and the first argument is the route pattern
    // string. The key is a pure function of verb + path; the remaining args are the handler's
    // positional params.
    if let Some(HandlerAddress::Http {
        verb,
        path: Some(path),
    }) = &address
    {
        let key = crate::emitter::http_handler_method_name_ir(IrHttpMethod::from(*verb), path);
        let rest = lower_args(&args[1..], cx, pre);
        return settle(address_call(&svc.name, &key, rest, deps));
    }
    if let Some(facts) = handler_facts(cx, &svc.name) {
        // cron address: the emitted key is position-indexed among the service's same-kind
        // handlers; cron drops the leading schedule string.
        if let Some(HandlerAddress::Schedule {
            expr: Some(expr),
            param,
        }) = &address
        {
            let idx = facts.cron.iter().position(|c| c == expr);
            let key = crate::emitter::cron_handler_method_name(
                &svc.name,
                idx.unwrap_or(facts.cron.len()),
            );
            let mut rest = lower_args(&args[1..], cx, pre);
            // #3874: a `CronSchedule` parameter holds the address's own schedule.
            if let Some(at) = param {
                // The literal is a valid `CronSchedule` the checker matched to the
                // handler; the brand is a type, so it is cast to it.
                let schedule = crate::ts_build::as_expr(
                    str_lit(expr.clone()),
                    bynk_ts::TsType::named("never"),
                );
                rest.insert((*at).min(rest.len()), schedule);
            }
            return settle(address_call(&svc.name, &key, rest, deps));
        }
        if let Some(HandlerAddress::Ws(shape)) = &address {
            let built = websocket_lifecycle_call(shape, &svc.name, args, e.span, &deps, cx, pre);
            return settle(built);
        }
        if matches!(address, Some(HandlerAddress::Message { .. })) && facts.has_message {
            // A `from queue` service binds exactly one queue and declares one `on message`
            // handler, so the position index is 0.
            let key = crate::emitter::queue_handler_method_name(&svc.name, 0);
            let mut lowered = lower_args(args, cx, pre);
            // #3880: a handler that declares a `QueueDelivery` is given the address's
            // second argument, or the default delivery (the first attempt, sent at the
            // epoch, which is where each case's harness clock starts), in its position.
            if let Some(HandlerAddress::Message { delivery: Some(at) }) = &address
                && !lowered.is_empty()
            {
                let given = (lowered.len() > 1).then(|| lowered.remove(1));
                let delivery = given.unwrap_or_else(|| {
                    TsExpr::object(vec![
                        ("attempts".to_string(), num_lit("1")),
                        ("sentAt".to_string(), num_lit("0")),
                    ])
                });
                lowered.insert((*at).min(lowered.len()), delivery);
            }
            if plain && args.first().is_some_and(|a| is_inbound_email(a, cx)) && !lowered.is_empty()
            {
                return settle(inbound_email_call(&svc.name, &key, lowered.swap_remove(0)));
            }
            return settle(address_call(&svc.name, &key, lowered, deps));
        }
        // #2052: an event address delivers through the module's `__bynkEvent_<svc>` helper
        // (`tests_emit::event_delivery_helper`), which supplies the default envelope and calls
        // the service's generated `event` method, whose prologue runs the pattern filter and the
        // `via schema(N)` guard as every real delivery does. An omitted envelope is `undefined`.
        if matches!(address, Some(HandlerAddress::Event)) && facts.has_event {
            cx.runtime_use().note_test_event_service(&svc.name);
            let payload = match args.first() {
                Some(a) => pre.lower_node(a, cx),
                None => cx.missing_fact(e.span, "event argument of a `svc.event` call"),
            };
            // The legitimate default: an omitted envelope is `undefined`.
            let envelope = args
                .get(1)
                .map(|a| pre.lower_node(a, cx))
                .unwrap_or_else(undefined_node);
            let helper_name = event_helper_name(&svc.name);
            return settle({
                let nodes = vec![payload, envelope, deps];
                call(TsExpr::Ident(helper_name.clone()), nodes)
            });
        }
    }
    // `svc.call(args)` and other (non-http) forms: pass args through with deps.
    let lowered = lower_args(args, cx, pre);
    settle(address_call(&svc.name, &method.name, lowered, deps))
}

/// #3328: whether `e` is a first-party `InboundEmail`, the message an email
/// service's `on message` takes (a queue's message is the user's own type).
fn is_inbound_email(e: &Expr, cx: &LowerCtx) -> bool {
    cx.commons().expr_types.get(&e.id).is_some_and(|te| {
        matches!(&*cx.commons().tys().get(te.ty), bynk_check::checker::Ty::Named { name, .. } if name == "InboundEmail")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(text: &str) -> bynk_ts::TsExpr {
        ident(text)
    }

    fn printed(built: bynk_ts::TsExpr) -> String {
        bynk_ts::print_expr(&built)
    }

    /// Verbs, route paths and actor names are written the same by Rust's `{:?}` and the printer,
    /// including `\`, `"` and non-ASCII text.
    #[test]
    fn route_text_prints_as_rust_debug_does() {
        for s in [
            "GET",
            "/orders/:id",
            "/a\\b",
            "/q\"x",
            "/café",
            "/日本語",
            "a'b",
        ] {
            assert_eq!(printed(str_lit(s)), format!("{s:?}"), "{s:?}");
        }
    }

    /// A sum handler's principal.
    #[test]
    fn a_sum_principal_prints() {
        let want = r#"{ ...deps, who: { tag: "User", identity: id } }"#;
        assert_eq!(printed(who_deps("User", Some(node("id")))), want);
        assert_eq!(
            printed(who_deps("Visitor", None)),
            r#"{ ...deps, who: { tag: "Visitor" } }"#
        );
    }

    /// An identity keeps its `as any`; an `Oidc` handler adds its claims.
    #[test]
    fn an_identity_principal_prints() {
        let plain = "{ ...deps, identity: (id as any) }";
        assert_eq!(printed(identity_deps(node("id"), None)), plain);
        let claims = "{ ...deps, identity: (id as any), claims: c }";
        assert_eq!(printed(identity_deps(node("id"), Some(node("c")))), claims);
    }

    /// The inbound-email arrow prints as the text it replaced.
    #[test]
    fn the_inbound_email_arrow_prints_as_the_text_it_replaced() {
        let want = r#"((__m: any) => svc.key(__m, { ...deps, identity: { address: __m.from, dmarc: __dmarcVerdictOf(__m.headers) } }))(mail)"#;
        assert_eq!(
            printed(inbound_email_call("svc", "key", node("mail"))),
            want
        );
    }

    /// An addressed call is a node over a member callee.
    #[test]
    fn an_address_call_is_a_node_over_a_member_callee() {
        let built = address_call("svc", "key", vec![node("a")], node("deps"));
        assert_eq!(printed(built), "svc.key(a, deps)");
    }

    /// The system driver names are composed in one place.
    #[test]
    fn driver_names_are_composed_by_their_naming_functions() {
        assert_eq!(
            sysdrive_name(SysdriveKind::typed("noauth"), "api", "http_GET_open"),
            "__sysdrive_noauth_api_http_GET_open"
        );
        assert_eq!(
            wrong_method_driver_name("api"),
            "__sysdrive_wrongmethod_api"
        );
        assert_eq!(event_helper_name("OnPlaced"), "__bynkEvent_OnPlaced");
    }
}
