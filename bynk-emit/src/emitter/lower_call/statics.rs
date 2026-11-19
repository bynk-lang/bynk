//! The intrinsic and type statics of a method call: `HttpResult.*`, `List`/`Map`/`Set`
//! `empty`, `Set.of`, `Float.pi`/`e`, the numeric parses, `Duration.millis`, `Instant.from*`,
//! the runtime-value statics (`Bytes.*`, `Decimal.*`, ...) and a static call on a declared type.
//! Moved out of `lower.rs` (back-half paydown 1A.20, #3721, Decision E): the first of the
//! `lower_call/` files the dispatch run (1A.20 to 1A.30) fills.

use bynk_check::builtin_names::types::*;
use bynk_check::checker::{Callee, Ty};
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsExpr, TsLit, TsParam, TsStmt, TsType, TsUnaryOp,
};

use super::super::lower::{
    Built, LoweredExpr, Pre, built_of, call_built, ident_node, lower_http_variant_args,
    lower_json_codec_call, member_node, node_or_text, runtime_ctor_call, settle_built,
};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::key_is_plain;
use super::super::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use super::super::{Expr, ExprKind, Ident, TypeRef};

/// `a.b`.
fn member(object: TsExpr, property: &str) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.to_string(),
    }
}

/// `globalThis.<name>`.
fn global(name: &str) -> TsExpr {
    member_node("globalThis", name)
}

/// `callee<A, B>`.
fn type_app(callee: TsExpr, type_args: Vec<TsType>) -> TsExpr {
    TsExpr::TypeApp {
        callee: Box::new(callee),
        type_args,
    }
}

/// `new callee(args)`.
fn new(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::New {
        callee: Box::new(callee),
        args,
    }
}

/// A call to a runtime helper's genuine identifier.
fn call(helper: &str, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(ident_node(helper)),
        args,
    }
}

/// The type arguments written at a call (`Box.empty[Int]()`), as nodes: the same
/// `TsType`s `ts_type_ref` prints, so the text and the node forms agree.
fn explicit_type_args(type_args: &[TypeRef]) -> Vec<TsType> {
    type_args
        .iter()
        .map(|t| ts_type_ref_to_ts_type(t, None))
        .collect()
}

/// Lower a static call on a declared type (`Box.empty[Int]()`), or `None` when the call is not one. Moved out of
/// `lower_method_call` (#3167); builds a node when its arguments are nodes (1A.20, #3721).
pub(in crate::emitter) fn lower_type_static_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    // Static call: receiver is a bare ident naming a declared type — unless
    // the checker resolved it as a test-service address, which a target's
    // service named like a `uses`-imported type it hides is (#1927).
    if let ExprKind::Ident(id) = &receiver.kind
        && cx.commons().types.contains_key(&id.name)
        && !matches!(
            cx.commons().callees.get(&e.id),
            Some(
                bynk_check::checker::Callee::TestService { .. }
                    | bynk_check::checker::Callee::TestAssetFetch { .. }
            )
        )
    {
        let mut all: Vec<LoweredExpr> = args.iter().map(|a| pre.lower_node(a, cx)).collect();
        // #2719: a static method in the needs-context set takes the context last.
        let mut needs_exec = false;
        if let Some(Callee::Static(decl, witnesses)) = cx.commons().callees.get(&e.id) {
            // Constrained generics slice 4: the call's witnesses follow the
            // ordinary arguments, as for an instance method.
            all.extend(
                cx.witness_args(decl, witnesses)
                    .into_iter()
                    .map(LoweredExpr::Node),
            );
            if cx.commons().exec_plan.fn_needs(decl) {
                all.push(LoweredExpr::Node(cx.exec_node()));
                needs_exec = true;
            }
        }
        // Slice 4 (DECISION B): explicit type arguments (`Box.empty[Int]()`) are
        // kept, so a call with no argument of type `T` still types as written.
        let mut callee = member_node(&id.name, &method.name);
        if !type_args.is_empty() {
            callee = type_app(callee, explicit_type_args(type_args));
        }
        let built = call_built(callee, all);
        return Some(if needs_exec {
            settle_built(built, cx)
        } else {
            built
        });
    }
    None
}

