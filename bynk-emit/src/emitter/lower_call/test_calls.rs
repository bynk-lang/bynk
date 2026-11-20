//! The test-case and system-tier service-address calls of a method call (back-half paydown
//! 1A.26, #3727): a local service call inside a `bynk test` case (`svc.call(args)`, an HTTP, cron,
//! queue, event or websocket address), the `system`-tier HTTP, cron and queue drivers, and the
//! asset fetch. They run only while lowering a case body, never production code. Moved out of
//! `lower.rs` byte for byte (Decision D) and then converted: each builds a node when every part
//! is one and falls back to the same text otherwise.
//!
//! The call-site principal (`by Alice(...)`) reaches these as nodes (`LowerCtx::call_site_identity`
//! and friends, Decision B); a principal that is still text makes the call text.

use bynk_check::checker::{Callee, Ty};
use bynk_model::IrHttpMethod;
use bynk_ts::{TsExpr, TsObjectEntry};

use super::super::lower::{
    Built, LoweredExpr, Pre, node_or_text, settle_helper, settle_with, str_node, text_value,
};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{any_ty, arrow, call, id, member, param, paren, take};
use super::super::{Expr, ExprKind, Ident, TypeRef, escape_ts_string};

// ---- Names ----

/// The name of a `system`-tier driver: `<driver>_<service>_<key>`.
fn sysdrive_name(driver: &str, service: &str, key: &str) -> String {
    format!("{driver}_{service}_{key}")
}

/// The generic `405` driver of a service (#707).
fn wrong_method_driver_name(service: &str) -> String {
    format!("__sysdrive_wrongmethod_{service}")
}

/// The module's event-delivery helper for a service (#2052).
fn event_helper_name(service: &str) -> String {
    format!("__bynkEvent_{service}")
}

/// `{s:?}` as a bridge value: a string-literal node when Rust's escaping and the printer's agree
/// (ASCII route text, identifier actor names), else the Debug text, so a call over a string the
/// two would write differently falls back to text byte for byte (Decision C).
pub(in crate::emitter) fn debug_str(s: &str) -> Built {
    let debug = format!("{s:?}");
    if debug == format!("\"{}\"", escape_ts_string(s)) {
        Ok(str_node(s))
    } else {
        Err(debug)
    }
}

