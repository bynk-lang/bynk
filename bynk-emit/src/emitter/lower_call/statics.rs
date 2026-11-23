//! The intrinsic and type statics of a method call: `HttpResult.*`, `List`/`Map`/`Set`
//! `empty`, `Set.of`, `Float.pi`/`e`, the numeric parses, `Duration.millis`, `Instant.from*`,
//! the runtime-value statics (`Bytes.*`, `Decimal.*`, ...) and a static call on a declared type.
//! Moved out of `lower.rs` (back-half paydown 1A.20, #3721, Decision E): the first of the
//! `lower_call/` files the dispatch run (1A.20 to 1A.30) fills.

use bynk_check::checker::{Callee, Ty};
use bynk_check::kernel_op::{
    BytesStatic, DecimalStatic, DurationStatic, FloatStatic, InstantStatic, IntStatic, IntrinsicNs,
    ListStatic, MapStatic, RequestCookiesStatic, RequestHeadersStatic, RequestQueryStatic,
    SetStatic, StaticOp, StreamStatic, TokenClaimsStatic,
};
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsExpr, TsLit, TsParam, TsStmt, TsType, TsUnaryOp,
};

use super::super::lower::{
    Pre, lower_http_variant_args, member_node, runtime_ctor_call, settle_built,
};
use super::super::lower_ctx::LowerCtx;
use super::super::lower_list::{key_is_plain, take};
use super::super::ts_types::{ts_ty_to_ts_type, ts_type_ref_to_ts_type};
use super::super::{Expr, ExprKind, Ident, TypeRef};
use super::codec_statics::lower_json_codec_call;
use super::kernel_scalar::identity;
use super::query::stream_generator;
use crate::ts_build::{call, call as call_built, ident, member, new, type_app};

