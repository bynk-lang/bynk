//! The capability and platform calls of a method call (back-half paydown 1A.24, #3725): a
//! capability call (`deps.Cap.op(args)`, in a handler, a provider, a test case or through a
//! `D1.peek` handle), `Events.emit`, the session and typed cookies, and the derived-`D1`
//! `select`/`firstAs`, with their helpers. Moved out of `lower.rs` byte for byte (Decision F).

use bynk_check::checker::codable::CapEncoding;
use bynk_check::checker::{Callee, Ty};
use bynk_ts::{TsExpr, TsLit, TsStmt, TsType};

use super::super::emit::flatten_emit_ident_chain;
use super::super::lower::{Built, LoweredExpr, Pre, ident_node, node_or_text};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{
    binary, block_arrow, call, const_, expr_stmt, id, member, param, paren, ret,
};
use super::super::ts_types::{ts_type_ref, ts_type_ref_to_ts_type};
use super::super::{
    CommonsItem, Expr, ExprKind, Ident, TypeBody, TypeRef, escape_ts_string, test_scaffold_ns,
};
use super::codec_statics::{DecodeSource, json_decoder_fn};

/// `"text"`: raw text, which the printer escapes (the one escaper, `escape_ts_literal`, that
/// `escape_ts_string` re-exports, so the node and text forms cannot disagree).
fn lit_str(text: &str) -> TsExpr {
    TsExpr::Lit(TsLit::Str(text.to_string()))
}

/// A numeric literal printed verbatim.
fn lit_num(digits: impl ToString) -> TsExpr {
    TsExpr::Lit(TsLit::Num(digits.to_string()))
}

/// `"text" as const`: a string-literal tag that keeps its literal type.
fn tag_const(text: &str) -> TsExpr {
    TsExpr::As {
        expr: Box::new(lit_str(text)),
        ty: TsType::named("const"),
    }
}

/// An inline object literal of `key: value` entries.
fn object(entries: Vec<(&str, TsExpr)>) -> TsExpr {
    TsExpr::object(
        entries
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect(),
    )
}

/// `a.b.c` as a member chain over `root`.
fn chain(root: TsExpr, path: &[&str]) -> TsExpr {
    path.iter().fold(root, |object, name| member(object, name))
}

/// Lower every argument, keeping each as the bridge value it lowered to.
fn lower_args(args: &[Expr], cx: &mut LowerCtx, pre: &mut Pre) -> Vec<LoweredExpr> {
    args.iter().map(|a| pre.lower_node(a, cx)).collect()
}

/// `deps` in a test case or on a `D1.peek` handle, else the handler's own deps receiver, as a node
/// and as the text the fallbacks print.
fn deps_receiver(in_test: bool, cx: &LowerCtx) -> (TsExpr, String) {
    if in_test {
        (id("deps"), "deps".to_string())
    } else {
        (cx.cap_deps_node(), cx.cap_deps_expr().to_string())
    }
}

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
    wire: Option<&CapEncoding>,
) -> String {
    let kept = kept_type_args(type_args, wire);
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

/// Constrained generics slice 3: a type parameter that only occurs in encoded (`unknown`)
/// arguments is not in the emitted interface, so a call omits it. The one filter the text form
/// ([`capability_call_type_args_ts`]) and the node form ([`capability_call_type_args_node`]) share.
fn kept_type_args<'a>(type_args: &'a [TypeRef], wire: Option<&CapEncoding>) -> Vec<&'a TypeRef> {
    match wire {
        Some(w) => type_args
            .iter()
            .zip(&w.keep_type_args)
            .filter_map(|(t, keep)| keep.then_some(t))
            .collect(),
        None => type_args.iter().collect(),
    }
}

/// [`capability_call_type_args_ts`] as type nodes: empty when there is nothing to name.
fn capability_call_type_args_node(
    type_args: &[TypeRef],
    wire: Option<&CapEncoding>,
) -> Vec<TsType> {
    kept_type_args(type_args, wire)
        .into_iter()
        .map(|t| ts_type_ref_to_ts_type(t, None))
        .collect()
}

/// Constrained generics slice 3: how the capability call `e` encodes its
/// `Codable` arguments, when it has any.
pub(in crate::emitter) fn capability_wire(
    e: &Expr,
    cx: &LowerCtx,
) -> Option<std::sync::Arc<CapEncoding>> {
    match cx.commons().callee(e.id) {
        Some(Callee::Capability { wire, .. } | Callee::CrossCap { wire, .. }) => wire.clone(),
        _ => None,
    }
}