/// The `[+-]?digits` grammar `Int.parse` accepts (#1657), a fixed literal.
const INT_GRAMMAR: &str = "/^[+-]?[0-9]+$/";
/// The decimal-with-optional-exponent grammar `Float.parse` accepts (#1657).
const FLOAT_GRAMMAR: &str = "/^[+-]?([0-9]+(\\.[0-9]*)?|\\.[0-9]+)([eE][+-]?[0-9]+)?$/";

/// `Int.parse(s)` / `Float.parse(s)` (ADR 0048, #1657): the whole string must match
/// a strict decimal grammar before `Number(…)` sees it. `Number` alone accepts
/// surrounding whitespace, `0x`/`0b`/`0o` prefixes and (for `Int`) exponents, so
/// `" 7 "`, `"0x10"` and `"1e3"` parsed. `Int` is `[+-]?digits`, then a safe integer
/// (`+ 0` turns `-0` into `0`); `Float` is a decimal with an optional exponent, then
/// finite (the 0040 posture).
///
/// The one place a 1A slice puts literal text in a node: the grammar is a regular
/// expression literal, which `bynk_ts` has no node for, so it is a `TsLit::Raw`.
/// It is one of two fixed constants, never program text (#3721, Decision C).
fn parse_iife(is_int: bool, s: LoweredExpr) -> Built {
    let (grammar, value, guard_fn) = if is_int {
        (
            INT_GRAMMAR,
            TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Call {
                    callee: Box::new(global("Number")),
                    args: vec![ident_node("__s")],
                }),
                right: Box::new(TsExpr::Lit(TsLit::Num("0".to_string()))),
            },
            "isSafeInteger",
        )
    } else {
        (
            FLOAT_GRAMMAR,
            TsExpr::Call {
                callee: Box::new(global("Number")),
                args: vec![ident_node("__s")],
            },
            "isFinite",
        )
    };
    node_or_text(
        vec![s],
        |c| {
            let s = c.into_iter().next().expect("one child");
            let matches = TsExpr::Call {
                callee: Box::new(member(TsExpr::Lit(TsLit::Raw(grammar.to_string())), "test")),
                args: vec![ident_node("__s")],
            };
            let guard = TsExpr::Call {
                callee: Box::new(member(member_node("globalThis", "Number"), guard_fn)),
                args: vec![ident_node("__n")],
            };
            let body = vec![
                TsStmt::if_stmt(
                    TsExpr::Unary {
                        op: TsUnaryOp::Not,
                        expr: Box::new(matches),
                    },
                    TsStmt::return_stmt(Some(ident_node("None")), None),
                    None,
                ),
                TsStmt::const_stmt(TsBindingName::Ident("__n".to_string()), None, value, None),
                TsStmt::return_stmt(
                    Some(TsExpr::Conditional {
                        test: Box::new(guard),
                        consequent: Box::new(call("Some", vec![ident_node("__n")])),
                        alternate: Box::new(ident_node("None")),
                    }),
                    None,
                ),
            ];
            let arrow = TsExpr::Arrow {
                params: vec![TsParam {
                    name: "__s".to_string(),
                    ty: Some(TsType::named("string")),
                    optional: false,
                }],
                is_async: false,
                generics: vec![],
                return_type: None,
                body: Box::new(TsArrowBody::Block(body)),
            };
            TsExpr::Call {
                callee: Box::new(TsExpr::Paren(Box::new(arrow))),
                args: vec![s],
            }
        },
        |c| {
            let s = c.into_iter().next().expect("one child");
            let value = if is_int {
                "globalThis.Number(__s) + 0"
            } else {
                "globalThis.Number(__s)"
            };
            let guard = if is_int {
                "globalThis.Number.isSafeInteger(__n)"
            } else {
                "globalThis.Number.isFinite(__n)"
            };
            format!(
                "((__s: string) => {{ if (!{grammar}.test(__s)) return None; const __n = {value}; return {guard} ? Some(__n) : None; }})({s})"
            )
        },
    )
}

/// `Set.of(xs)` over a plain key is `new globalThis.Set<T>(xs)`; over a keyed one
/// the runtime's `__bynkSetOf<T>(xs)`.
fn set_of(plain: bool, elem: TsType, xs: LoweredExpr) -> Built {
    let one = |c: Vec<TsExpr>| c.into_iter().next().expect("one child");
    let text_ty = bynk_ts::print_type(&elem);
    let node_elem = elem.clone();
    node_or_text(
        vec![xs],
        move |c| {
            if plain {
                new(type_app(global("Set"), vec![node_elem]), vec![one(c)])
            } else {
                TsExpr::Call {
                    callee: Box::new(type_app(ident_node("__bynkSetOf"), vec![node_elem])),
                    args: vec![one(c)],
                }
            }
        },
        move |c| {
            let xs = c.into_iter().next().expect("one child");
            if plain {
                format!("new globalThis.Set<{text_ty}>({xs})")
            } else {
                format!("__bynkSetOf<{text_ty}>({xs})")
            }
        },
    )
}

