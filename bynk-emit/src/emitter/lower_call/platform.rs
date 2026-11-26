//! The capability and platform calls of a method call (back-half paydown 1A.24, #3725): a
//! capability call (`deps.Cap.op(args)`, in a handler, a provider, a test case or through a
//! `D1.peek` handle), `Events.emit`, the session and typed cookies, and the derived-`D1`
//! `select`/`firstAs`, with their helpers. Moved out of `lower.rs` byte for byte (Decision F).

use bynk_check::checker::codable::CapEncoding;
use bynk_check::checker::{Callee, Ty};
use bynk_check::firstparty::FirstPartyCap;
use bynk_ts::{TsExpr, TsLit, TsStmt, TsType};

use super::super::emit::flatten_emit_ident_chain;
use super::super::lower::Pre;
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::ts_type_ref_to_ts_type;
use super::super::{Expr, ExprKind, Ident, TypeBody, TypeRef, test_scaffold_ns};
use super::codec_statics::{DecodeSource, json_decoder_fn};
use crate::ts_build::{
    arrow_with, binary, block_arrow, call, const_typed, expr_stmt, ident, member, method_call,
    num_lit, paren, ret, str_lit,
};
use bynk_ts::TsParam;

/// `"text" as const`: a string-literal tag that keeps its literal type.
fn tag_const(text: &str) -> TsExpr {
    TsExpr::AsConst(Box::new(str_lit(text)))
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
    path.iter().fold(root, |object, name| member(object, *name))
}

/// Lower every argument, in order.
fn lower_args(args: &[Expr], cx: &mut LowerCtx, pre: &mut Pre) -> Vec<bynk_ts::TsExpr> {
    args.iter().map(|a| pre.lower_node(a, cx)).collect()
}