/// `{ ...deps, <entries> }`.
fn deps_with(entries: Vec<TsObjectEntry>) -> TsExpr {
    let mut all = vec![TsObjectEntry::Spread(id("deps"))];
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

fn bridge(built: Built) -> LoweredExpr {
    built.into()
}

// ---- The call-site principal as the handler's `deps` ----

/// The `deps` argument an addressed test-case call passes, and whether it is the bare `deps`.
struct DepsArg {
    value: LoweredExpr,
    plain: bool,
}

/// I3 (#1971), #2020: `{ ...deps, who: { tag, identity? } }`, a sum handler's principal.
fn who_deps(actor: &str, identity: Option<LoweredExpr>) -> LoweredExpr {
    let tag = bridge(debug_str(actor));
    let who = |tag: TsExpr, identity: Option<TsExpr>| {
        let mut fields = vec![TsObjectEntry::prop("tag".to_string(), tag)];
        fields.extend(identity.map(|i| TsObjectEntry::prop("identity".to_string(), i)));
        deps_with(vec![TsObjectEntry::prop(
            "who".to_string(),
            TsExpr::object_entries(fields),
        )])
    };
    bridge(match identity {
        Some(identity) => node_or_text(
            vec![tag, identity],
            |c| {
                let [tag, identity] = take(c);
                who(tag, Some(identity))
            },
            |t| {
                let [tag, identity] = take(t);
                format!("{{ ...deps, who: {{ tag: {tag}, identity: {identity} }} }}")
            },
        ),
        None => node_or_text(
            vec![tag],
            |c| {
                let [tag] = take(c);
                who(tag, None)
            },
            |t| {
                let [tag] = take(t);
                format!("{{ ...deps, who: {{ tag: {tag} }} }}")
            },
        ),
    })
}

/// `{ ...deps, identity: (identity as any), claims? }`; an `Oidc` handler also reads its token's
/// claims from `deps.claims` (#2380): the principal's, or none.
fn identity_deps(identity: LoweredExpr, claims: Option<LoweredExpr>) -> LoweredExpr {
    let mut children = vec![identity];
    children.extend(claims);
    bridge(node_or_text(
        children,
        |c| {
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
        },
        |t| {
            let mut texts = t.into_iter();
            let identity = texts.next().unwrap_or_default();
            let claims = texts
                .next()
                .map(|c| format!(", claims: {c}"))
                .unwrap_or_default();
            format!("{{ ...deps, identity: ({identity} as any){claims} }}")
        },
    ))
}

/// The key the addressed handler's principal is looked up under (a sum handler's `deps.who`).
fn sum_handler_key(method: &Ident, args: &[Expr]) -> Option<String> {
    match (IrHttpMethod::from_ident(&method.name), args.first()) {
        (Some(verb), Some(first)) => match &first.kind {
            ExprKind::StrLit(path) => Some(crate::emitter::http_handler_method_name_ir(verb, path)),
            _ => None,
        },
        _ if method.name == "call" => Some("call".to_string()),
        // #3051: a websocket `open` reads its principal as an `on call` or http handler does.
        _ if method.name == "open" => Some("open".to_string()),
        _ => None,
    }
}

/// The `deps` argument of an addressed call: the call-site principal folded in, or `deps`.
fn test_deps(cx: &LowerCtx, service: &str, sum_key: Option<&str>) -> DepsArg {
    let plain = |value| DepsArg { value, plain: true };
    let deps = || LoweredExpr::Node(id("deps"));
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
            let claims = if sum_key.is_some_and(|k| cx.is_oidc_handler(service, k)) {
                cx.note_token_claims();
                Some(cx.call_site_claims.clone().unwrap_or_else(|| {
                    LoweredExpr::Node(call(id("__tokenClaims"), vec![TsExpr::array(vec![])]))
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
    mut lowered: Vec<LoweredExpr>,
    deps: LoweredExpr,
) -> Built {
    lowered.push(deps);
    node_or_text(
        lowered,
        |nodes| call(member(id(service), key), nodes),
        |texts| format!("{service}.{key}({})", texts.join(", ")),
    )
}

fn lower_args(args: &[Expr], cx: &mut LowerCtx, pre: &mut Pre) -> Vec<LoweredExpr> {
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
                .filter_map(|h| match h {
                    bynk_model::IrHandlerKind::Cron { expr } => Some(expr.clone()),
                    _ => None,
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
/// reads the envelope sender with no DMARC verdict (`DmarcNone`) unless the case names a principal:
/// `((__m: any) => svc.key(__m, { ...deps, identity: { ... } }))(mail)`.
fn inbound_email_call(service: &str, key: &str, mail: LoweredExpr) -> Built {
    node_or_text(
        vec![mail],
        |c| {
            let [mail] = take(c);
            let sender = TsExpr::object(vec![
                ("address".to_string(), member(id("__m"), "from")),
                (
                    "dmarc".to_string(),
                    TsExpr::object(vec![("tag".to_string(), str_node("DmarcNone"))]),
                ),
            ]);
            let handler = call(
                member(id(service), key),
                vec![
                    id("__m"),
                    deps_with(vec![TsObjectEntry::prop("identity".to_string(), sender)]),
                ],
            );
            call(
                paren(arrow(vec![param("__m", Some(any_ty()))], handler)),
                vec![mail],
            )
        },
        |t| {
            let [mail] = take(t);
            format!(
                "((__m: any) => {service}.{key}(__m, {{ ...deps, identity: {{ address: __m.from, dmarc: {{ tag: \"DmarcNone\" }} }} }}))({mail})"
            )
        },
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
) -> Option<Built> {
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callees.get(&e.id),
            Some(bynk_check::checker::Callee::TestAssetFetch { .. })
        )
        && let Some(first) = args.first()
    {
        let asset = pre.lower_node(first, cx);
        return Some(node_or_text(
            vec![asset],
            |c| {
                let [asset] = take(c);
                call(id("__bynkAssetGet"), vec![member(asset, "url")])
            },
            |t| {
                let [asset] = take(t);
                format!("__bynkAssetGet(({asset}).url)")
            },
        ));
    }
    None
}

/// Lower a `system`-tier cron or queue address, driven through the participant Worker, or `None`
/// when the call is not one. #2136: the driver builds the platform-shaped event and settles the
/// execution context itself.
pub(in crate::emitter) fn lower_system_schedule_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    if let ExprKind::Ident(id) = &receiver.kind {
        let driver = match (method.name.as_str(), args.first().map(|a| &a.kind)) {
            ("schedule", Some(ExprKind::StrLit(expr))) => cx.system_schedule_driver(&id.name, expr),
            ("message", _) => cx.system_message_driver(&id.name),
            _ => None,
        };
        if let Some(driver) = driver {
            let skip = usize::from(method.name == "schedule");
            let rest = lower_args(&args[skip..], cx, pre);
            return Some(node_or_text(
                rest,
                |nodes| call(TsExpr::Ident(driver.clone()), nodes),
                |texts| format!("{driver}({})", texts.join(", ")),
            ));
        }
    }
    None
}

// ---- The `system`-tier HTTP call ----

/// #707: a `(method, path)` whose path is a declared route but whose method has no handler is a
/// wrong-method call; drive the generic `405` driver.
fn wrong_method_call(service: &str, verb: IrHttpMethod, path: &str) -> Built {
    let name = wrong_method_driver_name(service);
    node_or_text(
        vec![bridge(debug_str(verb.as_str())), bridge(debug_str(path))],
        |nodes| call(TsExpr::Ident(name.clone()), nodes),
        |texts| format!("{name}({})", texts.join(", ")),
    )
}

/// The driver prefix a call drives. #706: `by Nobody` drives the *no-auth* driver (no
/// `Authorization` header, so the real seam rejects it); it takes precedence over the body form.
/// Slice C: a `Wire(…)` argument drives the *raw* driver (input sent unvalidated, result decodes
/// to an `HttpOutcome`). #821: the two axes are independent. #2135: `by Forged` / `by Stale`
/// drive the signing driver that sends a wrong or a stale signature.
fn system_driver(cx: &LowerCtx, has_wire: bool) -> String {
    let driver = match (cx.call_site_no_credential, has_wire) {
        (true, true) => "__sysdrive_rawnoauth".to_string(),
        (true, false) => "__sysdrive_noauth".to_string(),
        (false, true) => "__sysdrive_raw".to_string(),
        (false, false) => "__sysdrive".to_string(),
    };
    match (cx.call_site_bad_signature, has_wire) {
        (Some(v), true) => format!("__sysdrive_raw{v}"),
        (Some(v), false) => format!("__sysdrive_{v}"),
        (None, _) => driver,
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
}

/// How a typed argument becomes its string slot.
#[derive(Clone, Copy)]
enum Wire {
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
                format!("{}{suffix}", route.2),
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
        }
    }

    /// Argument `i`, as the slot the driver takes. A typed argument mixed into a raw call is
    /// converted to its `string` slot; a path param coerces via `String(...)`.
    fn slot(&self, i: usize, a: &Expr, lowered: LoweredExpr, cx: &LowerCtx) -> LoweredExpr {
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
            cx.note_cookie_string();
            return serialised(lowered, ty, Wire::Cookie, cx);
        }
        // #2854: a typed `query` record becomes the query string the raw driver appends to the URL.
        if let Some((at, ty)) = &self.query
            && *at == i
        {
            cx.note_query_string();
            return serialised(lowered, ty, Wire::Query, cx);
        }
        match &self.body {
            Some((at, ty)) if *at == i && self.form => {
                cx.note_query_string();
                serialised(lowered, ty, Wire::Form, cx)
            }
            Some((at, ty)) if *at == i => serialised(lowered, ty, Wire::Json, cx),
            _ => bridge(node_or_text(
                vec![lowered],
                |c| call(id("String"), c),
                |t| format!("String({})", t.join(", ")),
            )),
        }
    }
}

/// A typed argument serialised through the wire codec (`serialise_expr_via`). A named type's typed
/// literal is unbranded in test-scaffold code (the checker already validated it against the
/// handler's declared type), so the value is cast through `any` for `serialise_<Name>`, which
/// expects the branded shape. P7.2: deferred, not narrowed; a bare `as unknown as {ty}` broke a
/// real `tsc --strict` fixture (385, "Cannot find name").
fn serialised(lowered: LoweredExpr, ty: &TypeRef, wire: Wire, cx: &LowerCtx) -> LoweredExpr {
    let (ns, ru) = (cx.system_http_type_ns(), cx.runtime_use());
    bridge(node_or_text(
        vec![lowered],
        |c| {
            let [value] = take(c);
            let ser =
                crate::emitter::serialisation::serialise_expr_via(ty, &paren_as_any(value), ns, ru);
            match wire {
                Wire::Cookie => call(id("__cookieString"), vec![ser]),
                Wire::Query => call(id("__queryString"), vec![ser]),
                Wire::Form => call(
                    member(call(id("__queryString"), vec![ser]), "slice"),
                    vec![TsExpr::Lit(bynk_ts::TsLit::Num("1".to_string()))],
                ),
                Wire::Json => call(
                    member(member(id("globalThis"), "JSON"), "stringify"),
                    vec![ser],
                ),
            }
        },
        |t| {
            let [value] = take(t);
            // 1A: a still-text value reaches the serialiser as text.
            let ser = bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                ty,
                &text_value(format!("({value} as any)")),
                ns,
                ru,
            ));
            match wire {
                Wire::Cookie => format!("__cookieString({ser})"),
                Wire::Query => format!("__queryString({ser})"),
                Wire::Form => format!("__queryString({ser}).slice(1)"),
                Wire::Json => format!("globalThis.JSON.stringify({ser})"),
            }
        },
    ))
}

/// Lower a `system`-tier http address, driven through a real `worker.fetch`, or `None` when the
/// call is not one. Slice B: `__sysdrive_<svc>_<key>(rest, sub)`, not a direct handler call.
pub(in crate::emitter) fn lower_system_http_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    let ExprKind::Ident(svc) = &receiver.kind else {
        return None;
    };
    if !cx.is_system_http_service(&svc.name) {
        return None;
    }
    let verb = IrHttpMethod::from_ident(&method.name)?;
    let ExprKind::StrLit(path) = &args.first()?.kind else {
        return None;
    };
    let route = (svc.name.clone(), verb.as_str().to_string(), path.clone());
    if !cx.has_system_http_route(&route) {
        return Some(wrong_method_call(&svc.name, verb, path));
    }
    let key = crate::emitter::http_handler_method_name_ir(verb, path);
    // The `sub` slot: the call-site identity, else the empty string.
    let sub = cx
        .call_site_identity
        .clone()
        .unwrap_or_else(|| LoweredExpr::Node(str_node("")));
    let has_wire = args[1..]
        .iter()
        .any(|a| matches!(&a.kind, ExprKind::Wire(_)));
    let driver = system_driver(cx, has_wire);
    let slots = RawSlots::new(cx, &route, driver.starts_with("__sysdrive_raw"));
    let mut all: Vec<LoweredExpr> = args[1..]
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
        format!("{path}{}", crate::emitter::EXTRA_HEADERS_ROUTE_SUFFIX),
    );
    if cx.has_system_http_route(&extra) {
        let sends_extra = matches!(
            cx.commons().callees.get(&e.id),
            Some(Callee::TestService {
                extra_headers: true,
                ..
            })
        );
        let undefined = || LoweredExpr::Node(id("undefined"));
        let slot = if sends_extra {
            all.pop().unwrap_or_else(undefined)
        } else {
            undefined()
        };
        all.push(slot);
    }
    all.push(sub);
    // #2019: a refinement-guarded route's signing drivers take the claims the acting actor
    // presents; the no-auth drivers sign nothing. #2155: an actor-sum route's signing drivers
    // take the actor the call acts as, which picks its credential.
    if !cx.call_site_no_credential && cx.call_site_bad_signature.is_none() {
        all.extend(cx.system_http_claims_arg(&route));
        all.extend(cx.system_http_sum_actor_arg(&route));
    }
    let name = sysdrive_name(&driver, &svc.name, &key);
    Some(node_or_text(
        all,
        |nodes| call(TsExpr::Ident(name.clone()), nodes),
        |texts| format!("{name}({})", texts.join(", ")),
    ))
}

// ---- The test-case service call ----

/// #3051: the websocket lifecycle addresses. The `TestConnection` remembers the route values and
/// `deps` (the principal) from `open`; `message`/`close` rebuild the handler's argument list from
/// them. The checker encoded the shape in the callee's address. Typed from the handler's own
/// signature (`Parameters<typeof …>`), so `tsc` checks the arity, the frame position and the
/// `deps` shape against the emitted handler; only the route values, which the `TestConnection`
/// holds as `unknown`, are cast, as production's dispatch casts `__att.args[i]`.
///
/// 1A: needs typeof and indexed-access TsTypes: `bynk_ts` has neither, so these three IIFEs stay text.
fn websocket_lifecycle_call(
    shape: &str,
    svc: &str,
    args: &[Expr],
    deps: &LoweredExpr,
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> String {
    let mut parts = shape.split(' ');
    let which = parts.next().unwrap_or_default();
    let nums: Vec<usize> = parts.filter_map(|p| p.parse().ok()).collect();
    let tc = args
        .first()
        .map(|a| pre.lower(a, cx))
        .unwrap_or_else(|| "undefined".to_string());
    let sig = |m: &str| format!("Parameters<typeof {svc}.{m}>");
    match which {
        "open" => {
            let route: Vec<String> = args[1..].iter().map(|a| pre.lower(a, cx)).collect();
            let mut parts = vec!["__c".to_string()];
            parts.extend(route.iter().cloned());
            parts.push(deps.clone().into_text());
            format!(
                "(async () => {{ const __c = {tc}; const __a: {} = [{}]; __c.remember([{}], __a[__a.length - 1]); return {svc}.open(...__a); }})()",
                sig("open"),
                parts.join(", "),
                route.join(", ")
            )
        }
        "message" => {
            let (frame_idx, n) = (
                nums.first().copied().unwrap_or(0),
                nums.get(1).copied().unwrap_or(1),
            );
            let frame = args
                .get(1)
                .map(|a| pre.lower(a, cx))
                .unwrap_or_else(|| "undefined".to_string());
            let p = sig("message");
            let mut k = 0usize;
            let mut parts = vec!["__c".to_string()];
            for i in 0..n {
                if i == frame_idx {
                    parts.push(frame.clone());
                } else {
                    parts.push(format!("__o.route[{k}] as {p}[{}]", i + 1));
                    k += 1;
                }
            }
            parts.push(format!("__o.deps as {p}[{}]", n + 1));
            format!(
                "(async () => {{ const __c = {tc}; const __o = __c.opened(); const __a: {p} = [{}]; return {svc}.message(...__a); }})()",
                parts.join(", ")
            )
        }
        _ => {
            let n = nums.first().copied().unwrap_or(0);
            let p = sig("close");
            let mut parts = vec!["__c".to_string()];
            for i in 0..n {
                parts.push(format!("__o.route[{i}] as {p}[{}]", i + 1));
            }
            parts.push(format!("__o.deps as {p}[{}]", n + 1));
            format!(
                "(async () => {{ const __c = {tc}; const __o = __c.opened(); const __a: {p} = [{}]; return {svc}.close(...__a); }})()",
                parts.join(", ")
            )
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
) -> Option<Built> {
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
    let sum_key = sum_handler_key(method, args);
    let DepsArg { value: deps, plain } = test_deps(cx, &svc.name, sum_key.as_deref());
    let settle = |built: Built| Some(settle_with(helper, built));
    // http address: the method is an HTTP verb and the first argument is the route pattern
    // string. The key is a pure function of verb + path; the remaining args are the handler's
    // positional params.
    if let Some(verb) = IrHttpMethod::from_ident(&method.name)
        && let Some(first) = args.first()
        && let ExprKind::StrLit(path) = &first.kind
    {
        let key = crate::emitter::http_handler_method_name_ir(verb, path);
        let rest = lower_args(&args[1..], cx, pre);
        return settle(address_call(&svc.name, &key, rest, deps));
    }
    if let Some(facts) = handler_facts(cx, &svc.name) {
        // cron address: the emitted key is position-indexed among the service's same-kind
        // handlers; cron drops the leading schedule string.
        if method.name == "schedule"
            && let Some(ExprKind::StrLit(expr)) = args.first().map(|a| &a.kind)
        {
            let idx = facts.cron.iter().position(|c| c == expr);
            let key = crate::emitter::cron_handler_method_name(
                &svc.name,
                idx.unwrap_or(facts.cron.len()),
            );
            let rest = lower_args(&args[1..], cx, pre);
            return settle(address_call(&svc.name, &key, rest, deps));
        }
        if let Some(Callee::TestService { address, .. }) = cx.commons().callee(e.id)
            && let Some(shape) = address.strip_prefix("ws ")
        {
            let shape = shape.to_string();
            let text = websocket_lifecycle_call(&shape, &svc.name, args, &deps, cx, pre);
            return settle(Err(text));
        }
        if method.name == "message" && facts.has_message {
            // A `from queue` service binds exactly one queue and declares one `on message`
            // handler, so the position index is 0.
            let key = crate::emitter::queue_handler_method_name(&svc.name, 0);
            let mut lowered = lower_args(args, cx, pre);
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
        if method.name == "event" && facts.has_event {
            cx.runtime_use().note_test_event_service(&svc.name);
            let undefined = || LoweredExpr::Node(id("undefined"));
            let payload = args
                .first()
                .map(|a| pre.lower_node(a, cx))
                .unwrap_or_else(undefined);
            let envelope = args
                .get(1)
                .map(|a| pre.lower_node(a, cx))
                .unwrap_or_else(undefined);
            let helper_name = event_helper_name(&svc.name);
            return settle(node_or_text(
                vec![payload, envelope, deps],
                |nodes| call(TsExpr::Ident(helper_name.clone()), nodes),
                |texts| format!("{helper_name}({})", texts.join(", ")),
            ));
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

    fn node(text: &str) -> LoweredExpr {
        LoweredExpr::Node(id(text))
    }

    fn text(text: &str) -> LoweredExpr {
        LoweredExpr::Text(text.to_string())
    }

    fn printed(built: Built) -> String {
        LoweredExpr::from(built).into_text()
    }

    /// Verbs, route paths and actor names are written the same by Rust's `{:?}` and the printer,
    /// including `\`, `"` and non-ASCII text, so they build a literal node.
    #[test]
    fn debug_str_agrees_with_the_printer_on_route_text() {
        for s in [
            "GET",
            "/orders/:id",
            "/a\\b",
            "/q\"x",
            "/café",
            "/日本語",
            "a'b",
        ] {
            let built = debug_str(s);
            assert!(built.is_ok(), "{s:?} should build a node");
            assert_eq!(printed(built), format!("{s:?}"), "{s:?}");
        }
    }

    /// Where the two disagree (a control character Rust writes as `\u{7f}` and the printer as
    /// `\u{7F}`), the text path is kept, so the call stays byte for byte.
    #[test]
    fn debug_str_keeps_the_text_where_the_escapes_differ() {
        for s in ["/a\u{7f}b", "/a\0b"] {
            let built = debug_str(s);
            assert!(built.is_err(), "{s:?} should stay text");
            assert_eq!(printed(built), format!("{s:?}"));
        }
    }

    /// A sum handler's principal, as a node and as the text it falls back to.
    #[test]
    fn a_sum_principal_prints_the_same_as_a_node_and_as_text() {
        let want = r#"{ ...deps, who: { tag: "User", identity: id } }"#;
        let as_node = who_deps("User", Some(node("id"))).into_text();
        assert_eq!(as_node, want);
        let as_text = who_deps("User", Some(text("id"))).into_text();
        assert_eq!(as_text, want);
        assert_eq!(
            who_deps("Visitor", None).into_text(),
            r#"{ ...deps, who: { tag: "Visitor" } }"#
        );
    }

    /// An identity keeps its `as any`; an `Oidc` handler adds its claims.
    #[test]
    fn an_identity_principal_prints_the_same_as_a_node_and_as_text() {
        let plain = "{ ...deps, identity: (id as any) }";
        assert_eq!(identity_deps(node("id"), None).into_text(), plain);
        assert_eq!(identity_deps(text("id"), None).into_text(), plain);
        let claims = "{ ...deps, identity: (id as any), claims: c }";
        assert_eq!(
            identity_deps(node("id"), Some(node("c"))).into_text(),
            claims
        );
        assert_eq!(
            identity_deps(text("id"), Some(node("c"))).into_text(),
            claims
        );
    }

    /// The inbound-email arrow prints as the text it replaced, as a node and as text.
    #[test]
    fn the_inbound_email_arrow_prints_the_same_as_a_node_and_as_text() {
        let want = r#"((__m: any) => svc.key(__m, { ...deps, identity: { address: __m.from, dmarc: { tag: "DmarcNone" } } }))(mail)"#;
        assert_eq!(
            printed(inbound_email_call("svc", "key", node("mail"))),
            want
        );
        assert_eq!(
            printed(inbound_email_call("svc", "key", text("mail"))),
            want
        );
    }

    /// An addressed call is a node over a member callee when every part is one.
    #[test]
    fn an_address_call_is_a_node_only_when_every_part_is_one() {
        let built = address_call("svc", "key", vec![node("a")], node("deps"));
        assert!(built.is_ok());
        assert_eq!(printed(built), "svc.key(a, deps)");
        let built = address_call("svc", "key", vec![text("a + b")], node("deps"));
        assert!(built.is_err());
        assert_eq!(printed(built), "svc.key(a + b, deps)");
    }

    /// The system driver names are composed in one place.
    #[test]
    fn driver_names_are_composed_by_their_naming_functions() {
        assert_eq!(
            sysdrive_name("__sysdrive_noauth", "api", "http_GET_open"),
            "__sysdrive_noauth_api_http_GET_open"
        );
        assert_eq!(
            wrong_method_driver_name("api"),
            "__sysdrive_wrongmethod_api"
        );
        assert_eq!(event_helper_name("OnPlaced"), "__bynkEvent_OnPlaced");
    }
}