/// `globalThis.<name>`.
fn global(name: &str) -> TsExpr {
    member_node("globalThis", name)
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
) -> Option<bynk_ts::TsExpr> {
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
        let mut all: Vec<bynk_ts::TsExpr> = args.iter().map(|a| pre.lower_node(a, cx)).collect();
        // #2719: a static method in the needs-context set takes the context last.
        let mut needs_exec = false;
        if let Some(Callee::Static(decl, witnesses)) = cx.commons().callees.get(&e.id) {
            // Constrained generics slice 4: the call's witnesses follow the
            // ordinary arguments, as for an instance method.
            all.extend(cx.witness_args(decl, witnesses));
            if cx.commons().exec_plan.fn_needs(decl) {
                all.push(cx.exec_node());
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
fn parse_iife(is_int: bool, s: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    let (grammar, value, guard_fn) = if is_int {
        (
            INT_GRAMMAR,
            TsExpr::Binary {
                op: TsBinaryOp::Add,
                left: Box::new(TsExpr::Call {
                    callee: Box::new(global("Number")),
                    args: vec![ident("__s")],
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
                args: vec![ident("__s")],
            },
            "isFinite",
        )
    };
    {
        let c = vec![s];
        let s = c.into_iter().next().expect("one child");
        let matches = TsExpr::Call {
            callee: Box::new(member(TsExpr::Lit(TsLit::Raw(grammar.to_string())), "test")),
            args: vec![ident("__s")],
        };
        let guard = TsExpr::Call {
            callee: Box::new(member(member_node("globalThis", "Number"), guard_fn)),
            args: vec![ident("__n")],
        };
        let body = vec![
            TsStmt::if_stmt(
                TsExpr::Unary {
                    op: TsUnaryOp::Not,
                    expr: Box::new(matches),
                },
                TsStmt::return_stmt(Some(ident("None")), None),
                None,
            ),
            TsStmt::const_stmt(TsBindingName::Ident("__n".to_string()), None, value, None),
            TsStmt::return_stmt(
                Some(TsExpr::Conditional {
                    test: Box::new(guard),
                    consequent: Box::new(call(ident("Some"), vec![ident("__n")])),
                    alternate: Box::new(ident("None")),
                }),
                None,
            ),
        ];
        let arrow = TsExpr::Arrow {
            params: vec![TsParam::new(
                "__s".to_string(),
                Some(TsType::named("string")),
            )],
            is_async: false,
            generics: vec![],
            return_type: None,
            body: Box::new(TsArrowBody::Block(body)),
        };
        TsExpr::Call {
            callee: Box::new(TsExpr::Paren(Box::new(arrow))),
            args: vec![s],
        }
    }
}

/// `Set.of(xs)` over a plain key is `new globalThis.Set<T>(xs)`; over a keyed one
/// the runtime's `__bynkSetOf<T>(xs)`.
fn set_of(plain: bool, elem: TsType, xs: bynk_ts::TsExpr) -> bynk_ts::TsExpr {
    if plain {
        new(type_app(global("Set"), vec![elem]), vec![xs])
    } else {
        TsExpr::Call {
            callee: Box::new(type_app(ident("__bynkSetOf"), vec![elem])),
            args: vec![xs],
        }
    }
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
) -> Option<bynk_ts::TsExpr> {
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
            Some(Callee::Intrinsic {
                ns: IntrinsicNs::HttpResult,
                ..
            })
        )
    {
        let args_lowered = lower_http_variant_args(e, args, pre, cx);
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
            Some(Callee::Intrinsic {
                op: StaticOp::List(ListStatic::Empty)
                    | StaticOp::Map(MapStatic::Empty)
                    | StaticOp::Set(SetStatic::Empty),
                ..
            })
        )
        && args.is_empty()
    {
        match cx.commons().expr_ty(e.id).as_deref() {
            // K6a (#2596).
            Some(Ty::Set(t)) => {
                return Some(new(
                    type_app(global("Set"), vec![ts_ty_to_ts_type(*t, tys)]),
                    vec![],
                ));
            }
            Some(Ty::List(t)) => {
                // `([] as readonly T[])`: the bracket keeps the cast one operand.
                return Some(TsExpr::Paren(Box::new(TsExpr::As {
                    expr: Box::new(TsExpr::array(vec![])),
                    ty: TsType::readonly_array(ts_ty_to_ts_type(*t, tys)),
                })));
            }
            Some(Ty::Map(k, v)) => {
                return Some(new(
                    type_app(
                        global("Map"),
                        vec![ts_ty_to_ts_type(*k, tys), ts_ty_to_ts_type(*v, tys)],
                    ),
                    vec![],
                ));
            }
            _ => {}
        }
    }
    // K6a (#2596): `Set.of(xs)` — a set of the list's elements; the first
    // occurrence keeps its position.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic {
                op: StaticOp::Set(SetStatic::Of),
                ..
            })
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
    // The codec's decoders and encoders are their own family (1A.21), and a node they return
    // passes through.
    if let Some(built) = lower_json_codec_call(e, receiver, args, cx, pre) {
        return Some(built);
    }
    // K6b (#2598): `Float.pi()` and `Float.e()`.
    if let ExprKind::Ident(_) = &receiver.kind
        && let Some(Callee::Intrinsic {
            op: StaticOp::Float(constant @ (FloatStatic::Pi | FloatStatic::E)),
            ..
        }) = cx.commons().callee(e.id)
        && args.is_empty()
    {
        let constant = if *constant == FloatStatic::Pi {
            "PI"
        } else {
            "E"
        };
        return Some(member(member_node("globalThis", "Math"), constant));
    }
    // v0.22a: the numeric parse statics — `Int.parse(s)` / `Float.parse(s)`
    // (ADR 0048; the grammar is on [`parse_iife`]).
    // P6.21 (partial, continued): reads `Callee::Intrinsic` instead of
    // `id.name == INT || id.name == FLOAT` — see the `List`/`Map` branch
    // above. `Int`/`Float` are lexically reserved (the parser only admits
    // them here followed by `.`), so unlike `List`/`Map` this specific guard
    // never had a real shadowing gap to close — converted for consistency
    // with every other static-constructor branch in this function.
    if let ExprKind::Ident(_) = &receiver.kind
        && let Some(Callee::Intrinsic {
            op: parse @ (StaticOp::Int(IntStatic::Parse) | StaticOp::Float(FloatStatic::Parse)),
            ..
        }) = cx.commons().callee(e.id)
        && args.len() == 1
    {
        let is_int = matches!(parse, StaticOp::Int(_));
        let s = pre.lower_node(&args[0], cx);
        return Some(parse_iife(is_int, s));
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
            Some(Callee::Intrinsic {
                op: StaticOp::Duration(DurationStatic::Millis),
                ..
            })
        )
        && args.len() == 1
    {
        return Some(identity(pre, &args[0], cx));
    }
    // K3 (#2592): `Instant.fromIso(s)`, a guarded RFC 3339 read.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic {
                op: StaticOp::Instant(InstantStatic::FromIso),
                ..
            })
        )
        && args.len() == 1
    {
        let s = pre.lower_node(&args[0], cx);
        return Some(runtime_ctor_call(None, "__bynkInstantFromIso", s));
    }
    // v0.90 (ADR 0114): `Instant.fromEpochMillis(n)` — an `Instant` lowers to
    // its epoch milliseconds, so this is the identity on the argument.
    if let ExprKind::Ident(_) = &receiver.kind
        && matches!(
            cx.commons().callee(e.id),
            Some(Callee::Intrinsic {
                op: StaticOp::Instant(InstantStatic::FromEpochMillis),
                ..
            })
        )
        && args.len() == 1
    {
        return Some(identity(pre, &args[0], cx));
    }
    None
}