/// Lower the built-in scalar and collection statics (`HttpResult.*`, `List.empty`, `Map.empty`, `Set.empty`/`of`, the JSON codec, `Float.pi`/`e`, the numeric parses, `Duration.millis`, `Instant.from*`), or `None` when the call is none of them. Moved out of
/// `lower_method_call` (#3167); builds a node when its arguments are nodes (1A.20, #3721).
pub(in crate::emitter) fn lower_scalar_static_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    let tys = cx.ty_render();
    // v0.9: explicit `HttpResult.Variant(args)` construction. The
    // checker has already recorded the expression's type — emit it
    // directly through the runtime's HttpResult namespace.
    //
    // P6.21/P6.23 (review of #1251): reads `Callee::Intrinsic` (the sink
    // #1251 added) instead of `id.name == HTTP_RESULT` — closes the last
    // real instance of R6.5's name-matched-receiver defect class this
    // session's own P6.21 slices left open, since Decision C originally
    // excluded this shape for lack of a `Callee` to read.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, .. }) if *ns == HTTP_RESULT
        )
    {
        cx.note_http_result();
        let args_lowered = lower_http_variant_args(&method.name, args, pre, cx);
        return Some(call_built(
            member_node("HttpResult", &method.name),
            args_lowered,
        ));
    }
    // v0.20b: built-in collection statics — `List.empty()` /
    // `Map.empty()`. The checker recorded the instantiated type;
    // emit it explicitly so the TS value doesn't infer as `never[]`
    // / `Map<unknown, unknown>` outside contextually-typed positions.
    //
    // P6.21 (partial, continued — the earlier #1247 attempt reverted this
    // exact conversion after a zero-diff bless caught it silently dropping
    // `Instant`/`Bytes` statics inside `.test.bynk` bodies; root-caused and
    // fixed separately — `typecheck_case_body` now returns its own
    // `callees` map instead of discarding it, so `Callee::Intrinsic` is
    // populated for test-case bodies too): reads `Callee::Intrinsic` (P6.0)
    // instead of `id.name == LIST || id.name == MAP` plus
    // `!cx.commons().types.contains_key` — the checker's own
    // `check_method_call` already resolves the identical shadowing question
    // (`ctx.lookup(...).is_none() && !ctx.input.types.contains_key(...)`,
    // `calls.rs`) before ever recording this `Callee`, closing a real gap
    // the emitter's own guard left open: no `cx.is_local(&id.name)` check,
    // unlike every other branch this session already converted.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if (*ns == LIST || *ns == MAP || *ns == SET) && op == "empty"
        )
        && args.is_empty()
    {
        match cx.commons().expr_ty(e.id).as_deref() {
            // K6a (#2596).
            Some(Ty::Set(t)) => {
                return Some(Ok(new(
                    type_app(global("Set"), vec![ts_ty_to_ts_type(*t, tys)]),
                    vec![],
                )));
            }
            Some(Ty::List(t)) => {
                // `([] as readonly T[])`: the bracket keeps the cast one operand.
                return Some(Ok(TsExpr::Paren(Box::new(TsExpr::As {
                    expr: Box::new(TsExpr::array(vec![])),
                    ty: TsType::readonly_array(ts_ty_to_ts_type(*t, tys)),
                }))));
            }
            Some(Ty::Map(k, v)) => {
                return Some(Ok(new(
                    type_app(
                        global("Map"),
                        vec![ts_ty_to_ts_type(*k, tys), ts_ty_to_ts_type(*v, tys)],
                    ),
                    vec![],
                )));
            }
            _ => {}
        }
    }
    // K6a (#2596): `Set.of(xs)` — a set of the list's elements; the first
    // occurrence keeps its position.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == SET && op == "of"
        )
        && args.len() == 1
        && let Some(Ty::Set(t)) = cx.commons().expr_ty(e.id).as_deref()
    {
        let t = *t;
        let xs = pre.lower_node(&args[0], cx);
        let plain = key_is_plain(t, tys.types());
        if !plain {
            cx.runtime_use().note_key("__bynkSetOf");
        }
        return Some(set_of(plain, ts_ty_to_ts_type(t, tys), xs));
    }
    // v0.22b: the typed JSON codec (ADR 0045).
    // 1A: the codec's decoders and encoders are their own family (1A.21); it
    // stays as it is, and a node it returns passes through.
    if let Some(built) = pre.absorb_built(lower_json_codec_call(e, receiver, method, args, cx)) {
        return Some(built);
    }
    // K6b (#2598): `Float.pi()` and `Float.e()`.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == FLOAT && (op == "pi" || op == "e")
        )
        && args.is_empty()
    {
        let constant = if method.name == "pi" { "PI" } else { "E" };
        return Some(Ok(member(member_node("globalThis", "Math"), constant)));
    }
    // v0.22a: the numeric parse statics — `Int.parse(s)` / `Float.parse(s)`
    // (ADR 0048; the grammar is on [`parse_iife`]).
    // P6.21 (partial, continued): reads `Callee::Intrinsic` instead of
    // `id.name == INT || id.name == FLOAT` — see the `List`/`Map` branch
    // above. `Int`/`Float` are lexically reserved (the parser only admits
    // them here followed by `.`), so unlike `List`/`Map` this specific guard
    // never had a real shadowing gap to close — converted for consistency
    // with every other static-constructor branch in this function.
    if let ExprKind::Ident(id) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if (*ns == INT || *ns == FLOAT) && op == "parse"
        )
        && args.len() == 1
    {
        let s = pre.lower_node(&args[0], cx);
        return Some(parse_iife(id.name == INT, s));
    }
    // v0.86 (ADR 0112): `Duration.millis(n)` — the runtime `Int`→`Duration`
    // constructor. A `Duration` lowers to its milliseconds, so this is the
    // identity on the argument.
    //
    // P6.21 (partial, continued): reads `Callee::Intrinsic` instead of
    // `id.name == DURATION` — see the `List`/`Map` branch above for why this
    // closes a real shadowing gap the bare name check left open (the checker
    // itself guards `ctx.lookup(DURATION).is_none()` before ever recording
    // this `Callee`; the same applies to `Instant`/`Bytes`/`Stream` below).
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == DURATION && op == "millis"
        )
        && args.len() == 1
    {
        return Some(built_of(pre.lower_node(&args[0], cx)));
    }
    // K3 (#2592): `Instant.fromIso(s)`, a guarded RFC 3339 read.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == INSTANT && op == "fromIso"
        )
        && args.len() == 1
    {
        let s = pre.lower_node(&args[0], cx);
        cx.note_time_ops();
        return Some(runtime_ctor_call(None, "__bynkInstantFromIso", s));
    }
    // v0.90 (ADR 0114): `Instant.fromEpochMillis(n)` — an `Instant` lowers to
    // its epoch milliseconds, so this is the identity on the argument.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == INSTANT && op == "fromEpochMillis"
        )
        && args.len() == 1
    {
        return Some(built_of(pre.lower_node(&args[0], cx)));
    }
    None
}

