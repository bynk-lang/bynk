//! The capability and platform calls of a method call (back-half paydown 1A.24, #3725): a
//! capability call (`deps.Cap.op(args)`, in a handler, a provider, a test case or through a
//! `D1.peek` handle), `Events.emit`, the session and typed cookies, and the derived-`D1`
//! `select`/`firstAs`, with their helpers. Moved out of `lower.rs` byte for byte (Decision F).

use bynk_check::checker::{Callee, Ty};

use super::super::lower::{LoweredExpr, Pre};
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::ts_type_ref;
use super::super::{
    CommonsItem, Expr, ExprKind, Ident, TypeBody, TypeRef, escape_ts_string, test_scaffold_ns,
};
use super::codec_statics::{DecodeSource, json_decoder_fn};

/// #926: render a capability call's explicit type argument(s) as a TS
/// generic argument list (`<T>`), or `""` when absent. Unlike
/// `Json.decode[T]` (`lower_json_codec_call`, below), which specialises a
/// runtime codec per call and needs no TS-level generic at all, a capability
/// operation's `T` is a pure type-level parameter on a real generic TS
/// interface method (Decision C) — with no argument of type `T`, TypeScript
/// cannot infer a return-position-only parameter, so the call site must
/// name it explicitly.
pub(in crate::emitter) fn capability_call_type_args_ts(
    type_args: &[TypeRef],
    wire: Option<&bynk_check::checker::codable::CapEncoding>,
) -> String {
    // Constrained generics slice 3: a type parameter that only occurs in encoded
    // (`unknown`) arguments is not in the emitted interface, so a call omits it.
    let kept: Vec<&TypeRef> = match wire {
        Some(w) => type_args
            .iter()
            .zip(&w.keep_type_args)
            .filter_map(|(t, keep)| keep.then_some(t))
            .collect(),
        None => type_args.iter().collect(),
    };
    if kept.is_empty() {
        return String::new();
    }
    format!(
        "<{}>",
        kept.into_iter()
            .map(ts_type_ref)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// Constrained generics slice 3: how the capability call `e` encodes its
/// `Codable` arguments, when it has any.
pub(in crate::emitter) fn capability_wire(
    e: &Expr,
    cx: &LowerCtx,
) -> Option<std::sync::Arc<bynk_check::checker::codable::CapEncoding>> {
    match cx.commons().callee(e.id) {
        Some(Callee::Capability { wire, .. } | Callee::CrossCap { wire, .. }) => wire.clone(),
        _ => None,
    }
}

/// Constrained generics slice 3 (DECISION D): encode each argument whose
/// parameter mentions a `Codable` type parameter, whole, with the codec of its
/// instantiated type. The provider receives plain JSON and no witness.
pub(in crate::emitter) fn encode_capability_args(
    wire: Option<&bynk_check::checker::codable::CapEncoding>,
    args: &mut [String],
    cx: &LowerCtx,
) {
    let Some(wire) = wire else {
        return;
    };
    for (arg, ty) in args.iter_mut().zip(&wire.encode) {
        if let Some(ty) = ty {
            *arg = cx.encode_call(*ty, arg);
        }
    }
}

/// Typed capability reads (#3458): a call to an operation whose `Effect[P]`
/// payload mentions a `Codable` parameter gets the provider's plain-JSON reply
/// decoded as the instantiated `P`, whole, with the codec of `P`. What a reply
/// that does not decode means is the operation's declared policy (a fault, or a
/// miss for an `Option` payload); the runtime helper applies it. Other calls
/// are returned as they are.
pub(in crate::emitter) fn decode_capability_reply(
    call: String,
    wire: Option<&bynk_check::checker::codable::CapEncoding>,
    cap: &str,
    op: &str,
    cx: &LowerCtx,
) -> String {
    use bynk_check::checker::codable::ReplyPolicy;
    let Some(reply) = wire.and_then(|w| w.reply.as_ref()) else {
        return call;
    };
    cx.runtime_use().note_cmp("__bynkCapabilityReply");
    let policy = match reply.policy {
        ReplyPolicy::Fault => "fault",
        ReplyPolicy::Miss => "miss",
    };
    format!(
        "__bynkCapabilityReply({call}, {}, {{ capability: \"{}\", operation: \"{}\", type: \"{}\" }}, \"{policy}\")",
        // 1A: witness printed for a text caller
        bynk_ts::print_expr(&cx.codec(reply.payload)),
        escape_ts_string(cap),
        escape_ts_string(op),
        escape_ts_string(&reply.payload.display(cx.commons().tys())),
    )
}

/// #934 (call-site key scoping): `Idempotency.dedup[T](key)` and
/// `Idempotency.remember[T](key, value, expiresAfter)` and
/// `Idempotency.claim(key, expiresAfter)` all take the
/// developer-supplied dedup key as their first argument (already lowered, in
/// `args_lowered[0]`). Rewrite it in place to a template literal prefixed with
/// the calling handler's own qualified name, so two unrelated handlers that
/// happen to pick the same literal key never collide.
///
/// `is_first_party` must already have confirmed this is genuinely the
/// first-party `bynk.Idempotency` capability, not merely a same-named
/// capability some other adapter or context happens to declare (both call
/// sites resolve that separately, since the two lowering paths — flattened
/// vs. cross-context-qualified — carry the provenance differently).
///
/// `cx.handler_scope` is set at every real capability-call site (an ordinary
/// service handler, an agent handler, a composed provider op body, a
/// websocket lifecycle DO method — see `emit_service`/`emit_agent`/
/// `emit_provider`/`emit_ws_do_method`). Its absence here means some new
/// capability-call site was added without threading it through; that is a
/// compiler bug, not a user error, so this fails loudly rather than silently
/// shipping an unscoped key.
pub(in crate::emitter) fn scope_idempotency_key(
    is_first_party: bool,
    method: &str,
    args_lowered: &mut [String],
    cx: &LowerCtx,
) {
    if !is_first_party || !matches!(method, "dedup" | "remember" | "claim") {
        return;
    }
    let scope = cx
        .handler_scope()
        .unwrap_or_else(|| panic!("Idempotency.{method} lowered with no handler_scope set"));
    args_lowered[0] = format!("`{scope}::${{{}}}`", args_lowered[0]);
}

/// W30 (#2840): lower a first-party `Idempotency.claim(key, expiresAfter)` made
/// in a service handler so the claim is recorded in the handler's claim scope
/// (`__claims`, declared by [`CLAIM_SCOPE_DECLARATION`]). The scope decides how long
/// the claim is first taken for (`lease`: a short lease when the provider will
/// settle it, the full `expiresAfter` when it won't); `note` remembers the key
/// when it was `Claimed`, and the scope confirms it for the full `expiresAfter`
/// or releases it when the handler exits. The call itself still goes through
/// `<deps>.Idempotency.claim`, so a `stub` of it applies.
pub(in crate::emitter) fn tracked_claim_call(
    cap_expr: &str,
    args: &[String],
    cx: &mut LowerCtx,
) -> String {
    cx.claim_scope_used = true;
    // The bare `await` below makes any enclosing thunk or IIFE effectful —
    // see `emitted_await` (a `par` part that is a claim call must be `async`).
    cx.emitted_await = true;
    format!(
        "(await (async (__k: string, __t: number) => __claims.note(__k, await {cap_expr}.claim(__k, __claims.lease(__t)), __t))({}, {}))",
        args[0], args[1]
    )
}

/// #3147: lower `D1.peek(<participant>.<Cap>)` in an integration or `system`
/// case, or `None` when the call is not one. It holds the participant's own
/// capability over its test database, read-only, as `deps.<local>`: the same
/// `WorkersD1` the app's code calls, over the very database object the
/// participant's `env` holds, so a call on the handle (an ordinary capability
/// call on `deps.<local>`) sees what the app wrote.
pub(in crate::emitter) fn lower_d1_peek_call(e: &Expr, cx: &mut LowerCtx) -> Option<String> {
    let Some(Callee::D1Peek {
        participant,
        binding,
        local,
    }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let ns = test_scaffold_ns(&participant);
    Some(format!(
        "(deps.{local} = new bynk_cloudflare__binding.WorkersD1({{ {binding}: deps.env_{ns}.{binding}.readOnly() }}, \"{binding}\"))"
    ))
}

/// Lower a derived-`D1` `select`/`firstAs` call (#2865), or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_d1_select_call(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // #2865: `Cap.select[T](tail, params)` / `Cap.firstAs[T](tail, params)` on
    // a derived `D1`. The checker wrote the statement (`SELECT` + the column
    // list from `T` + the literal tail); this runs it through `all`/`first`
    // and decodes with the `decodeSql` codec, so no runtime op is involved.
    // Keyed on the recorded callee, not the method name. #2957: the error
    // tags are `as const` — `Err<E>` infers `E` from the literal, and a bare
    // `"Sql"` widens to `string`, so a match on `Err(Sql(e))` would not narrow
    // and `e` would reach tsc as `DecodeError | SqlError`.
    if let Some(Callee::D1Select {
        cap,
        row,
        sql,
        first,
        pg,
        mysql,
    }) = cx.commons().callees.get(&e.id).cloned()
        && let Some(params) = args.get(1)
    {
        let tys = cx.ty_render();
        let target = if first {
            row
        } else {
            tys.intern(Ty::List(row))
        };
        // Hyperdrive H3 (#3320): a `Postgres` read decodes Postgres' text
        // forms and fails as a `PgReadError`, whose variants are `PgSql` and
        // `PgDecode` (`Sql` and `Decode` are `SqlReadError`'s).
        // Hyperdrive H5 (#3322): a `MySql` read decodes the same text forms for
        // `Int` (a `BIGINT` arrives as text) and fails as a `MySqlReadError`
        // (`MySqlSql`, `MySqlDecode`).
        let source = if pg || mysql {
            DecodeSource::Pg
        } else {
            DecodeSource::Sql
        };
        let (sql_tag, decode_tag) = if pg {
            ("PgSql", "PgDecode")
        } else if mysql {
            ("MySqlSql", "MySqlDecode")
        } else {
            ("Sql", "Decode")
        };
        // 1A: the D1 `select` caller splices the decoder into a text IIFE until 1A.24.
        let decoder = LoweredExpr::from(
            json_decoder_fn(target, source, cx)
                .expect("a checked D1 select row type has a JSON decoder"),
        )
        .into_text();
        {
            let params = pre.lower(params, cx);
            let deps =
                if cx.in_test_case() || cap.starts_with(bynk_check::checker::peek::LOCAL_PREFIX) {
                    "deps".to_string()
                } else {
                    cx.cap_deps_expr().to_string()
                };
            let sql = escape_ts_string(&sql);
            let (op, decode) = if first {
                (
                    "first",
                    format!(
                        "if (__r.value.tag === \"None\") return Ok(None); \
                         const __d = __decode(__r.value.value); \
                         return __d.tag === \"Ok\" ? Ok(Some(__d.value)) : Err({{ tag: \"{decode_tag}\" as const, error: __d.error }});"
                    ),
                )
            } else {
                (
                    "all",
                    format!(
                        "const __d = __decode(__r.value); \
                         return __d.tag === \"Ok\" ? Ok(__d.value) : Err({{ tag: \"{decode_tag}\" as const, error: __d.error }});"
                    ),
                )
            };
            return Some(format!(
                "(async () => {{ const __decode = {decoder}; \
                 const __r = await {deps}.{cap}.{op}(\"{sql}\", {params}); \
                 if (__r.tag === \"Err\") return Err({{ tag: \"{sql_tag}\" as const, error: __r.error }}); \
                 {decode} }})()"
            ));
        }
    }
    None
}

/// #3386: lower `Actor.issue(identity)` and `Actor.expire()` on a local `SignedCookie` actor, or
/// `None` when the call is not one. `issue` is an effect: it signs the identity into a session
/// cookie with the actor's keys (from `deps.__session`, supplied by the Worker's compose from the
/// environment) at the `Clock`'s time; `expire` is the pure matching deletion.
pub(in crate::emitter) fn lower_signed_cookie_call(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let Some(Callee::SignedCookie { actor, issue }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let seam = cx.commons().commons.items.iter().find_map(|i| match i {
        CommonsItem::Actor(a) if a.name.name == actor => {
            bynk_check::actors::signed_cookie_seam_of(a, None)
        }
        _ => None,
    })?;
    cx.note_session_cookie();
    let name = escape_ts_string(&seam.cookie);
    let same_site = escape_ts_string(&seam.same_site);
    if !issue {
        return Some(format!(
            "__expireSessionCookie(\"{name}\", \"{same_site}\")"
        ));
    }
    let id = pre.lower(args.first()?, cx);
    let deps = if cx.in_test_case() {
        "deps".to_string()
    } else {
        cx.cap_deps_expr().to_string()
    };
    let actor = escape_ts_string(&actor);
    Some(format!(
        "(async () => __issueSessionCookie(__sessionKeysOf({deps}, \"{actor}\"), \"{actor}\", \"{name}\", \"{same_site}\", {id}, {}, await {deps}.Clock.now()))()",
        seam.ttl_secs
    ))
}

/// #3446: lower `Record.field.cookie(value)` and `Record.field.expire()` to the `Cookie` record
/// literal an author would have written by hand: the field's wire name (its `@name`, else its own
/// name, the key the decoder reads) and the value as the decoder reads it. The leaf table is the
/// inverse of `fieldsObject`'s (an `Int` its digits, a `Bool` `true`/`false`, an enum its variant's
/// name, everything else its text), keyed on the same `query_leaf_kind` the decoder's field spec
/// uses, so the two cannot drift. `expire` is the deletion `clearCookie` sends: empty, `Max-Age=0`,
/// epoch `Expires`. Every other attribute is the `Cookie` default, applied by the router.
pub(in crate::emitter) fn lower_cookie_field_call(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    let Some(Callee::CookieField {
        record,
        field,
        expire,
    }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let types = &cx.commons().types;
    let TypeBody::Record(body) = &types.get(&record)?.body else {
        return None;
    };
    let f = body.fields.iter().find(|f| f.name.name == field)?;
    let name = escape_ts_string(&f.wire_name().unwrap_or_else(|| f.name.name.clone()));
    if expire {
        return Some(typed_cookie_literal(&name, "\"\"", "Some(0)", "Some(0)"));
    }
    let leaf = match &f.type_ref {
        TypeRef::Option(inner, _) => inner.as_ref(),
        other => other,
    };
    let kind = crate::emitter::workers_entry::query_leaf_kind(leaf, types, false);
    let mut value = pre.lower(args.first()?, cx);
    if kind == "decimal" {
        // A `Decimal` is its rendered text on the wire; the runtime helper takes it as such.
        cx.note_decimal("__bynkDecimalToString");
        value = format!("__bynkDecimalToString({value})");
    }
    // The helper rides the cookie-helper import group (`SESSION_COOKIE_RUNTIME_IMPORTS`).
    cx.note_session_cookie();
    let text = format!("__cookieValue(\"{kind}\", {value})");
    Some(typed_cookie_literal(
        &name,
        &text,
        "{ tag: \"None\" as const }",
        "{ tag: \"None\" as const }",
    ))
}

/// #3446: the `Cookie` record literal of a typed write, with every attribute the record's
/// defaults give (the router and `serialiseCookie` read them all: they apply no defaults of their
/// own), as a hand-written `Cookie { name, value }` lowers.
fn typed_cookie_literal(name: &str, value: &str, max_age: &str, expires: &str) -> String {
    format!(
        "({{ name: \"{name}\", value: {value}, maxAge: {max_age}, expires: {expires}, \
         domain: {{ tag: \"None\" as const }}, path: \"/\", secure: true, httpOnly: true, \
         sameSite: {{ tag: \"Lax\" as const }}, partitioned: false }})"
    )
}

/// Lower `Events.emit[E](event)`, buffered into the handler's `__events` until commit, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_events_emit_call(
    e: &Expr,
    receiver: &Expr,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // Events track, slice 0 (spine #936): `Events.emit[E](event)` never
    // calls through a provider like an ordinary capability op — release-
    // at-commit (the retired events track's §3.0, `design/archive/retired-tracks.md`) buffers it into the handler's own
    // `__events` local instead, flushed only if the handler completes
    // without throwing. The local is declared by the outer body wrapper
    // whenever `block_uses_emit` is true (the same syntactic check this
    // receiver/method match mirrors), so the two stay in lockstep by
    // construction. The flush calls `deps.__eventsDispatch`, which the
    // publishing context's own compose wires to the Cloudflare fan-out DO
    // (Workers) or an in-process dispatch closure (Bundle) — see
    // `emit_events_fanout_do`/`project.rs`'s `__eventsDispatch` construction.
    // P6.21 (continued, review of #1254): reads `Callee::Capability` (P6.0)
    // instead of `id.name == "Events" && method.name == "emit"` — a real
    // instance of R6.5's name-matched-receiver defect class, the same one
    // every other branch in this module already closed. `unit_table_uses_
    // emit` (`project.rs`) already reads this exact `Callee` for the
    // identical classification, project-wide. `cx.is_first_party_events()`
    // stays: it answers a different question (is *this unit's* `Events`
    // capability actually the first-party runtime implementation, not
    // which provider/target the emitted TS should call at all) than the
    // one `Callee` settles.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Capability { cap, op, .. }) if cap == "Events" && op == "emit"
        )
        && cx.is_first_party_events()
    {
        let event_name = type_args
            .first()
            .and_then(|t| match t {
                TypeRef::Named(ident) => Some(ident.name.clone()),
                _ => None,
            })
            .unwrap_or_else(|| "unknown".to_string());
        let payload = pre.lower(&args[0], cx);
        // Events track, slice 2 (spine #936): the envelope is minted here,
        // once, before fan-out duplicates this single `__events` entry to
        // every subscriber — so one emission always carries one `eventId`/
        // `emittedAt` to all of them (the invariant the identical-eventId
        // fixture proves). `publisherId` is the emitting *context's*
        // qualified name, not the emitting agent's instance identity:
        // `Events.emit` is also legal from a plain, keyless service handler
        // with no agent to report (`negative/503_events_emit_outside_owner`),
        // so a context-scoped identifier is the only one available
        // uniformly at every legal emission site — an amendment to
        // design/bynk-design-notes.md §7's "the publisher is the emitting
        // agent" framing (events-envelope ADR). `emittedAt` uses bare
        // `Date.now()`, not `given Clock`, matching the runtime's existing
        // JWT exp/nbf precedent of bypassing the `Clock` capability for its
        // own internal timestamps — requiring `Clock` alongside `Events`
        // would be new ambient coupling and would break every existing
        // fixture that emits with `given Events` alone. `schemaVersion` is
        // the event's own declared `@schema(N)` (Events slice 3b, #978),
        // or `1` if it declares none — a real, per-type computed value is
        // still a future slice (3c, the cross-build schema registry).
        let schema_version = cx.event_schema_version(&event_name);
        let publisher_id = escape_ts_string(cx.owning_context().unwrap_or_default());
        return Some(format!(
            "(async () => {{ __events.push({{ type: \"{event_name}\", payload: {payload}, envelope: {{ eventId: globalThis.crypto.randomUUID(), publisherId: \"{publisher_id}\", emittedAt: globalThis.Date.now(), schemaVersion: {schema_version} }} }}); }})()"
        ));
    }
    None
}

/// Lower a capability call: in a test `case` body, or on a capability named in `given`, or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_capability_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<String> {
    // #1975: a capability call in a test `case` body (`Greeter.hello("ann")`)
    // reaches the case's own `deps`, built by `__makeTestDeps`: the checker
    // puts the target's capabilities in scope there, so it records the call
    // as a capability operation, but a case body is no handler and has no
    // `given` set for the branch below to read.
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.in_test_case()
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Capability { cap, .. }) if *cap == id.name
        )
    {
        let mut args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = format!(
            "deps.{}.{}({})",
            id.name,
            method.name,
            args_lowered.join(", ")
        );
        return Some(decode_capability_reply(
            call,
            wire.as_deref(),
            &id.name,
            &method.name,
            cx,
        ));
    }
    // #3147: a read through a `D1.peek` handle, whatever the handle is called:
    // the call is recorded on the capability the case holds as `deps.<cap>`.
    if let Some(Callee::Capability { cap, .. }) = cx.commons().callee(e.id)
        && cap.starts_with(bynk_check::checker::peek::LOCAL_PREFIX)
    {
        let cap = cap.clone();
        let mut args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = format!("deps.{cap}.{}({})", method.name, args_lowered.join(", "));
        return Some(decode_capability_reply(
            call,
            wire.as_deref(),
            &cap,
            &method.name,
            cx,
        ));
    }
    // Capability call: receiver is a bare ident naming a declared
    // capability in `given`. Lower to `<deps>.Capability.op(args)`,
    // where `<deps>` is `deps` in a handler body and `this.deps` in a
    // provider body (v0.12 provider composition).
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.has_capability(&id.name)
    {
        let mut args_lowered: Vec<String> = args.iter().map(|a| pre.lower(a, cx)).collect();
        // #934: a flattened `consumes bynk { Idempotency }` local name is
        // first-party only if it's actually `bynk`'s own capability — either
        // declared right here (this unit *is* `bynk`) or flattened in from it
        // (`flattened_caps`). A same-named capability from any other adapter
        // or context (e.g. the #926/#931 generic-op test fixtures' own
        // illustrative `capability Idempotency`) must not be scoped.
        let is_first_party = id.name == "Idempotency"
            && (cx.in_bynk_unit()
                || cx
                    .cross_context()
                    .flattened_caps
                    .get(&id.name)
                    .map(String::as_str)
                    == Some("bynk"));
        scope_idempotency_key(is_first_party, &method.name, &mut args_lowered, cx);
        if is_first_party && method.name == "claim" && cx.tracks_claims() {
            let cap_expr = format!("{}.{}", cx.cap_deps_expr(), id.name);
            return Some(tracked_claim_call(&cap_expr, &args_lowered, cx));
        }
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = format!(
            "{}.{}.{}{}({})",
            cx.cap_deps_expr(),
            id.name,
            method.name,
            capability_call_type_args_ts(type_args, wire.as_deref()),
            args_lowered.join(", ")
        );
        let decoded = decode_capability_reply(call, wire.as_deref(), &id.name, &method.name, cx);
        let target = format!("{}.{}", id.name, method.name);
        return Some(cx.trace_step("capability", &target, e.span.start, decoded));
    }
    None
}