/// Lower the statics that build a runtime value (`Bytes.*`, `Decimal.*`, `TokenClaims.of`, `RequestHeaders.*`, `RequestQuery.*`, `Stream.of`), or `None` when the call is none of them. Moved out of
/// `lower_method_call` (#3167); builds a node when its arguments are nodes (1A.20, #3721).
pub(in crate::emitter) fn lower_runtime_value_static_call(
    e: &Expr,
    receiver: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<bynk_ts::TsExpr> {
    // The operation the checker resolved for this call (1A.33): one match over the
    // closed `StaticOp`, not a name comparison per family. The arity stays checked here, as the
    // checker only reports a mismatch.
    let ExprKind::Ident(_) = &receiver.kind else {
        return None;
    };
    let Some(Callee::Intrinsic { op, .. }) = cx.commons().callee(e.id) else {
        return None;
    };
    let op = *op;
    match (op, args.len()) {
        // v0.110 (ADR 0142 D2): the `Bytes` static constructors. `fromUtf8` is the
        // UTF-8 encoding of a string (total); `fromBase64` is a guarded base64
        // decode returning `Option` (`None` on invalid base64); `empty` is the
        // zero octet sequence.
        (StaticOp::Bytes(BytesStatic::FromUtf8), 1) => {
            let s = pre.lower_node(&args[0], cx);
            Some({
                let c = vec![s];
                TsExpr::Call {
                    callee: Box::new(member(new(global("TextEncoder"), vec![]), "encode")),
                    args: c,
                }
            })
        }
        (StaticOp::Bytes(BytesStatic::FromBase64), 1) => {
            let s = pre.lower_node(&args[0], cx);
            cx.note_bytes();
            Some(runtime_ctor_call(None, "__bynkBytesFromBase64", s))
        }
        // #2016: the guarded hex and base64url decodes, `None` on bad input.
        (StaticOp::Bytes(BytesStatic::FromHex), 1) => {
            let s = pre.lower_node(&args[0], cx);
            Some(runtime_ctor_call(None, "__bynkBytesFromHex", s))
        }
        (StaticOp::Bytes(BytesStatic::FromBase64Url), 1) => {
            let s = pre.lower_node(&args[0], cx);
            Some(runtime_ctor_call(None, "__bynkBytesFromBase64Url", s))
        }
        (StaticOp::Bytes(BytesStatic::Empty), 0) => Some(new(global("Uint8Array"), vec![])),
        // #2622: `Decimal.parse(s)` and `Decimal.fromInt(n)`.
        (StaticOp::Decimal(DecimalStatic::Parse), 1) => {
            let s = pre.lower_node(&args[0], cx);
            cx.note_decimal("__bynkDecimalParse");
            Some(runtime_ctor_call(None, "__bynkDecimalParse", s))
        }
        (StaticOp::Decimal(DecimalStatic::FromInt), 1) => {
            let n = pre.lower_node(&args[0], cx);
            cx.note_decimal("__bynkDecimalFromInt");
            Some(runtime_ctor_call(None, "__bynkDecimalFromInt", n))
        }
        // #2380 (L5): `TokenClaims.of(lines)` builds the runtime's claims view.
        (StaticOp::TokenClaims(TokenClaimsStatic::Of), 1) => {
            let xs = pre.lower_node(&args[0], cx);
            Some(runtime_ctor_call(None, "__tokenClaims", xs))
        }
        // #2341 (L4): `RequestHeaders.of(lines)` / `RequestHeaders.empty()` build
        // the runtime's headers view from `"name: value"` lines.
        (StaticOp::RequestHeaders(RequestHeadersStatic::Of), 1) => {
            let xs = pre.lower_node(&args[0], cx);
            Some(runtime_ctor_call(None, "__requestHeaders", xs))
        }
        (StaticOp::RequestHeaders(RequestHeadersStatic::Empty), 0) => {
            Some(call(ident("__requestHeaders"), vec![TsExpr::array(vec![])]))
        }
        // #2854: `RequestQuery.of(pairs)` / `RequestQuery.empty()` build the
        // runtime's query view from `"name=value"` pairs.
        (StaticOp::RequestQuery(RequestQueryStatic::Of), 1) => {
            let xs = pre.lower_node(&args[0], cx);
            Some(runtime_ctor_call(None, "__requestQuery", xs))
        }
        (StaticOp::RequestQuery(RequestQueryStatic::Empty), 0) => {
            Some(call(ident("__requestQuery"), vec![TsExpr::array(vec![])]))
        }
        // #3277: `RequestCookies.of(pairs)` / `RequestCookies.empty()` build the
        // runtime's cookies view from `"name=value"` pairs.
        (StaticOp::RequestCookies(RequestCookiesStatic::Of), 1) => {
            let xs = pre.lower_node(&args[0], cx);
            Some(runtime_ctor_call(None, "__requestCookies", xs))
        }
        (StaticOp::RequestCookies(RequestCookiesStatic::Empty), 0) => {
            Some(call(ident("__requestCookies"), vec![TsExpr::array(vec![])]))
        }
        // v0.100: `Stream.of(xs)` — the deterministic in-memory source. A `Stream`
        // lowers to a host async iterable; `of` wraps a list as an async generator.
        // Emitted inline (no runtime import), like the collection kernels.
        (StaticOp::Stream(StreamStatic::Of), 1) => {
            let xs = pre.lower_node(&args[0], cx);
            Some({
                let n = vec![xs];
                let [xs] = take::<1, _>(n);
                // `(async function* () { for (const __e of xs) { yield __e; } })()`
                let each = TsStmt::for_of(
                    "__e",
                    xs,
                    TsStmt::block(
                        vec![TsStmt::expr_stmt(
                            TsExpr::Yield(Box::new(ident("__e"))),
                            None,
                        )],
                        None,
                    ),
                    None,
                );
                stream_generator(None, vec![each])
            })
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::emit_source;

    /// The emitted TypeScript for a one-function commons.
    fn emitted(body: &str) -> String {
        emit_source(&format!("commons st\n{body}\n"))
    }

    /// The returned expression of `export function f`.
    fn returned(ts: &str) -> String {
        let at = ts.find("export function f(").expect("f");
        let rest = &ts[at..];
        let ret = rest.find("  return ").expect("a return") + "  return ".len();
        let end = rest[ret..].find(";\n}\n").expect("end");
        rest[ret..ret + end].to_string()
    }

    const INT_RE: &str = "/^[+-]?[0-9]+$/";
    const FLOAT_RE: &str = r"/^[+-]?([0-9]+(\.[0-9]*)?|\.[0-9]+)([eE][+-]?[0-9]+)?$/";

    fn parse_text(re: &str, value: &str, guard: &str, arg: &str) -> String {
        format!(
            "((__s: string) => {{ if (!{re}.test(__s)) return None; const __n = {value}; return {guard} ? Some(__n) : None; }})({arg})"
        )
    }

    /// Each static over identifier arguments prints exactly today's text and
    /// leaves no expression as text (1A.4's counter reads 0).
    #[test]
    fn statics_over_nodes_print_as_before() {
        let int_parse = parse_text(
            INT_RE,
            "globalThis.Number(__s) + 0",
            "globalThis.Number.isSafeInteger(__n)",
            "s",
        );
        let float_parse = parse_text(
            FLOAT_RE,
            "globalThis.Number(__s)",
            "globalThis.Number.isFinite(__n)",
            "s",
        );
        for (body, want) in [
            (
                "fn f() -> List[Int] { List.empty() }",
                "([] as readonly number[])".to_string(),
            ),
            (
                "fn f() -> Set[Int] { Set.empty() }",
                "new globalThis.Set<number>()".to_string(),
            ),
            (
                "fn f() -> Map[String, Int] { Map.empty() }",
                "new globalThis.Map<string, number>()".to_string(),
            ),
            (
                "fn f() -> Float { Float.pi() }",
                "globalThis.Math.PI".to_string(),
            ),
            (
                "fn f() -> Float { Float.e() }",
                "globalThis.Math.E".to_string(),
            ),
            ("fn f(s: String) -> Option[Int] { Int.parse(s) }", int_parse),
            (
                "fn f(s: String) -> Option[Float] { Float.parse(s) }",
                float_parse,
            ),
            (
                "fn f(n: Int) -> Duration { Duration.millis(n) }",
                "n".to_string(),
            ),
            (
                "fn f(n: Int) -> Instant { Instant.fromEpochMillis(n) }",
                "n".to_string(),
            ),
            (
                "fn f(s: String) -> Option[Instant] { Instant.fromIso(s) }",
                "__bynkInstantFromIso(s)".to_string(),
            ),
            (
                "fn f(s: String) -> Bytes { Bytes.fromUtf8(s) }",
                "new globalThis.TextEncoder().encode(s)".to_string(),
            ),
            (
                "fn f() -> Bytes { Bytes.empty() }",
                "new globalThis.Uint8Array()".to_string(),
            ),
            (
                "fn f(s: String) -> Option[Bytes] { Bytes.fromBase64(s) }",
                "__bynkBytesFromBase64(s)".to_string(),
            ),
            (
                "fn f(s: String) -> Option[Bytes] { Bytes.fromHex(s) }",
                "__bynkBytesFromHex(s)".to_string(),
            ),
            (
                "fn f(s: String) -> Option[Bytes] { Bytes.fromBase64Url(s) }",
                "__bynkBytesFromBase64Url(s)".to_string(),
            ),
            (
                "fn f(s: String) -> Option[Decimal] { Decimal.parse(s) }",
                "__bynkDecimalParse(s)".to_string(),
            ),
            (
                "fn f(n: Int) -> Decimal { Decimal.fromInt(n) }",
                "__bynkDecimalFromInt(n)".to_string(),
            ),
        ] {
            let ts = emitted(body);
            assert_eq!(returned(&ts), want, "{body}");
        }
    }

    /// `Set.of` over a plain key is a `Set` constructor; over a keyed one it
    /// goes through `__bynkSetOf`, which is imported on both the node and the
    /// text path.
    #[test]
    fn set_of_notes_its_key_helper_only_for_a_non_plain_key() {
        let ts = emitted("fn f(xs: List[Int]) -> Set[Int] { Set.of(xs) }");
        assert_eq!(returned(&ts), "new globalThis.Set<number>(xs)");
        assert!(!ts.contains("__bynkSetOf"), "{ts}");

        let ts = emitted("fn f(xs: List[Bytes]) -> Set[Bytes] { Set.of(xs) }");
        assert_eq!(returned(&ts), "__bynkSetOf<globalThis.Uint8Array>(xs)");
        assert!(ts.contains("__bynkSetOf"), "{ts}");
    }

    /// The identity statics keep the hoists of their argument: the `?` temporary
    /// still reaches the statement list, and the argument stays a node.
    #[test]
    fn an_identity_static_keeps_its_arguments_hoists() {
        let ts = emitted(
            "fn g() -> Result[Int, String] { Ok(3) }\n\
             fn f() -> Result[Duration, String] { Ok(Duration.millis(g()?)) }",
        );
        assert!(ts.contains("const __r0 = g();"), "{ts}");
        assert!(ts.contains("return Ok(__r0.value);"), "{ts}");
    }

    /// `Stream.of` is a node (1A.35a part B): an async generator over a `for…of` and a `yield`,
    /// laid out across lines, with every token of the one-line text it replaced.
    #[test]
    fn stream_of_is_an_async_generator_node() {
        let ts = emitted("fn f(xs: List[String]) -> Stream[String] { Stream.of(xs) }");
        assert_eq!(
            returned(&ts),
            "(async function* () {\n    for (const __e of xs) {\n      yield __e;\n    }\n  })()"
        );
    }

    /// A static on a declared type: with and without explicit type arguments,
    /// and with a witness.
    #[test]
    fn type_statics_print_with_type_arguments_and_witnesses() {
        let box_ = "type Box[T] = { value: T }\n\
                    fn Box.of(value: T) -> Box[T] { Box { value } }\n\
                    fn Box.empty() -> Box[Option[T]] { Box { value: None } }\n";
        for (f, want) in [
            ("fn f(n: Int) -> Box[Int] { Box.of(n) }", "Box.of(n)"),
            (
                "fn f() -> Box[Option[Int]] { Box.empty[Int]() }",
                "Box.empty<number>()",
            ),
            (
                "fn f() -> Box[Option[List[Int]]] { Box.empty[List[Int]]() }",
                "Box.empty<readonly number[]>()",
            ),
        ] {
            let ts = emitted(&format!("{box_}{f}"));
            assert_eq!(returned(&ts), want, "{f}");
        }
        // A comparator witness follows the arguments.
        let ts = emitted(
            "type Sorted[T: Ord] = { items: List[T] }\n\
             fn Sorted.empty() -> Sorted[T] { Sorted { items: [] } }\n\
             fn f() -> Sorted[Int] { Sorted.empty() }",
        );
        assert_eq!(returned(&ts), "Sorted.empty(__bynkCmpInt)");
    }

    /// A UFCS call with explicit type arguments: the receiver's own arguments
    /// first (spelled when in scope everywhere, `any` otherwise), then the
    /// explicit ones, printed as `ts_type_ref` prints them.
    #[test]
    fn ufcs_explicit_type_arguments_are_a_type_application() {
        let head = "type Box[A] = { value: A }\ntype Item = { sku: String }\n\
                    fn Box.probe[U](self) -> Bool { true }\n";
        for (f, want) in [
            (
                "fn f(b: Box[Int]) -> Bool { b.probe[String]() }",
                "Box.probe<number, string>(b)",
            ),
            (
                "fn f(b: Box[Item]) -> Bool { b.probe[Item]() }",
                "Box.probe<any, Item>(b)",
            ),
            (
                "fn f(b: Box[List[Int]]) -> Bool { b.probe[Option[Int]]() }",
                "Box.probe<readonly number[], Option<number>>(b)",
            ),
        ] {
            let ts = emitted(&format!("{head}{f}"));
            assert_eq!(returned(&ts), want, "{f}");
        }
    }

    /// Decision G, the pass-through: a delegated kernel reaches the call's caller as
    /// the print it has (one call per kernel family). The `Map` and `Set` arms are
    /// nodes since 1A.30, so their receivers print without the text form's brackets.
    #[test]
    fn delegated_kernels_print_as_before() {
        for (body, want) in [
            ("fn f(m: Map[String, Int]) -> Int { m.length() }", "m.size"),
            ("fn f(s: Set[Int]) -> Bool { s.contains(1) }", "s.has(1)"),
            ("fn f(n: Int) -> Int { n.abs() }", "globalThis.Math.abs(n)"),
            (
                "fn f(x: Float) -> Int { x.floor() }",
                "__bynkToInt(globalThis.Math.floor(x), \"floor\")",
            ),
            ("fn f(d: Duration) -> Int { d.toMillis() }", "d"),
            ("fn f(i: Instant) -> Int { i.toEpochMillis() }", "i"),
            ("fn f(b: Bytes) -> Int { b.length() }", "b.length"),
            (
                "fn f(d: Decimal) -> String { d.toString() }",
                "__bynkDecimalToString(d)",
            ),
            ("fn f(s: String) -> Int { s.length() }", "s.length"),
            (
                "fn f(o: Option[Int]) -> Int { o.getOrElse(0) }",
                "((__o: Option<number>, __d: number) => __o.tag === \"Some\" ? __o.value : __d)(o, 0)",
            ),
            (
                "fn f(r: Result[Int, String]) -> Int { r.getOrElse(0) }",
                "((__r: Result<number, string>, __d: number) => __r.tag === \"Ok\" ? __r.value : __d)(r, 0)",
            ),
        ] {
            let ts = emitted(body);
            assert_eq!(returned(&ts), want, "{body}");
        }
    }
}