/// Lower the statics that build a runtime value (`Bytes.*`, `Decimal.*`, `TokenClaims.of`, `RequestHeaders.*`, `RequestQuery.*`, `Stream.of`), or `None` when the call is none of them. Moved out of
/// `lower_method_call` (#3167); builds a node when its arguments are nodes (1A.20, #3721).
pub(in crate::emitter) fn lower_runtime_value_static_call(
    e: &Expr,
    receiver: &Expr,
    method: &Ident,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<Built> {
    // v0.110 (ADR 0142 D2): the `Bytes` static constructors. `fromUtf8` is the
    // UTF-8 encoding of a string (total); `fromBase64` is a guarded base64
    // decode returning `Option` (`None` on invalid base64); `empty` is the
    // zero octet sequence.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == BYTES)
    {
        match (method.name.as_str(), args.len()) {
            ("fromUtf8", 1) => {
                let s = pre.lower_node(&args[0], cx);
                return Some(node_or_text(
                    vec![s],
                    |c| TsExpr::Call {
                        callee: Box::new(member(new(global("TextEncoder"), vec![]), "encode")),
                        args: c,
                    },
                    |c| format!("new globalThis.TextEncoder().encode({})", c.join(", ")),
                ));
            }
            ("fromBase64", 1) => {
                let s = pre.lower_node(&args[0], cx);
                cx.note_bytes();
                return Some(runtime_ctor_call(None, "__bynkBytesFromBase64", s));
            }
            // #2016: the guarded hex and base64url decodes, `None` on bad input.
            ("fromHex", 1) => {
                let s = pre.lower_node(&args[0], cx);
                cx.note_bytes_ops();
                return Some(runtime_ctor_call(None, "__bynkBytesFromHex", s));
            }
            ("fromBase64Url", 1) => {
                let s = pre.lower_node(&args[0], cx);
                cx.note_bytes_ops();
                return Some(runtime_ctor_call(None, "__bynkBytesFromBase64Url", s));
            }
            ("empty", 0) => {
                return Some(Ok(new(global("Uint8Array"), vec![])));
            }
            _ => {}
        }
    }
    // #2622: `Decimal.parse(s)` and `Decimal.fromInt(n)`.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == DECIMAL)
    {
        match (method.name.as_str(), args.len()) {
            ("parse", 1) => {
                let s = pre.lower_node(&args[0], cx);
                cx.note_decimal("__bynkDecimalParse");
                return Some(runtime_ctor_call(None, "__bynkDecimalParse", s));
            }
            ("fromInt", 1) => {
                let n = pre.lower_node(&args[0], cx);
                cx.note_decimal("__bynkDecimalFromInt");
                return Some(runtime_ctor_call(None, "__bynkDecimalFromInt", n));
            }
            _ => {}
        }
    }
    // #2380 (L5): `TokenClaims.of(lines)` builds the runtime's claims view.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, op }) if *ns == TOKEN_CLAIMS && op == "of")
        && args.len() == 1
    {
        let xs = pre.lower_node(&args[0], cx);
        cx.note_token_claims();
        return Some(runtime_ctor_call(None, "__tokenClaims", xs));
    }
    // #2341 (L4): `RequestHeaders.of(lines)` / `RequestHeaders.empty()` build
    // the runtime's headers view from `"name: value"` lines.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == REQUEST_HEADERS)
    {
        match (method.name.as_str(), args.len()) {
            ("of", 1) => {
                let xs = pre.lower_node(&args[0], cx);
                cx.note_request_headers();
                return Some(runtime_ctor_call(None, "__requestHeaders", xs));
            }
            ("empty", 0) => {
                cx.note_request_headers();
                return Some(Ok(call("__requestHeaders", vec![TsExpr::array(vec![])])));
            }
            _ => {}
        }
    }
    // #2854: `RequestQuery.of(pairs)` / `RequestQuery.empty()` build the
    // runtime's query view from `"name=value"` pairs.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == REQUEST_QUERY)
    {
        match (method.name.as_str(), args.len()) {
            ("of", 1) => {
                let xs = pre.lower_node(&args[0], cx);
                cx.note_request_query();
                return Some(runtime_ctor_call(None, "__requestQuery", xs));
            }
            ("empty", 0) => {
                cx.note_request_query();
                return Some(Ok(call("__requestQuery", vec![TsExpr::array(vec![])])));
            }
            _ => {}
        }
    }
    // #3277: `RequestCookies.of(pairs)` / `RequestCookies.empty()` build the
    // runtime's cookies view from `"name=value"` pairs.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(cx.commons().callee(e.id), Some(Callee::Intrinsic { ns, .. }) if *ns == REQUEST_COOKIES)
    {
        match (method.name.as_str(), args.len()) {
            ("of", 1) => {
                let xs = pre.lower_node(&args[0], cx);
                cx.note_request_cookies();
                return Some(runtime_ctor_call(None, "__requestCookies", xs));
            }
            ("empty", 0) => {
                cx.note_request_cookies();
                return Some(Ok(call("__requestCookies", vec![TsExpr::array(vec![])])));
            }
            _ => {}
        }
    }
    // v0.100: `Stream.of(xs)` — the deterministic in-memory source. A `Stream`
    // lowers to a host async iterable; `of` wraps a list as an async generator.
    // Emitted inline (no runtime import), like the collection kernels.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic { ns, op }) if *ns == STREAM && op == "of"
        )
        && args.len() == 1
    {
        let xs = pre.lower(&args[0], cx);
        // 1A: generator expression has no bynk_ts node
        return Some(Err(format!(
            "(async function* () {{ for (const __e of {xs}) {{ yield __e; }} }})()"
        )));
    }
    None
}