/// Constrained generics slice 3 (DECISION D): encode each argument whose
/// parameter mentions a `Codable` type parameter, whole, with the codec of its
/// instantiated type. The provider receives plain JSON and no witness.
pub(in crate::emitter) fn encode_capability_args(
    wire: Option<&CapEncoding>,
    args: &mut [LoweredExpr],
    cx: &LowerCtx,
) {
    let Some(wire) = wire else {
        return;
    };
    for (arg, ty) in args.iter_mut().zip(&wire.encode) {
        let Some(ty) = ty else {
            continue;
        };
        let value = std::mem::replace(arg, LoweredExpr::Text(String::new()));
        *arg = match value {
            LoweredExpr::Node(n) => LoweredExpr::Node(cx.encode_call(*ty, n)),
            // A text argument is encoded as text: the same node, over the text as an operand.
            LoweredExpr::Text(t) => {
                LoweredExpr::Text(bynk_ts::print_expr(&cx.encode_call(*ty, ident_node(&t))))
            }
        };
    }
}

/// Typed capability reads (#3458): a call to an operation whose `Effect[P]`
/// payload mentions a `Codable` parameter gets the provider's plain-JSON reply
/// decoded as the instantiated `P`, whole, with the codec of `P`. What a reply
/// that does not decode means is the operation's declared policy (a fault, or a
/// miss for an `Option` payload); the runtime helper applies it. Other calls
/// are returned as they are.
pub(in crate::emitter) fn decode_capability_reply(
    call: Built,
    wire: Option<&CapEncoding>,
    cap: &str,
    op: &str,
    cx: &LowerCtx,
) -> Built {
    use bynk_check::checker::codable::ReplyPolicy;
    let Some(reply) = wire.and_then(|w| w.reply.as_ref()) else {
        return call;
    };
    cx.runtime_use().note_cmp("__bynkCapabilityReply");
    let policy = match reply.policy {
        ReplyPolicy::Fault => "fault",
        ReplyPolicy::Miss => "miss",
    };
    let payload = reply.payload.display(cx.commons().tys());
    let codec = cx.codec(reply.payload);
    match call {
        Ok(node) => Ok(crate::emitter::lower_list::call(
            id("__bynkCapabilityReply"),
            vec![
                node,
                codec,
                object(vec![
                    ("capability", lit_str(cap)),
                    ("operation", lit_str(op)),
                    ("type", lit_str(&payload)),
                ]),
                lit_str(policy),
            ],
        )),
        Err(text) => Err(format!(
            "__bynkCapabilityReply({text}, {}, {{ capability: \"{}\", operation: \"{}\", type: \"{}\" }}, \"{policy}\")",
            bynk_ts::print_expr(&codec),
            escape_ts_string(cap),
            escape_ts_string(op),
            escape_ts_string(&payload),
        )),
    }
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
    args_lowered: &mut [LoweredExpr],
    cx: &LowerCtx,
) {
    if !is_first_party || !matches!(method, "dedup" | "remember" | "claim") {
        return;
    }
    let scope = cx
        .handler_scope()
        .unwrap_or_else(|| panic!("Idempotency.{method} lowered with no handler_scope set"));
    let key = std::mem::replace(&mut args_lowered[0], LoweredExpr::Text(String::new()));
    args_lowered[0] = match key {
        // `TemplateLit` parts print verbatim, so a scope that a template literal would read
        // differently (a backtick, `${`, a backslash) keeps the text form, which prints it the
        // same way and is no worse off.
        LoweredExpr::Node(n) if template_safe(scope) => LoweredExpr::Node(TsExpr::template_lit(
            vec![format!("{scope}::"), String::new()],
            vec![n],
        )),
        other => LoweredExpr::Text(format!("`{scope}::${{{}}}`", other.into_text())),
    };
}

