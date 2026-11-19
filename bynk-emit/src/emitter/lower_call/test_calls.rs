//! The test-case and system-tier service-address calls of a method call (back-half paydown
//! 1A.26, #3727): a local service call inside a `bynk test` case (`svc.call(args)`, an HTTP, cron,
//! queue, event or websocket address), the `system`-tier HTTP, cron and queue drivers, and the
//! asset fetch. They run only while lowering a case body, never production code. Moved out of
//! `lower.rs` byte for byte (Decision D) before they convert to nodes.

use bynk_check::checker::{Callee, Ty};
use bynk_model::IrHttpMethod;

use super::super::lower::{Pre, text_value};
use super::super::lower_ctx::LowerCtx;
use super::super::{Expr, ExprKind, Ident};

/// Lower a test `case` asset fetch (`svc.GET(asset)`), or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_test_asset_fetch_call(
    e: &Expr,
    receiver: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // v0.7 / v0.182: local service call inside a test case body. `svc.call(args)`
    // lowers to `svc.call(args, deps)`. An http address `svc.<VERB>("/path", …)`
    // (#664) lowers to the emitted handler key `svc.http_<VERB>_<path>(<rest>,
    // <deps>)`, where `<deps>` folds in the call-site identity when present.
    // Slice B: a `system`-tier http address drives a real `worker.fetch` through
    // a driver (`__sysdrive_<svc>_<key>(rest, sub)`), not a direct handler call.
    // #1883, slice 3 ([DECISION F]): `svc.GET(asset)` in a case — the
    // checker classified it (`Callee::TestAssetFetch`). The platform serves an asset before the Worker runs, so it
    // reaches no handler in either tier: the module's `__bynkAssetGet` answers
    // from the files the target's Worker ships (`asset_harness`).
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callees.get(&e.id),
            Some(bynk_check::checker::Callee::TestAssetFetch { .. })
        )
        && let Some(first) = args.first()
    {
        let asset = pre.lower(first, cx);
        return Some(format!("__bynkAssetGet(({asset}).url)"));
    }
    None
}

/// Lower a `system`-tier cron or queue address, driven through the participant Worker, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_system_schedule_call(
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // #2136: a `system`-tier cron or queue address drives the participant
    // Worker's real `scheduled` / `queue` entry through a driver, which builds
    // the platform-shaped event and settles the execution context itself.
    if let ExprKind::Ident(id) = &receiver.kind {
        let driver = match (method.name.as_str(), args.first().map(|a| &a.kind)) {
            ("schedule", Some(ExprKind::StrLit(expr))) => cx.system_schedule_driver(&id.name, expr),
            ("message", _) => cx.system_message_driver(&id.name),
            _ => None,
        };
        if let Some(driver) = driver {
            let skip = usize::from(method.name == "schedule");
            let rest: Vec<String> = args[skip..].iter().map(|a| pre.lower(a, cx)).collect();
            return Some(format!("{driver}({})", rest.join(", ")));
        }
    }
    None
}