/// `deps` in a test case or on a `D1.peek` handle, else the handler's own deps receiver, as a node
/// and as the text the fallbacks print.
fn deps_receiver(in_test: bool, cx: &LowerCtx) -> TsExpr {
    if in_test {
        ident("deps")
    } else {
        cx.cap_deps_node()
    }
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

/// #3945: the identity of a generic capability operation call's type arguments (their resolved types: `[Outcome]`), as the trailing
/// argument `...([{ __bynkTypeKey: "ReserveOutcome" }] as unknown as [])` of a test build: a stub
/// or an observation that names the type arguments (`Cap.op[T]`) tells the calls apart by it. The
/// real binding ignores a trailing argument, and the spread of an empty tuple keeps `tsc` quiet.
/// `None` outside a test build, and for a call with no type arguments.
fn type_key_arg(e: &Expr, type_args: &[TypeRef], cx: &LowerCtx) -> Option<TsExpr> {
    if !cx.typed_capability_calls() {
        return None;
    }
    let resolved: Vec<bynk_check::checker::TyId> = match cx.commons().callee(e.id) {
        Some(Callee::Capability { type_args, .. }) => type_args.clone(),
        _ => Vec::new(),
    };
    if type_args.is_empty() && resolved.is_empty() {
        return None;
    }
    // #4382: the call's resolved list, written arguments first as a `stub` spells them and the
    // inferred rest after, so a clause naming the leading ones matches a call that inferred them.
    let mut shown: Vec<String> = type_args
        .iter()
        .map(|t| strip_key_brackets(&type_key_of(std::slice::from_ref(t), cx)))
        .collect();
    let types = cx.ty_render().types();
    shown.extend(resolved.iter().skip(shown.len()).map(|t| types.display(*t)));
    Some(type_key_expr(
        &["[", shown.join(", ").as_str(), "]"].concat(),
    ))
}

fn strip_key_brackets(key: &str) -> String {
    key.strip_prefix('[')
        .and_then(|k| k.strip_suffix(']'))
        .unwrap_or(key)
        .to_string()
}

/// The key of `type_args` as a call site spells them: a consumed type this module renames
/// (`consumes c { T as U }`) is keyed by the name its owner declares, as a `stub` names it.
pub(in crate::emitter) fn type_key_of(type_args: &[TypeRef], cx: &LowerCtx) -> String {
    let declared: Vec<TypeRef> = type_args
        .iter()
        .map(|t| {
            let renamed = if let TypeRef::Named(id) = t {
                cx.consumed_declared_name(&id.name).map(|original| {
                    TypeRef::Named(bynk_syntax::ast::Ident {
                        name: original.to_string(),
                        span: id.span,
                    })
                })
            } else {
                None
            };
            renamed.unwrap_or_else(|| t.clone())
        })
        .collect();
    bynk_check::checker::capability_type_key(&declared, &cx.commons().types, cx.ty_render().types())
}

fn type_key_expr(key: &str) -> TsExpr {
    let entry = object(vec![("__bynkTypeKey", str_lit(key))]);
    TsExpr::Spread(Box::new(TsExpr::As {
        expr: Box::new(TsExpr::As {
            expr: Box::new(TsExpr::array(vec![entry])),
            ty: TsType::named("unknown"),
        }),
        ty: TsType::named("[]"),
    }))
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
    args: Vec<bynk_ts::TsExpr>,
    cx: &LowerCtx,
) -> Vec<bynk_ts::TsExpr> {
    let Some(wire) = wire else {
        return args;
    };
    let encoded = |ty: &bynk_check::checker::TyId, arg: bynk_ts::TsExpr| cx.encode_call(*ty, arg);
    let mut types = wire.encode.iter();
    args.into_iter()
        .map(|arg| match types.next() {
            Some(Some(ty)) => encoded(ty, arg),
            _ => arg,
        })
        .collect()
}

/// Typed capability reads (#3458): a call to an operation whose `Effect[P]`
/// payload mentions a `Codable` parameter gets the provider's plain-JSON reply
/// decoded as the instantiated `P`, whole, with the codec of `P`. What a reply
/// that does not decode means is the operation's declared policy (a fault, or a
/// miss for an `Option` payload); the runtime helper applies it. Other calls
/// are returned as they are.
pub(in crate::emitter) fn decode_capability_reply(
    call: bynk_ts::TsExpr,
    wire: Option<&CapEncoding>,
    cap: &str,
    op: &str,
    cx: &LowerCtx,
) -> bynk_ts::TsExpr {
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
    crate::ts_build::call(
        ident("__bynkCapabilityReply"),
        vec![
            call,
            codec,
            object(vec![
                ("capability", str_lit(cap)),
                ("operation", str_lit(op)),
                ("type", str_lit(&payload)),
            ]),
            str_lit(policy),
        ],
    )
}

/// #4116: whether an `Idempotency` key argument is statically text: a `String`, or a refined or
/// opaque `String`. A type variable is not, and goes through `__bynkIdempotencyKey`, which stores
/// a run-time string as it is.
fn key_is_text(key: &Expr, cx: &LowerCtx) -> bool {
    use bynk_check::checker::{NamedKind, Ty};
    use bynk_syntax::ast::BaseType;
    // An unrecorded type is not known to be text: the run-time helper decides.
    let Some(te) = cx.commons().expr_types.get(&key.id) else {
        return false;
    };
    let ty = te.ty.get(cx.ty_render().types());
    matches!(
        &*ty,
        Ty::Base(BaseType::String)
            | Ty::Named {
                kind: NamedKind::Refined(BaseType::String) | NamedKind::Opaque(BaseType::String),
                ..
            }
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
    args_lowered: &mut Vec<bynk_ts::TsExpr>,
    key_arg: Option<&Expr>,
    cx: &LowerCtx,
) {
    if !is_first_party || !matches!(method, "dedup" | "remember" | "claim") {
        return;
    }
    let scope = cx
        .handler_scope()
        .unwrap_or_else(|| panic!("Idempotency.{method} lowered with no handler_scope set"));
    let mut key = args_lowered.remove(0);
    // #4116: a key that is not text is stored under its canonical encoding. A `String`, a
    // refined `String` and an opaque `String` are stored as they were, so existing entries match.
    if key_arg.is_some_and(|k| !key_is_text(k, cx)) {
        cx.runtime_use().note_cmp("__bynkIdempotencyKey");
        key = call(ident("__bynkIdempotencyKey"), vec![key]);
    }
    // `TemplateLit` parts print verbatim, so the scope prints as it is spelled.
    let scoped = TsExpr::template_lit(vec![format!("{scope}::"), String::new()], vec![key]);
    args_lowered.insert(0, scoped);
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
    args: Vec<bynk_ts::TsExpr>,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    cx.claim_scope_used = true;
    // The bare `await` below makes any enclosing thunk or IIFE effectful —
    // see `emitted_await` (a `par` part that is a claim call must be `async`).
    cx.emitted_await = true;
    {
        let nodes = args;
        let k = || ident("__k");
        let t = || ident("__t");
        let claim = TsExpr::Await(Box::new(call(
            member(cap_node, "claim"),
            vec![k(), call(member(ident("__claims"), "lease"), vec![t()])],
        )));
        let note = call(member(ident("__claims"), "note"), vec![k(), claim, t()]);
        let note_arrow = TsExpr::Arrow {
            params: vec![
                TsParam::new("__k", Some(TsType::named("string"))),
                TsParam::new("__t", Some(TsType::named("number"))),
            ],
            is_async: true,
            generics: vec![],
            return_type: None,
            body: Box::new(bynk_ts::TsArrowBody::Expr(Box::new(note))),
        };
        paren(TsExpr::Await(Box::new(call(paren(note_arrow), nodes))))
    }
}

/// #3147: lower `D1.peek(<participant>.<Cap>)` in an integration or `system`
/// case, or `None` when the call is not one. It holds the participant's own
/// capability over its test database, read-only, as `deps.<local>`: the same
/// `WorkersD1` the app's code calls, over the very database object the
/// participant's `env` holds, so a call on the handle (an ordinary capability
/// call on `deps.<local>`) sees what the app wrote.
pub(in crate::emitter) fn lower_d1_peek_call(
    e: &Expr,
    cx: &mut LowerCtx,
) -> Option<bynk_ts::TsExpr> {
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
        chain(ident("deps"), &[&format!("env_{ns}"), &binding, "readOnly"]),
        vec![],
    );
    let workers_d1 = TsExpr::New {
        callee: Box::new(chain(ident("bynk_cloudflare__binding"), &["WorkersD1"])),
        args: vec![object(vec![(&binding, read_only)]), str_lit(&binding)],
    };
    Some(paren(TsExpr::Assign {
        target: Box::new(member(ident("deps"), &local)),
        value: Box::new(workers_d1),
    }))
}

/// Lower a derived-`D1` `select`/`firstAs` call (#2865), or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167) unchanged.
pub(in crate::emitter) fn lower_d1_select_call(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
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
    // A checked row type has a JSON decoder; one that has none is a missing fact (1A.34).
    let decoder = json_decoder_fn(target, source, cx)
        .unwrap_or_else(|| cx.missing_fact(e.span, "JSON decoder for a `select` row type"));
    let params = pre.lower_node(params, cx);
    let in_test = cx.in_test_case() || cap.starts_with(bynk_check::checker::peek::LOCAL_PREFIX);
    let deps_node = deps_receiver(in_test, cx);
    let op = if first { "first" } else { "all" };
    Some({
        let nodes = vec![decoder, params];
        let [decoder, params] = super::super::lower_list::take(nodes);
        let r = || ident("__r");
        let d = || ident("__d");
        let tag_is = |obj: TsExpr, tag: &str| {
            binary(
                bynk_ts::TsBinaryOp::StrictEq,
                member(obj, "tag"),
                str_lit(tag),
            )
        };
        let err = |tag: &str, error: TsExpr| {
            call(
                ident("Err"),
                vec![object(vec![("tag", tag_const(tag)), ("error", error)])],
            )
        };
        let mut body = vec![
            const_typed("__decode", None, decoder),
            const_typed(
                "__r",
                None,
                TsExpr::Await(Box::new(call(
                    chain(deps_node, &[&cap, op]),
                    vec![str_lit(&sql), params],
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
                ret(call(ident("Ok"), vec![ident("None")])),
                None,
            ));
            chain(r(), &["value", "value"])
        } else {
            member(r(), "value")
        };
        body.push(const_typed(
            "__d",
            None,
            call(ident("__decode"), vec![decode_arg]),
        ));
        let ok_value = if first {
            call(ident("Some"), vec![member(d(), "value")])
        } else {
            member(d(), "value")
        };
        body.push(ret(TsExpr::Conditional {
            test: Box::new(tag_is(d(), "Ok")),
            consequent: Box::new(call(ident("Ok"), vec![ok_value])),
            alternate: Box::new(err(decode_tag, member(d(), "error"))),
        }));
        call(paren(block_arrow(vec![], true, body)), vec![])
    })
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
) -> Option<bynk_ts::TsExpr> {
    if let Some(Callee::SessionRevokeAll { actor }) = cx.commons().callees.get(&e.id).cloned() {
        return lower_session_revoke_all(&actor, args.first()?, cx, pre);
    }
    let Some(Callee::SignedCookie { actor, issue }) = cx.commons().callees.get(&e.id).cloned()
    else {
        return None;
    };
    let seam = cx.commons().commons.items.iter().find_map(|i| {
        i.as_actor()
            .filter(|a| a.name.name == actor)
            .and_then(|a| bynk_check::actors::signed_cookie_seam_of(a, None))
    })?;
    if !issue {
        let expire = call(
            ident("__expireSessionCookie"),
            vec![str_lit(&seam.cookie), str_lit(&seam.same_site)],
        );
        if !seam.sessions {
            return Some(expire);
        }
        return Some(lower_session_expire(&seam, expire, cx));
    }
    let key = pre.lower_node(args.first()?, cx);
    let deps_node = deps_receiver(cx.in_test_case(), cx);
    Some({
        let nodes = vec![key];
        let [key] = super::super::lower_list::take(nodes);
        let now = TsExpr::Await(Box::new(call(
            chain(deps_node.clone(), &["Clock", "now"]),
            vec![],
        )));
        let mut issue_args = vec![
            call(
                ident("__sessionKeysOf"),
                vec![deps_node.clone(), str_lit(&actor)],
            ),
            str_lit(&actor),
            str_lit(&seam.cookie),
            str_lit(&seam.same_site),
            key.clone(),
            num_lit(seam.ttl_secs),
            now,
        ];
        if seam.sessions {
            // #3938: record the new session in the identity's session agent before the cookie is
            // signed, so a cookie never exists for a session the agent does not hold.
            let agent = bynk_check::actors::sessions_agent_name(&actor);
            cx.record_agent_call(&agent, "add");
            let instance = cx.agent_construct_node(&agent, ident("__sub"));
            issue_args[4] = ident("__sub");
            issue_args.push(arrow_with(
                vec![
                    TsParam::new("__sid".to_string(), Some(TsType::named("string"))),
                    TsParam::new("__exp".to_string(), Some(TsType::named("number"))),
                    TsParam::new("__now".to_string(), Some(TsType::named("number"))),
                ],
                true,
                method_call(
                    instance,
                    "add",
                    vec![ident("__sid"), ident("__exp"), ident("__now"), deps_node],
                ),
            ));
            let issue = call(ident("__issueSessionCookie"), issue_args);
            return Some(call(
                paren(arrow_with(
                    vec![TsParam::new(
                        "__sub".to_string(),
                        Some(TsType::named(seam.identity_type.clone())),
                    )],
                    true,
                    issue,
                )),
                vec![key],
            ));
        }
        let issue = call(ident("__issueSessionCookie"), issue_args);
        call(paren(arrow_with(vec![], true, issue)), vec![])
    })
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
) -> Option<bynk_ts::TsExpr> {
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
        return Some(typed_cookie_node(
            &name,
            str_lit(""),
            call(ident("Some"), vec![num_lit(0)]),
            call(ident("Some"), vec![num_lit(0)]),
        ));
    }
    // One shape is peeled: an `Option` layer around the field's type.
    let leaf = if let TypeRef::Option(inner, _) = &f.type_ref {
        inner.as_ref()
    } else {
        &f.type_ref
    };
    let kind = crate::emitter::workers_entry::query_leaf_kind(leaf, types, false);
    let value = pre.lower_node(args.first()?, cx);
    let decimal = kind == "decimal";
    if decimal {
        // A `Decimal` is its rendered text on the wire; the runtime helper takes it as such.
        cx.note_decimal("__bynkDecimalToString");
    }
    let none = || object(vec![("tag", tag_const("None"))]);
    Some({
        let nodes = vec![value];
        let [value] = super::super::lower_list::take(nodes);
        let value = if decimal {
            call(ident("__bynkDecimalToString"), vec![value])
        } else {
            value
        };
        typed_cookie_node(
            &name,
            call(ident("__cookieValue"), vec![str_lit(kind), value]),
            none(),
            none(),
        )
    })
}

/// [`typed_cookie_literal`] as a node: the field's wire `name` is raw text (the printer escapes it).
fn typed_cookie_node(name: &str, value: TsExpr, max_age: TsExpr, expires: TsExpr) -> TsExpr {
    paren(object(vec![
        ("name", str_lit(name)),
        ("value", value),
        ("maxAge", max_age),
        ("expires", expires),
        ("domain", object(vec![("tag", tag_const("None"))])),
        ("path", str_lit("/")),
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
    _type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
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
    // identical classification, project-wide. 1A.33: whether this `Events` is
    // bynk's own is the callee's `first_party`, resolved once by the checker.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Capability { op, first_party: Some(FirstPartyCap::Events), .. })
                if op == "emit"
        )
    {
        // #4113: the checker records the event the call emits (written or inferred) on the callee.
        let event_name = match cx.commons().callee(e.id) {
            Some(Callee::Capability {
                event: Some(name), ..
            }) => name.clone(),
            // `Events.emit` names an event the checker resolved: absent, a missing fact (1A.34).
            // The name is only a placeholder then.
            _ => {
                cx.missing_fact(e.span, "event type argument of `Events.emit`");
                String::new()
            }
        };
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
        // The envelope is a node over raw text; the text form escapes it as it always did.
        return Some({
            let nodes = vec![payload];
            let [payload] = super::super::lower_list::take(nodes);
            let global = |name: &str| member(ident("globalThis"), name);
            let envelope = object(vec![
                (
                    "eventId",
                    call(member(global("crypto"), "randomUUID"), vec![]),
                ),
                ("publisherId", str_lit(&owner)),
                ("emittedAt", call(member(global("Date"), "now"), vec![])),
                ("schemaVersion", num_lit(schema_version)),
                // #3895: the trace this emission happened in; `undefined` outside
                // one, which `JSON.stringify` leaves off the wire.
                ("trace", call(ident("__currentTraceId"), vec![])),
            ]);
            let entry = object(vec![
                ("type", str_lit(&event_name)),
                ("payload", payload),
                ("envelope", envelope),
            ]);
            let push = expr_stmt(call(member(ident("__events"), "push"), vec![entry]));
            call(paren(block_arrow(vec![], true, vec![push])), vec![])
        });
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
) -> Option<bynk_ts::TsExpr> {
    // #4105: `Cap.writeValue[T](value)` is lowered over the instance's `write`.
    if let Some(Callee::AnalyticsWrite { cap, mapping }) = cx.commons().callee(e.id).cloned() {
        let value = pre.lower_node(args.first()?, cx);
        let point = analytics_point(value, &mapping, cx);
        let deps = deps_receiver(cx.in_test_case(), cx);
        let call = capability_op_call(deps, &cap, "write", Vec::new(), vec![point]);
        let target = format!("{cap}.writeValue");
        return Some(cx.trace_step_node("capability", &target, e.span.start, call));
    }
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
        name_analytics_dataset(&recv.name, &method.name, args, &mut args_lowered, cx);
        let wire = capability_wire(e, cx);
        let mut args_lowered = encode_capability_args(wire.as_deref(), args_lowered, cx);
        args_lowered.extend(type_key_arg(e, type_args, cx));
        let call = capability_op_call(
            ident("deps"),
            &recv.name,
            &method.name,
            Vec::new(),
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
        let args_lowered = lower_args(args, cx, pre);
        let wire = capability_wire(e, cx);
        let args_lowered = encode_capability_args(wire.as_deref(), args_lowered, cx);
        let call = capability_op_call(ident("deps"), &cap, &method.name, Vec::new(), args_lowered);
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
        name_analytics_dataset(&recv.name, &method.name, args, &mut args_lowered, cx);
        // #934: a flattened `consumes bynk { Idempotency }` local name is first-party only if
        // it's actually `bynk`'s own capability. The checker resolved that for this call
        // (`firstparty::resolve`), so a same-named capability from any other adapter or context
        // (e.g. the #926/#931 generic-op test fixtures' own illustrative `capability
        // Idempotency`) is not scoped.
        let is_first_party = matches!(
            cx.commons().callee(e.id),
            Some(Callee::Capability {
                first_party: Some(FirstPartyCap::Idempotency),
                ..
            })
        );
        scope_idempotency_key(
            is_first_party,
            &method.name,
            &mut args_lowered,
            args.first(),
            cx,
        );
        if is_first_party && method.name == "claim" && cx.tracks_claims() {
            let cap_node = member(cx.cap_deps_node(), &recv.name);
            return Some(tracked_claim_call(cap_node, args_lowered, cx));
        }
        let wire = capability_wire(e, cx);
        let mut args_lowered = encode_capability_args(wire.as_deref(), args_lowered, cx);
        args_lowered.extend(type_key_arg(e, type_args, cx));
        let call = capability_op_call(
            cx.cap_deps_node(),
            &recv.name,
            &method.name,
            capability_call_type_args_node(type_args, wire.as_deref()),
            args_lowered,
        );
        let decoded = decode_capability_reply(call, wire.as_deref(), &recv.name, &method.name, cx);
        let target = format!("{}.{}", recv.name, method.name);
        return Some(cx.trace_step_node("capability", &target, e.span.start, decoded));
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
) -> Option<bynk_ts::TsExpr> {
    let chain = flatten_emit_ident_chain(receiver)?;
    let (_, cap) = cx.cross_context().resolve_cross_capability(&chain)?;
    let mut args_lowered = lower_args(args, cx, pre);
    let first_party = matches!(
        cx.commons().callee(e.id),
        Some(Callee::CrossCap {
            first_party: Some(FirstPartyCap::Idempotency),
            ..
        })
    );
    scope_idempotency_key(
        first_party,
        &method.name,
        &mut args_lowered,
        args.first(),
        cx,
    );
    if first_party && method.name == "claim" && cx.tracks_claims() {
        let cap_node = member(cx.cap_deps_node(), &cap);
        return Some(tracked_claim_call(cap_node, args_lowered, cx));
    }
    let wire = capability_wire(e, cx);
    let mut args_lowered = encode_capability_args(wire.as_deref(), args_lowered, cx);
    args_lowered.extend(type_key_arg(e, type_args, cx));
    let call = capability_op_call(
        cx.cap_deps_node(),
        &cap,
        &method.name,
        capability_call_type_args_node(type_args, wire.as_deref()),
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
/// #4104: `AnalyticsSql.query(Signups, sql, params)` passes the instance's table, not the instance:
/// its first argument, which the checker required to name an `Analytics` instance, is lowered to
/// `await deps.Signups.dataset()`, the environment's qualified table, which the binding puts where
/// the SQL says `{dataset}`.
fn name_analytics_dataset(
    cap: &str,
    op: &str,
    args: &[Expr],
    lowered: &mut [bynk_ts::TsExpr],
    cx: &mut LowerCtx<'_>,
) {
    if cap != "AnalyticsSql" || op != "query" || lowered.len() != 3 {
        return;
    }
    // The checker decided this argument is an `Analytics` instance, and recorded it.
    if let Some(first) = args.first()
        && let ExprKind::Ident(instance) = &first.kind
        && matches!(
            cx.commons().callee(first.id),
            Some(Callee::Capability { cap, op, .. }) if *cap == instance.name && op == "dataset"
        )
    {
        let table = call(
            chain(cx.cap_deps_node(), &[instance.name.as_str(), "dataset"]),
            Vec::new(),
        );
        lowered[0] = TsExpr::Await(Box::new(table));
    }
}

/// #4105: the `DataPoint` a record is, from the mapping the checker computed: `((__v) => ({ index,
/// blobs, doubles }))(value)`, so the value is evaluated once however many columns read it.
fn analytics_point(
    value: TsExpr,
    mapping: &bynk_check::checker::analytics::AnalyticsMapping,
    cx: &mut LowerCtx,
) -> TsExpr {
    use bynk_check::checker::analytics::ColumnKind;
    let v = || ident("__v");
    let column = |f: &bynk_check::checker::analytics::ColumnField, cx: &mut LowerCtx| {
        let field = member(v(), &f.field);
        match f.kind {
            ColumnKind::Text | ColumnKind::Number => field,
            ColumnKind::Variant => member(field, "tag"),
            ColumnKind::Flag => call(ident("String"), vec![field]),
            ColumnKind::Decimal => {
                cx.note_decimal("__bynkDecimalToFloat");
                call(ident("__bynkDecimalToFloat"), vec![field])
            }
        }
    };
    let index = match &mapping.index {
        Some(f) => call(ident("Some"), vec![member(v(), f)]),
        None => ident("None"),
    };
    let blobs = mapping.blobs.iter().map(|f| column(f, cx)).collect();
    let doubles = mapping.doubles.iter().map(|f| column(f, cx)).collect();
    let point = object(vec![
        ("index", index),
        (
            "blobs",
            TsExpr::Array {
                items: blobs,
                multiline: false,
            },
        ),
        (
            "doubles",
            TsExpr::Array {
                items: doubles,
                multiline: false,
            },
        ),
    ]);
    call(
        paren(arrow_with(
            vec![TsParam::new("__v", None)],
            false,
            paren(point),
        )),
        vec![value],
    )
}

fn capability_op_call(
    deps: TsExpr,
    cap: &str,
    op: &str,
    type_args: Vec<TsType>,
    args: Vec<bynk_ts::TsExpr>,
) -> bynk_ts::TsExpr {
    let nodes = args;
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
}

/// #3938: `Actor.expire()` on a `sessions = true` actor: end the verified session in the identity's
/// session agent (when the handler is guarded by the actor, so the seam threaded one), then return
/// the deleting cookie. A handler the actor does not guard has no session to end, so only the
/// cookie is returned.
fn lower_session_expire(
    seam: &bynk_check::actors::SignedCookieSeam,
    expire: bynk_ts::TsExpr,
    cx: &mut LowerCtx,
) -> bynk_ts::TsExpr {
    use crate::ts_build::{as_expr, await_expr, if_, strict_neq};
    let agent = bynk_check::actors::sessions_agent_name(&seam.actor);
    cx.record_agent_call(&agent, "remove");
    let deps_node = deps_receiver(cx.in_test_case(), cx);
    let current = ident("__current");
    let identity = as_expr(
        member(current.clone(), "sub"),
        TsType::named(seam.identity_type.clone()),
    );
    let instance = cx.agent_construct_node(&agent, identity);
    let body = vec![
        crate::ts_build::const_(
            "__current",
            call(ident("__currentSession"), vec![deps_node.clone()]),
        ),
        if_(
            strict_neq(current.clone(), crate::ts_build::null_lit()),
            expr_stmt(await_expr(method_call(
                instance,
                "remove",
                vec![member(current, "sid"), deps_node],
            ))),
        ),
        ret(expire),
    ];
    call(paren(block_arrow(vec![], true, body)), vec![])
}

/// #3938: `Actor.revokeAll(identity)`: end every session of `identity` in its session agent. An
/// effect, like `issue`: an async arrow applied to the identity, whose agent call is awaited.
fn lower_session_revoke_all(
    actor: &str,
    identity: &Expr,
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    let seam = cx.commons().commons.items.iter().find_map(|i| {
        i.as_actor()
            .filter(|a| a.name.name == actor)
            .and_then(|a| bynk_check::actors::signed_cookie_seam_of(a, None))
    })?;
    let key = pre.lower_node(identity, cx);
    let deps_node = deps_receiver(cx.in_test_case(), cx);
    let agent = bynk_check::actors::sessions_agent_name(actor);
    cx.record_agent_call(&agent, "revokeAll");
    let instance = cx.agent_construct_node(&agent, ident("__sub"));
    let revoke = crate::ts_build::await_expr(method_call(instance, "revokeAll", vec![deps_node]));
    Some(call(
        paren(block_arrow(
            vec![TsParam::new(
                "__sub".to_string(),
                Some(TsType::named(seam.identity_type)),
            )],
            true,
            vec![expr_stmt(revoke)],
        )),
        vec![key],
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emitter::ctx::{BodyMode, ModuleCtx, UnitFacts};
    use crate::emitter::runtime_use::RuntimeUse;
    use bynk_check::checker::TypedCommons;

    /// The printer and `escape_ts_string` are one function, so a raw-text `Str` node prints what
    /// the text path printed, for any name, path or SQL string, non-ASCII included.
    #[test]
    fn the_literal_node_prints_the_escaped_text() {
        for s in [
            "plain",
            "caf\u{e9}",
            "\u{65e5}\u{672c}\u{8a9e}",
            "a \"q\" \\ b",
            "line\nbreak\ttab\r",
            "nul\0ctl\u{7f}",
            "sep\u{2028}\u{2029}",
            "\u{1f600} emoji",
            "SELECT \"a\" FROM t WHERE x = 'y'",
        ] {
            assert_eq!(
                bynk_ts::print_expr(&str_lit(s)),
                format!("\"{}\"", crate::emitter::escape_ts_string(s)),
                "{s:?}"
            );
        }
    }

    /// With a `wire` encoding only the kept type-argument flags survive, and none kept names
    /// nothing.
    #[test]
    fn the_wire_filter_drops_the_unkept_type_arguments() {
        use bynk_check::checker::codable::CapEncoding;
        use bynk_syntax::span::Span;
        let named = |n: &str| {
            TypeRef::Named(Ident {
                name: n.to_string(),
                span: Span::default(),
            })
        };
        let args = [named("A"), named("B"), named("C")];
        let wire = |keep: Vec<bool>| CapEncoding {
            encode: vec![None; 3],
            keep_type_args: keep,
            reply: None,
        };
        for (keep, text) in [
            (vec![true, false, true], "<A, C>"),
            (vec![false, false, false], ""),
            (vec![true, true, true], "<A, B, C>"),
        ] {
            let w = wire(keep);
            let nodes = capability_call_type_args_node(&args, Some(&w));
            let printed = if nodes.is_empty() {
                String::new()
            } else {
                format!(
                    "<{}>",
                    nodes
                        .iter()
                        .map(bynk_ts::print_type)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            assert_eq!(printed, text);
        }
        assert_eq!(capability_call_type_args_node(&args, None).len(), 3);
    }

    /// The typed-cookie node and the text literal print the same bytes.
    #[test]
    fn the_typed_cookie_node_prints_as_the_text_does() {
        let none = || object(vec![("tag", tag_const("None"))]);
        let node = typed_cookie_node("a\"b", str_lit("v"), none(), none());
        // The text literal the node replaced.
        let (name, value) = (crate::emitter::escape_ts_string("a\"b"), "\"v\"");
        let none = "{ tag: \"None\" as const }";
        let text = format!(
            "({{ name: \"{name}\", value: {value}, maxAge: {none}, expires: {none}, \
             domain: {{ tag: \"None\" as const }}, path: \"/\", secure: true, httpOnly: true, \
             sameSite: {{ tag: \"Lax\" as const }}, partitioned: false }})"
        );
        assert_eq!(bynk_ts::print_expr(&node), text);
    }

    /// `trace_step_node` prints `__traceStep("kind", "target", { file, line, col })(call)` in a
    /// traced build, and returns the call unchanged in a deploy build.
    #[test]
    fn the_trace_step_node_wraps_a_traced_call() {
        let commons = TypedCommons::empty();
        let cross = bynk_check::resolver::CrossContextInfo::default();
        let source: std::sync::Arc<str> = std::sync::Arc::from("a\n  Greeter.hello(\"x\")\n");
        let offset = source.find("Greeter").expect("the call is in the source");
        for traced in [false, true] {
            let ru = RuntimeUse::default();
            let mut module = ModuleCtx::new(&commons, &cross, &ru, UnitFacts::detached());
            if traced {
                module.trace = Some(crate::emit_model::TraceLoc {
                    source: source.clone(),
                    rel_path: "shop\\api \"x\".bynk".to_string(),
                });
            }
            let cx = LowerCtx::new(module, BodyMode::Method);
            let node = cx.trace_step_node(
                "capability",
                "Greeter.hello",
                offset,
                call(chain(ident("deps"), &["Greeter", "hello"]), vec![]),
            );
            let text = if traced {
                let (line, col) = bynk_syntax::span::line_col(&source, offset);
                format!(
                    "__traceStep(\"capability\", \"Greeter.hello\", {{ file: \"shop/api \\\"x\\\".bynk\", line: {line}, col: {col} }})(deps.Greeter.hello())"
                )
            } else {
                "deps.Greeter.hello()".to_string()
            };
            assert_eq!(bynk_ts::print_expr(&node), text, "traced: {traced}");
            assert_eq!(text.starts_with("__traceStep("), traced);
            // The wrapper names `__traceStep` in the tree, so a traced module imports it.
            let used =
                bynk_ts::unbound_names_in(&[bynk_ts::TsStmt::expr_stmt(node, None)], |_| false);
            assert_eq!(used.iter().any(|n| n == "__traceStep"), traced);
        }
    }
}