/// Whether `scope` prints the same inside a template literal's text as it reads.
fn template_safe(scope: &str) -> bool {
    !scope.contains('`') && !scope.contains("${") && !scope.contains('\\')
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
    cap_node: TsExpr,
    cap_expr: &str,
    args: Vec<LoweredExpr>,
    cx: &mut LowerCtx,
) -> Built {
    cx.claim_scope_used = true;
    // The bare `await` below makes any enclosing thunk or IIFE effectful —
    // see `emitted_await` (a `par` part that is a claim call must be `async`).
    cx.emitted_await = true;
    node_or_text(
        args,
        |nodes| {
            let k = || id("__k");
            let t = || id("__t");
            let claim = TsExpr::Await(Box::new(call(
                member(cap_node, "claim"),
                vec![k(), call(member(id("__claims"), "lease"), vec![t()])],
            )));
            let note = call(member(id("__claims"), "note"), vec![k(), claim, t()]);
            let note_arrow = TsExpr::Arrow {
                params: vec![
                    param("__k", Some(TsType::named("string"))),
                    param("__t", Some(TsType::named("number"))),
                ],
                is_async: true,
                generics: vec![],
                return_type: None,
                body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(note))),
            };
            TsExpr::Await(Box::new(paren(call(paren(note_arrow), nodes))))
        },
        |texts| {
            format!(
                "(await (async (__k: string, __t: number) => __claims.note(__k, await {cap_expr}.claim(__k, __claims.lease(__t)), __t))({}, {}))",
                texts[0], texts[1]
            )
        },
    )
}