/// Lower a `system`-tier http address, driven through a real `worker.fetch`, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_system_http_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.is_system_http_service(&id.name)
        && let Some(verb) = IrHttpMethod::from_ident(&method.name)
        && let Some(first) = args.first()
        && let ExprKind::StrLit(path) = &first.kind
    {
        // #707: a `(method, path)` whose path is a declared route but whose
        // method has no handler is a **wrong-method** call — drive the generic
        // `405` driver. The checker has already rejected a path that is not
        // declared for any method, so an absent route here means wrong method.
        let route_declared =
            cx.has_system_http_route(&(id.name.clone(), verb.as_str().to_string(), path.clone()));
        if !route_declared {
            return Some(format!(
                "__sysdrive_wrongmethod_{}({:?}, {:?})",
                id.name,
                verb.as_str(),
                path
            ));
        }
        let key = crate::emitter::http_handler_method_name_ir(verb, path);
        let sub = cx
            .call_site_identity
            .clone()
            .unwrap_or_else(|| "\"\"".to_string());
        // #706: `by Nobody` drives the *no-auth* driver — no `Authorization`
        // header, so the real seam rejects it (`401` → `Rejected(Unauthorized)`);
        // it takes precedence over the body form (the seam rejects before the
        // body is read). Otherwise — Slice C: a `Wire(…)` argument drives the
        // *raw* driver (input sent unvalidated, result decodes to an
        // `HttpOutcome`); a fully typed call keeps the serialising driver.
        // #821: the two axes are independent — a `Wire(…)` argument combined
        // with `by Nobody` drives the raw *and* no-auth driver together, so
        // the missing-credential rejection and the unvalidated body both
        // apply, rather than one silently discarding the other.
        let has_wire = args[1..]
            .iter()
            .any(|a| matches!(&a.kind, ExprKind::Wire(_)));
        let driver = match (cx.call_site_no_credential, has_wire) {
            (true, true) => "__sysdrive_rawnoauth".to_string(),
            (true, false) => "__sysdrive_noauth".to_string(),
            (false, true) => "__sysdrive_raw".to_string(),
            (false, false) => "__sysdrive".to_string(),
        };
        // #2135: `by Forged` / `by Stale` drive the signing driver that sends
        // a wrong or a stale signature (`__sysdrive_forged`, `…_rawstale`).
        let driver = match (cx.call_site_bad_signature, has_wire) {
            (Some(v), true) => format!("__sysdrive_raw{v}"),
            (Some(v), false) => format!("__sysdrive_{v}"),
            (None, _) => driver,
        };
        let is_raw = driver.starts_with("__sysdrive_raw");
        // #708/#821: the raw (and raw+no-auth) driver's every slot is a
        // `string`. `lower_expr_into` already lowers a `Wire(s)` to its raw inner
        // string, but a *typed* arg mixed into the same raw call must be
        // converted to that `string` slot: the body param serialises through
        // the same wire codec the typed driver uses (so a hand-typed body
        // matches a `Wire`d one byte-for-byte); any other (path) param just
        // coerces via `String(...)`.
        let body_info = if is_raw {
            cx.system_http_route_body(&(id.name.clone(), verb.as_str().to_string(), path.clone()))
                .cloned()
        } else {
            None
        };
        let query_info = if is_raw {
            cx.system_http_route_body(&(
                id.name.clone(),
                verb.as_str().to_string(),
                format!("{path}{}", crate::emitter::QUERY_ROUTE_SUFFIX),
            ))
            .cloned()
        } else {
            None
        };
        // #3276: a `@form` route's typed body is written as a form, not JSON.
        let form_route = is_raw
            && cx
                .system_http_route_body(&(
                    id.name.clone(),
                    verb.as_str().to_string(),
                    format!("{path}{}", crate::emitter::FORM_ROUTE_SUFFIX),
                ))
                .is_some();
        // #3277: the `cookies` record's position, when this call's route takes one.
        let cookies_info = if is_raw {
            cx.system_http_route_body(&(
                id.name.clone(),
                verb.as_str().to_string(),
                format!("{path}{}", crate::emitter::COOKIES_ROUTE_SUFFIX),
            ))
            .cloned()
        } else {
            None
        };
        let rest: Vec<String> = args[1..]
            .iter()
            .enumerate()
            .map(|(i, a)| {
                let lowered = pre.lower(a, cx);
                if !is_raw || matches!(&a.kind, ExprKind::Wire(_)) {
                    return lowered;
                }
                // A `headers: RequestHeaders` argument is no wire slot: it
                // reaches the raw driver typed, just as the typed driver takes
                // it, and becomes the request's headers, so it passes through
                // unconverted.
                if matches!(
                    cx.commons().expr_ty(a.id).as_deref(),
                    Some(Ty::RequestHeaders | Ty::RequestQuery | Ty::RequestCookies)
                ) {
                    return lowered;
                }
                // #3277: a typed `cookies` record becomes the `Cookie` header text
                // the raw driver sends.
                if let Some((cookies_idx, ty)) = &cookies_info
                    && *cookies_idx == i
                {
                    cx.note_cookie_string();
                    return format!(
                        "__cookieString({})",
                        bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                            ty,
                            &text_value(format!("({lowered} as any)")),
                            cx.system_http_type_ns(),
                            cx.runtime_use()
                        ))
                    );
                }
                // #2854: a typed `query` record becomes the query string the
                // raw driver appends to the URL.
                if let Some((query_idx, ty)) = &query_info
                    && *query_idx == i
                {
                    cx.note_query_string();
                    return format!(
                        "__queryString({})",
                        bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                            ty,
                            &text_value(format!("({lowered} as any)")),
                            cx.system_http_type_ns(),
                            cx.runtime_use()
                        ))
                    );
                }
                match &body_info {
                    // A named type's typed literal is unbranded in test-scaffold
                    // code (the checker already validated it against the
                    // handler's declared type — Slice A), the same reason
                    // `driver_param_ty` types a named driver param `any` rather
                    // than the branded type. Cast through `any` here too, so
                    // `serialise_<Name>` (which expects the branded shape)
                    // accepts the plain object literal.
                    //
                    // P7.2: deferred, not narrowed. A first attempt used
                    // `as unknown as {ty}` with a bare `ts_type_ref` and broke a
                    // real `tsc --strict` fixture (385, "Cannot find name") — the
                    // same bare-vs-qualified problem found (and fixed via
                    // `emitter/workers.rs`'s `qualified_type_ref`) for several
                    // other sites this slice touched, but that helper isn't
                    // reachable from this module, and this call site's own scope
                    // (`cx.system_http_type_ns()`) suggests the real
                    // qualification here may differ from `workers.rs`'s own
                    // `handlers` namespace besides — needs its own investigation,
                    // not a copy-paste of a fix built for a different file.
                    // #1435 (Arc E slice 1): `serialise_expr_via` now returns a
                    // real `bynk_ts::TsExpr` — this call site is `lower.rs`'s
                    // own permanent opaque exception (ADR
                    // `arc-c-lower-rs-permanent-exclusion`), so it prints at
                    // the boundary (`bynk_ts::print_expr`), the same
                    // treatment cluster 4's own `lower.rs:2552` call site
                    // already established.
                    Some((body_idx, ty)) if *body_idx == i && form_route => {
                        cx.note_query_string();
                        format!(
                            "__queryString({}).slice(1)",
                            bynk_ts::print_expr(
                                &crate::emitter::serialisation::serialise_expr_via(
                                    ty,
                                    &text_value(format!("({lowered} as any)")),
                                    cx.system_http_type_ns(),
                                    cx.runtime_use()
                                )
                            )
                        )
                    }
                    Some((body_idx, ty)) if *body_idx == i => format!(
                        "globalThis.JSON.stringify({})",
                        bynk_ts::print_expr(&crate::emitter::serialisation::serialise_expr_via(
                            ty,
                            &text_value(format!("({lowered} as any)")),
                            cx.system_http_type_ns(),
                            cx.runtime_use()
                        ))
                    ),
                    _ => format!("String({lowered})"),
                }
            })
            .collect();
        let mut all = rest;
        // #2910: a route that declares no `headers:` takes the case's extra
        // request headers (the call's trailing `RequestHeaders`, when the
        // checker accepted one) in a slot ahead of `sub`.
        if cx.has_system_http_route(&(
            id.name.clone(),
            verb.as_str().to_string(),
            format!("{path}{}", crate::emitter::EXTRA_HEADERS_ROUTE_SUFFIX),
        )) {
            let sends_extra = matches!(
                cx.commons().callees.get(&e.id),
                Some(Callee::TestService {
                    extra_headers: true,
                    ..
                })
            );
            let slot = if sends_extra {
                all.pop().unwrap_or_else(|| "undefined".to_string())
            } else {
                "undefined".to_string()
            };
            all.push(slot);
        }
        all.push(sub);
        // #2019: a refinement-guarded route's signing drivers take the claims
        // the acting actor presents; the no-auth drivers sign nothing.
        if !cx.call_site_no_credential
            && cx.call_site_bad_signature.is_none()
            && let Some(claims) = cx.system_http_claims_arg(&(
                id.name.clone(),
                verb.as_str().to_string(),
                path.clone(),
            ))
        {
            all.push(claims);
        }
        // #2155: an actor-sum route's signing drivers take the actor the call
        // acts as, which picks its credential.
        if !cx.call_site_no_credential
            && cx.call_site_bad_signature.is_none()
            && let Some(actor) = cx.system_http_sum_actor_arg(&(
                id.name.clone(),
                verb.as_str().to_string(),
                path.clone(),
            ))
        {
            all.push(actor);
        }
        return Some(format!(
            "{}_{}_{}({})",
            driver,
            id.name,
            key,
            all.join(", ")
        ));
    }
    None
}

/// Lower a local service call inside a test `case` body, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_test_service_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // #2024: a case `let` may shadow a service of the target; the local wins.
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.is_test_service(&id.name)
        && !cx.is_local(&id.name)
    {
        // #1994: a target whose handlers send with `detach` settles each addressed
        // call — awaits the promises the handler handed `deps.__exec.waitUntil`
        // — so the case's expectations see the sends' effects.
        let settles = cx.test_settles_sends();
        // #2683: on virtual time every addressed call is driven (auto-advance)
        // and then settled, whether or not the handler also sends.
        let virtual_time = cx.runtime_use().virtual_time();
        let settle = |call: String| {
            if virtual_time {
                format!("__bynkSettleVirtual(deps, {call})")
            } else if settles {
                format!("__bynkSettle(deps, {call})")
            } else {
                call
            }
        };
        // P7.2: deferred, not narrowed. `identity` is the raw lowered call-site value
        // (e.g. `"bob"`), with no reachable identity type at this `cx` — resolving
        // one needs the addressed actor's own identity type threaded through from
        // `bynk-check`'s actor declarations, not plumbed here today (unlike the
        // `ty: &TypeRef` available a few lines up for the sibling brand cast). A
        // guessed type risks a `tsc --strict` mismatch; left as `any`, named here.
        // I3 (#1971), #2020: a sum handler (`on call` or http) reads the
        // principal as the resolved actor, `deps.who` (the emitted handler's
        // deps key for every sum, whatever its binder is named):
        // `by Caller("shop.front")`, `by User("ada")` or `by Visitor`.
        let sum_key = match (IrHttpMethod::from_ident(&method.name), args.first()) {
            (Some(verb), Some(first)) => match &first.kind {
                ExprKind::StrLit(path) => {
                    Some(crate::emitter::http_handler_method_name_ir(verb, path))
                }
                _ => None,
            },
            _ if method.name == "call" => Some("call".to_string()),
            // #3051: a websocket `open` reads its principal as an `on call` or
            // http handler does.
            _ if method.name == "open" => Some("open".to_string()),
            _ => None,
        };
        let is_sum = sum_key
            .as_deref()
            .is_some_and(|k| cx.is_sum_handler(&id.name, k));
        let deps_expr = match (cx.call_site_actor.clone(), cx.call_site_identity.clone()) {
            (Some(actor), identity) if is_sum => match identity {
                Some(identity) => {
                    format!("{{ ...deps, who: {{ tag: {actor:?}, identity: {identity} }} }}")
                }
                None => format!("{{ ...deps, who: {{ tag: {actor:?} }} }}"),
            },
            (_, Some(_))
                if sum_key
                    .as_deref()
                    .is_some_and(|k| cx.is_binderless_handler(&id.name, k)) =>
            {
                "deps".to_string()
            }
            (_, Some(identity)) => {
                // #2380 (L5): an `Oidc` handler also reads its token's claims
                // from `deps.claims`: the principal's, or none.
                let claims = if sum_key
                    .as_deref()
                    .is_some_and(|k| cx.is_oidc_handler(&id.name, k))
                {
                    cx.note_token_claims();
                    let claims = cx
                        .call_site_claims
                        .clone()
                        .unwrap_or_else(|| "__tokenClaims([])".to_string());
                    format!(", claims: {claims}")
                } else {
                    String::new()
                };
                format!("{{ ...deps, identity: ({identity} as any){claims} }}")
            }
            (_, None) => "deps".to_string(),
        };
        // http address: the method is an HTTP verb and the first argument is the
        // route pattern string. The key is a pure function of verb + path; the
        // remaining args are the handler's positional params.
        if let Some(verb) = IrHttpMethod::from_ident(&method.name)
            && let Some(first) = args.first()
            && let ExprKind::StrLit(path) = &first.kind
        {
            let key = crate::emitter::http_handler_method_name_ir(verb, path);
            let rest: Vec<String> = args[1..].iter().map(|a| pre.lower(a, cx)).collect();
            let mut all = rest;
            all.push(deps_expr);
            return Some(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
        }
        // cron/queue address: the emitted key is position-indexed among the
        // service's same-kind handlers, so recover the index from the handler
        // list. cron drops the leading schedule string; queue passes the
        // message. P6.37 (design/tracks/the-ir.md §6a): reads `IrHandlerKind`
        // (P6.24a's own pure, unconditional mirror) rather than the raw AST
        // `HandlerKind` this map used to store.
        if let Some(handlers) = cx.test_service_handlers(&id.name) {
            if method.name == "schedule"
                && let Some(ExprKind::StrLit(expr)) = args.first().map(|a| &a.kind)
            {
                let mut idx = 0usize;
                for h in handlers {
                    if let bynk_model::IrHandlerKind::Cron { expr: e } = h {
                        if e == expr {
                            break;
                        }
                        idx += 1;
                    }
                }
                let key = crate::emitter::cron_handler_method_name(&id.name, idx);
                let rest: Vec<String> = args[1..].iter().map(|a| pre.lower(a, cx)).collect();
                let mut all = rest;
                all.push(deps_expr);
                return Some(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
            }
            // #3051: the websocket lifecycle addresses. The `TestConnection`
            // remembers the route values and `deps` (the principal) from `open`;
            // `message`/`close` rebuild the handler's argument list from them. The
            // checker encoded the shape in the callee's address.
            if let Some(Callee::TestService { address, .. }) = cx.commons().callee(e.id)
                && let Some(shape) = address.strip_prefix("ws ")
            {
                let mut parts = shape.split(' ');
                let which = parts.next().unwrap_or_default();
                let nums: Vec<usize> = parts.filter_map(|p| p.parse().ok()).collect();
                let tc = args
                    .first()
                    .map(|a| pre.lower(a, cx))
                    .unwrap_or_else(|| "undefined".to_string());
                let svc = &id.name;
                // Typed from the handler's own signature (`Parameters<typeof …>`), so
                // `tsc` checks the arity, the frame position and the `deps` shape
                // against the emitted handler; only the route values, which the
                // `TestConnection` holds as `unknown`, are cast, as production's
                // dispatch casts `__att.args[i]`.
                let sig = |m: &str| format!("Parameters<typeof {svc}.{m}>");
                let call = match which {
                    "open" => {
                        let route: Vec<String> =
                            args[1..].iter().map(|a| pre.lower(a, cx)).collect();
                        let mut parts = vec!["__c".to_string()];
                        parts.extend(route.iter().cloned());
                        parts.push(deps_expr.clone());
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
                };
                return Some(settle(call));
            }
            if method.name == "message"
                && handlers
                    .iter()
                    .any(|h| matches!(h, bynk_model::IrHandlerKind::Message))
            {
                // A `from queue` service binds exactly one queue and declares one
                // `on message` handler, so the position index is 0.
                let key = crate::emitter::queue_handler_method_name(&id.name, 0);
                let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
                // #3328: a case has no Email Routing to mint the `Sender` identity,
                // so a handler that binds it reads the envelope sender with no
                // DMARC verdict (`DmarcNone`) unless the case names a principal.
                if deps_expr == "deps"
                    && args.first().is_some_and(|a| is_inbound_email(a, cx))
                    && let Some(mail) = args_lowered.first()
                {
                    return Some(settle(format!(
                        "((__m: any) => {}.{}(__m, {{ ...deps, identity: {{ address: __m.from, dmarc: {{ tag: \"DmarcNone\" }} }} }}))({mail})",
                        id.name, key
                    )));
                }
                let mut all = args_lowered;
                all.push(deps_expr);
                return Some(settle(format!("{}.{}({})", id.name, key, all.join(", "))));
            }
            // #2052: an event address delivers through the module's
            // `__bynkEvent_<svc>` helper (`tests_emit::event_delivery_helper`),
            // which supplies the default envelope and calls the service's
            // generated `event` method, whose prologue runs the pattern filter
            // and the `via schema(N)` guard as every real delivery does. An
            // omitted envelope is passed as `undefined`.
            if method.name == "event"
                && handlers
                    .iter()
                    .any(|h| matches!(h, bynk_model::IrHandlerKind::Event))
            {
                cx.runtime_use().note_test_event_service(&id.name);
                let payload = args
                    .first()
                    .map(|a| pre.lower(a, cx))
                    .unwrap_or_else(|| "undefined".to_string());
                let envelope = args
                    .get(1)
                    .map(|a| pre.lower(a, cx))
                    .unwrap_or_else(|| "undefined".to_string());
                return Some(settle(format!(
                    "__bynkEvent_{}({payload}, {envelope}, {deps_expr})",
                    id.name
                )));
            }
        }
        // `svc.call(args)` and other (non-http) forms: pass args through with deps.
        let args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let mut all = args_lowered;
        all.push(deps_expr);
        return Some(settle(format!(
            "{}.{}({})",
            id.name,
            method.name,
            all.join(", ")
        )));
    }
    None
}

/// #3328: whether `e` is a first-party `InboundEmail`, the message an email
/// service's `on message` takes (a queue's message is the user's own type).
fn is_inbound_email(e: &Expr, cx: &LowerCtx) -> bool {
    cx.commons().expr_types.get(&e.id).is_some_and(|te| {
        matches!(&*cx.commons().tys().get(te.ty), bynk_check::checker::Ty::Named { name, .. } if name == "InboundEmail")
    })
}