/// #3147: lower `D1.peek(<participant>.<Cap>)` in an integration or `system`
/// case, or `None` when the call is not one. It holds the participant's own
/// capability over its test database, read-only, as `deps.<local>`: the same
/// `WorkersD1` the app's code calls, over the very database object the
/// participant's `env` holds, so a call on the handle (an ordinary capability
/// call on `deps.<local>`) sees what the app wrote.
pub(in crate::emitter) fn lower_d1_peek_call(e: &Expr, cx: &mut LowerCtx) -> Option<Built> {
    let Some(Callee::D1Peek {
        participant,
        binding,
        local,
    }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let ns = test_scaffold_ns(&participant);
    let read_only = call(
        chain(id("deps"), &[&format!("env_{ns}"), &binding, "readOnly"]),
        vec![],
    );
    let workers_d1 = TsExpr::New {
        callee: Box::new(chain(id("bynk_cloudflare__binding"), &["WorkersD1"])),
        args: vec![object(vec![(&binding, read_only)]), lit_str(&binding)],
    };
    Some(Ok(paren(TsExpr::Assign {
        target: Box::new(member(id("deps"), &local)),
        value: Box::new(workers_d1),
    })))
}

/// Lower a derived-`D1` `select`/`firstAs` call (#2865), or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_d1_select_call(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    // #2865: `Cap.select[T](tail, params)` / `Cap.firstAs[T](tail, params)` on
    // a derived `D1`. The checker wrote the statement (`SELECT` + the column
    // list from `T` + the literal tail); this runs it through `all`/`first`
    // and decodes with the `decodeSql` codec, so no runtime op is involved.
    // Keyed on the recorded callee, not the method name. #2957: the error
    // tags are `as const` — `Err<E>` infers `E` from the literal, and a bare
    // `"Sql"` widens to `string`, so a match on `Err(Sql(e))` would not narrow
    // and `e` would reach tsc as `DecodeError | SqlError`.
    let Some(Callee::D1Select {
        cap,
        row,
        sql,
        first,
        pg,
        mysql,
    }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let params = args.get(1)?;
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
    // The decoder is a node, except a `Form.decode`'s (`workers_entry` source text).
    let decoder = LoweredExpr::from(
        json_decoder_fn(target, source, cx)
            .expect("a checked D1 select row type has a JSON decoder"),
    );
    let params = pre.lower_node(params, cx);
    let in_test = cx.in_test_case() || cap.starts_with(bynk_check::checker::peek::LOCAL_PREFIX);
    let (deps_node, deps_text) = deps_receiver(in_test, cx);
    let op = if first { "first" } else { "all" };
    let escaped_sql = escape_ts_string(&sql);
    Some(node_or_text(
        vec![decoder, params],
        |nodes| {
            let [decoder, params] = super::super::lower_list::take(nodes);
            let r = || id("__r");
            let d = || id("__d");
            let tag_is = |obj: TsExpr, tag: &str| {
                binary(
                    bynk_ts::TsBinaryOp::StrictEq,
                    member(obj, "tag"),
                    lit_str(tag),
                )
            };
            let err = |tag: &str, error: TsExpr| {
                call(
                    id("Err"),
                    vec![object(vec![("tag", tag_const(tag)), ("error", error)])],
                )
            };
            let mut body = vec![
                const_("__decode", None, decoder),
                const_(
                    "__r",
                    None,
                    TsExpr::Await(Box::new(call(
                        chain(deps_node, &[&cap, op]),
                        vec![lit_str(&sql), params],
                    ))),
                ),
                TsStmt::if_stmt(
                    tag_is(r(), "Err"),
                    ret(err(sql_tag, member(r(), "error"))),
                    None,
                ),
            ];
            let decode_arg = if first {
                body.push(TsStmt::if_stmt(
                    tag_is(member(r(), "value"), "None"),
                    ret(call(id("Ok"), vec![id("None")])),
                    None,
                ));
                chain(r(), &["value", "value"])
            } else {
                member(r(), "value")
            };
            body.push(const_("__d", None, call(id("__decode"), vec![decode_arg])));
            let ok_value = if first {
                call(id("Some"), vec![member(d(), "value")])
            } else {
                member(d(), "value")
            };
            body.push(ret(TsExpr::Conditional {
                test: Box::new(tag_is(d(), "Ok")),
                consequent: Box::new(call(id("Ok"), vec![ok_value])),
                alternate: Box::new(err(decode_tag, member(d(), "error"))),
            }));
            call(paren(block_arrow(vec![], true, body)), vec![])
        },
        |texts| {
            let [decoder, params] = super::super::lower_list::take(texts);
            let decode = if first {
                format!(
                    "if (__r.value.tag === \"None\") return Ok(None); \
                     const __d = __decode(__r.value.value); \
                     return __d.tag === \"Ok\" ? Ok(Some(__d.value)) : Err({{ tag: \"{decode_tag}\" as const, error: __d.error }});"
                )
            } else {
                format!(
                    "const __d = __decode(__r.value); \
                     return __d.tag === \"Ok\" ? Ok(__d.value) : Err({{ tag: \"{decode_tag}\" as const, error: __d.error }});"
                )
            };
            format!(
                "(async () => {{ const __decode = {decoder}; \
                 const __r = await {deps_text}.{cap}.{op}(\"{escaped_sql}\", {params}); \
                 if (__r.tag === \"Err\") return Err({{ tag: \"{sql_tag}\" as const, error: __r.error }}); \
                 {decode} }})()"
            )
        },
    ))
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
) -> Option<Built> {
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
    if !issue {
        return Some(Ok(call(
            id("__expireSessionCookie"),
            vec![lit_str(&seam.cookie), lit_str(&seam.same_site)],
        )));
    }
    let key = pre.lower_node(args.first()?, cx);
    let (deps_node, deps_text) = deps_receiver(cx.in_test_case(), cx);
    let name = escape_ts_string(&seam.cookie);
    let same_site = escape_ts_string(&seam.same_site);
    let actor_text = escape_ts_string(&actor);
    Some(node_or_text(
        vec![key],
        |nodes| {
            let [key] = super::super::lower_list::take(nodes);
            let now = TsExpr::Await(Box::new(call(
                chain(deps_node.clone(), &["Clock", "now"]),
                vec![],
            )));
            let issue = call(
                id("__issueSessionCookie"),
                vec![
                    call(id("__sessionKeysOf"), vec![deps_node, lit_str(&actor)]),
                    lit_str(&actor),
                    lit_str(&seam.cookie),
                    lit_str(&seam.same_site),
                    key,
                    lit_num(seam.ttl_secs),
                    now,
                ],
            );
            call(
                paren(crate::emitter::lower_list::arrow_with(vec![], true, issue)),
                vec![],
            )
        },
        |texts| {
            let [key] = super::super::lower_list::take(texts);
            format!(
                "(async () => __issueSessionCookie(__sessionKeysOf({deps_text}, \"{actor_text}\"), \"{actor_text}\", \"{name}\", \"{same_site}\", {key}, {}, await {deps_text}.Clock.now()))()",
                seam.ttl_secs
            )
        },
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
) -> Option<Built> {
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
    let name = f.wire_name().unwrap_or_else(|| f.name.name.clone());
    if expire {
        return Some(Ok(typed_cookie_node(
            &name,
            lit_str(""),
            call(id("Some"), vec![lit_num(0)]),
            call(id("Some"), vec![lit_num(0)]),
        )));
    }
    let leaf = match &f.type_ref {
        TypeRef::Option(inner, _) => inner.as_ref(),
        other => other,
    };
    let kind = crate::emitter::workers_entry::query_leaf_kind(leaf, types, false);
    let value = pre.lower_node(args.first()?, cx);
    let decimal = kind == "decimal";
    if decimal {
        // A `Decimal` is its rendered text on the wire; the runtime helper takes it as such.
        cx.note_decimal("__bynkDecimalToString");
    }
    // The helper rides the cookie-helper import group (`SESSION_COOKIE_RUNTIME_IMPORTS`).
    cx.note_session_cookie();
    let none = || object(vec![("tag", tag_const("None"))]);
    Some(node_or_text(
        vec![value],
        |nodes| {
            let [value] = super::super::lower_list::take(nodes);
            let value = if decimal {
                call(id("__bynkDecimalToString"), vec![value])
            } else {
                value
            };
            typed_cookie_node(
                &name,
                call(id("__cookieValue"), vec![lit_str(kind), value]),
                none(),
                none(),
            )
        },
        |texts| {
            let [mut value] = super::super::lower_list::take(texts);
            if decimal {
                value = format!("__bynkDecimalToString({value})");
            }
            let text = format!("__cookieValue(\"{kind}\", {value})");
            typed_cookie_literal(
                &escape_ts_string(&name),
                &text,
                "{ tag: \"None\" as const }",
                "{ tag: \"None\" as const }",
            )
        },
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

/// [`typed_cookie_literal`] as a node: the field's wire `name` is raw text (the printer escapes it).
fn typed_cookie_node(name: &str, value: TsExpr, max_age: TsExpr, expires: TsExpr) -> TsExpr {
    paren(object(vec![
        ("name", lit_str(name)),
        ("value", value),
        ("maxAge", max_age),
        ("expires", expires),
        ("domain", object(vec![("tag", tag_const("None"))])),
        ("path", lit_str("/")),
        ("secure", TsExpr::Lit(TsLit::Bool(true))),
        ("httpOnly", TsExpr::Lit(TsLit::Bool(true))),
        ("sameSite", object(vec![("tag", tag_const("Lax"))])),
        ("partitioned", TsExpr::Lit(TsLit::Bool(false))),
    ]))
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
) -> Option<Built> {
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
        let payload = pre.lower_node(&args[0], cx);
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
        let owner = cx.owning_context().unwrap_or_default().to_string();
        let publisher_id = escape_ts_string(&owner);
        // The envelope is a node over raw text; the text form escapes it as it always did.
        return Some(node_or_text(
            vec![payload],
            |nodes| {
                let [payload] = super::super::lower_list::take(nodes);
                let global = |name: &str| member(id("globalThis"), name);
                let envelope = object(vec![
                    (
                        "eventId",
                        call(member(global("crypto"), "randomUUID"), vec![]),
                    ),
                    ("publisherId", lit_str(&owner)),
                    ("emittedAt", call(member(global("Date"), "now"), vec![])),
                    ("schemaVersion", lit_num(schema_version)),
                ]);
                let entry = object(vec![
                    ("type", lit_str(&event_name)),
                    ("payload", payload),
                    ("envelope", envelope),
                ]);
                let push = expr_stmt(call(member(id("__events"), "push"), vec![entry]));
                call(paren(block_arrow(vec![], true, vec![push])), vec![])
            },
            |texts| {
                let [payload] = super::super::lower_list::take(texts);
                format!(
                    "(async () => {{ __events.push({{ type: \"{event_name}\", payload: {payload}, envelope: {{ eventId: globalThis.crypto.randomUUID(), publisherId: \"{publisher_id}\", emittedAt: globalThis.Date.now(), schemaVersion: {schema_version} }} }}); }})()"
                )
            },
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
) -> Option<Built> {
    // #1975: a capability call in a test `case` body (`Greeter.hello("ann")`)
    // reaches the case's own `deps`, built by `__makeTestDeps`: the checker
    // puts the target's capabilities in scope there, so it records the call
    // as a capability operation, but a case body is no handler and has no
    // `given` set for the branch below to read.
    if let ExprKind::Ident(recv) = &receiver.kind
        && cx.in_test_case()
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Capability { cap, .. }) if *cap == recv.name
        )
    {
        let mut args_lowered = lower_args(args, cx, pre);
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = capability_op_call(
            id("deps"),
            "deps".to_string(),
            &recv.name,
            &method.name,
            Vec::new(),
            String::new(),
            args_lowered,
        );
        return Some(decode_capability_reply(
            call,
            wire.as_deref(),
            &recv.name,
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
        let mut args_lowered = lower_args(args, cx, pre);
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = capability_op_call(
            id("deps"),
            "deps".to_string(),
            &cap,
            &method.name,
            Vec::new(),
            String::new(),
            args_lowered,
        );
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
    if let ExprKind::Ident(recv) = &receiver.kind
        && cx.has_capability(&recv.name)
    {
        let mut args_lowered = lower_args(args, cx, pre);
        // #934: a flattened `consumes bynk { Idempotency }` local name is
        // first-party only if it's actually `bynk`'s own capability — either
        // declared right here (this unit *is* `bynk`) or flattened in from it
        // (`flattened_caps`). A same-named capability from any other adapter
        // or context (e.g. the #926/#931 generic-op test fixtures' own
        // illustrative `capability Idempotency`) must not be scoped.
        let is_first_party = recv.name == "Idempotency"
            && (cx.in_bynk_unit()
                || cx
                    .cross_context()
                    .flattened_caps
                    .get(&recv.name)
                    .map(String::as_str)
                    == Some("bynk"));
        scope_idempotency_key(is_first_party, &method.name, &mut args_lowered, cx);
        if is_first_party && method.name == "claim" && cx.tracks_claims() {
            let cap_expr = format!("{}.{}", cx.cap_deps_expr(), recv.name);
            let cap_node = member(cx.cap_deps_node(), &recv.name);
            return Some(tracked_claim_call(cap_node, &cap_expr, args_lowered, cx));
        }
        let wire = capability_wire(e, cx);
        encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
        let call = capability_op_call(
            cx.cap_deps_node(),
            cx.cap_deps_expr().to_string(),
            &recv.name,
            &method.name,
            capability_call_type_args_node(type_args, wire.as_deref()),
            capability_call_type_args_ts(type_args, wire.as_deref()),
            args_lowered,
        );
        let decoded = decode_capability_reply(call, wire.as_deref(), &recv.name, &method.name, cx);
        let target = format!("{}.{}", recv.name, method.name);
        return Some(match decoded {
            Ok(node) => Ok(cx.trace_step_node("capability", &target, e.span.start, node)),
            Err(text) => Err(cx.trace_step("capability", &target, e.span.start, text)),
        });
    }
    None
}

/// Lower a cross-context capability call (`B.Cap.op(…)`), or `None` when the call is not one: the
/// first half of `lower_cross_context_call`, whose service-call half is 1A.25's. The provider is
/// instantiated locally in the composition root, so this lowers to an in-process
/// `<deps>.<Cap>.op(args)` exactly like a local capability call — the consumed-context prefix is
/// resolved away.
pub(in crate::emitter) fn lower_cross_capability_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    let chain = flatten_emit_ident_chain(receiver)?;
    let (consumed, cap) = cx.cross_context().resolve_cross_capability(&chain)?;
    let mut args_lowered = lower_args(args, cx, pre);
    let first_party = cap == "Idempotency" && consumed == "bynk";
    scope_idempotency_key(first_party, &method.name, &mut args_lowered, cx);
    if first_party && method.name == "claim" && cx.tracks_claims() {
        let cap_expr = format!("{}.{}", cx.cap_deps_expr(), cap);
        let cap_node = member(cx.cap_deps_node(), &cap);
        return Some(tracked_claim_call(cap_node, &cap_expr, args_lowered, cx));
    }
    let wire = capability_wire(e, cx);
    encode_capability_args(wire.as_deref(), &mut args_lowered, cx);
    let call = capability_op_call(
        cx.cap_deps_node(),
        cx.cap_deps_expr().to_string(),
        &cap,
        &method.name,
        capability_call_type_args_node(type_args, wire.as_deref()),
        capability_call_type_args_ts(type_args, wire.as_deref()),
        args_lowered,
    );
    Some(decode_capability_reply(
        call,
        wire.as_deref(),
        &cap,
        &method.name,
        cx,
    ))
}

/// `<deps>.<cap>.<op><T>(args)`: a node when every argument is one, else the text. `deps` is the
/// receiver as a node and as text; the type arguments are empty when there is nothing to name.
fn capability_op_call(
    deps: TsExpr,
    deps_text: String,
    cap: &str,
    op: &str,
    type_args: Vec<TsType>,
    type_args_text: String,
    args: Vec<LoweredExpr>,
) -> Built {
    node_or_text(
        args,
        |nodes| {
            let callee = chain(deps, &[cap, op]);
            let callee = if type_args.is_empty() {
                callee
            } else {
                TsExpr::TypeApp {
                    callee: Box::new(callee),
                    type_args,
                }
            };
            call(callee, nodes)
        },
        |texts| {
            format!(
                "{deps_text}.{cap}.{op}{type_args_text}({})",
                texts.join(", ")
            )
        },
    )
}
