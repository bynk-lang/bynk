//! A value at a boundary (back-half paydown 2.3a, #4088): `serialise_expr`, `deserialise_expr` and their
//! `_via`, reference and witness forms, with their tests and the serialiser corpus. Moved out of
//! `serialisation.rs` byte for byte.

use bynk_syntax::ast::{BaseType, TypeRef};
use bynk_ts::{TsArrowBody, TsBinaryOp, TsExpr, TsParam, TsType, TsUnaryOp};

use crate::emitter::RuntimeUse;
use crate::ts_build::{as_expr, call, ident, member, ret, str_lit};

use super::field::{emit_field_deserialise, serialise_field_expr, serialise_field_expr_via};
use super::inner_ts_name;

/// v0.22b: an expression-form serialise for a codec target — the same
/// dispatch as a record field's serialisation.
pub(crate) fn serialise_expr(t: &TypeRef, value: &TsExpr, ru: &RuntimeUse) -> TsExpr {
    serialise_field_expr(t, value, None, ru)
}

/// v0.176 (#642): the one serialise dispatch for the workers cross-context
/// boundary, reaching helpers through `ns`. Replaces the two parallel
/// dispatches this boundary used to carry — `emit.rs`'s `workers_serialise_expr`
/// (which dropped `List`/`Map` to a bare `as JsonValue` cast) and
/// `workers_entry.rs`'s `serialise_call` (which did the same to `Bytes`, the
/// asymmetry that forced `bynk.types.bytes_at_workers_boundary`).
pub fn serialise_expr_via(t: &TypeRef, value: &TsExpr, ns: &str, ru: &RuntimeUse) -> TsExpr {
    serialise_field_expr_via(t, value, ns, ru)
}

/// v0.176 (#642): a deserialise **reference** for `ns`, shaped to
/// `callService`'s `deserialiseResult` parameter. The inline arms become a
/// lambda rather than the unvalidated `((j: any) => ({ tag: "Ok", value: j }))`
/// identity the caller path used to fall back to.
pub fn deserialise_ref_via(t: &TypeRef, ns: &str, ru: &RuntimeUse) -> TsExpr {
    match strip_effect(t) {
        TypeRef::Named(id) => ident(format!(
            "{ns}__deserialise_{}",
            bynk_check::emitted_names::codec_key(&id.name)
        )),
        t @ (TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::List(..)
        | TypeRef::Set(..)
        | TypeRef::Map(..)
        | TypeRef::App { .. }) => ident(format!("{ns}__deserialise_{}", inner_ts_name(t))),
        other @ TypeRef::Base(..)
        | other @ TypeRef::Effect(..)
        | other @ TypeRef::HttpResult(..)
        | other @ TypeRef::QueueResult(_)
        | other @ TypeRef::Asset(_)
        | other @ TypeRef::RequestHeaders(_)
        | other @ TypeRef::RequestQuery(_)
        | other @ TypeRef::RequestCookies(_)
        | other @ TypeRef::TokenClaims(_)
        | other @ TypeRef::BoundaryRejection(_)
        | other @ TypeRef::Query(..)
        | other @ TypeRef::Stream(..)
        | other @ TypeRef::Connection(..)
        | other @ TypeRef::History(..)
        | other @ TypeRef::ValidationError(_)
        | other @ TypeRef::DecodeError(_)
        | other @ TypeRef::Unit(_)
        | other @ TypeRef::Fn(..) => TsExpr::Arrow {
            params: vec![TsParam::new(
                "__j".to_string(),
                Some(TsType::named("__JsonValue")),
            )],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(deserialise_expr_via(
                other,
                &ident("__j"),
                "$",
                ns,
                ru,
            )))),
        },
    }
}

/// v0.176 (#642) follow-up: a serialise **reference** for `ns`, shaped to
/// `httpResultToResponse`'s serialiser parameter — the mirror image of
/// `deserialise_ref_via` above. Replaces `workers_entry.rs`'s
/// `http_value_serialiser`, a parallel dispatch that collapsed every base type
/// to `(v: any) => v as JsonValue`, dropping the `Float` non-finite guard and
/// `Bytes` base64 encoding that `serialise_field_expr_via` already carries.
pub(crate) fn serialise_ref_via(t: &TypeRef, ns: &str, ru: &RuntimeUse) -> TsExpr {
    match strip_effect(t) {
        TypeRef::Named(id) => ident(format!(
            "{ns}__serialise_{}",
            bynk_check::emitted_names::codec_key(&id.name)
        )),
        t @ (TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::List(..)
        | TypeRef::Set(..)
        | TypeRef::Map(..)
        | TypeRef::App { .. }) => ident(format!("{ns}__serialise_{}", inner_ts_name(t))),
        other @ TypeRef::Base(..)
        | other @ TypeRef::Effect(..)
        | other @ TypeRef::HttpResult(..)
        | other @ TypeRef::QueueResult(_)
        | other @ TypeRef::Asset(_)
        | other @ TypeRef::RequestHeaders(_)
        | other @ TypeRef::RequestQuery(_)
        | other @ TypeRef::RequestCookies(_)
        | other @ TypeRef::TokenClaims(_)
        | other @ TypeRef::BoundaryRejection(_)
        | other @ TypeRef::Query(..)
        | other @ TypeRef::Stream(..)
        | other @ TypeRef::Connection(..)
        | other @ TypeRef::History(..)
        | other @ TypeRef::ValidationError(_)
        | other @ TypeRef::DecodeError(_)
        | other @ TypeRef::Unit(_)
        | other @ TypeRef::Fn(..) => TsExpr::Arrow {
            params: vec![TsParam::new(
                "__v".to_string(),
                Some(crate::emitter::ts_types::ts_type_of(other)),
            )],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(serialise_field_expr_via(
                other,
                &ident("__v"),
                ns,
                ru,
            )))),
        },
    }
}

/// Constrained generics slice 3: the `{ encode, decode }` witness for a
/// variable-free type, from the generated codec for it. A named or container
/// codec takes the path it is called with. A base type has no generated helper,
/// so its decoder is the one a record field or container element uses
/// ([`emit_field_deserialise`]), reporting at the path it is given: that is what
/// every position nested in a container decodes with.
pub(crate) fn witness_codec_expr(t: &TypeRef, ru: &RuntimeUse) -> TsExpr {
    let encode = serialise_ref_via(t, "", ru);
    let decode = match strip_effect(t) {
        TypeRef::Named(_)
        | TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::List(..)
        | TypeRef::Set(..)
        | TypeRef::Map(..)
        | TypeRef::App { .. } => deserialise_ref_via(t, "", ru),
        other @ TypeRef::Base(..)
        | other @ TypeRef::Effect(..)
        | other @ TypeRef::HttpResult(..)
        | other @ TypeRef::QueueResult(_)
        | other @ TypeRef::Asset(_)
        | other @ TypeRef::RequestHeaders(_)
        | other @ TypeRef::RequestQuery(_)
        | other @ TypeRef::RequestCookies(_)
        | other @ TypeRef::TokenClaims(_)
        | other @ TypeRef::BoundaryRejection(_)
        | other @ TypeRef::Query(..)
        | other @ TypeRef::Stream(..)
        | other @ TypeRef::Connection(..)
        | other @ TypeRef::History(..)
        | other @ TypeRef::ValidationError(_)
        | other @ TypeRef::DecodeError(_)
        | other @ TypeRef::Unit(_)
        | other @ TypeRef::Fn(..) => {
            let mut body =
                emit_field_deserialise("v", other, &ident("__j"), &ident("__p"), None, ru);
            body.push(ret(call(
                ident("Ok"),
                vec![as_expr(
                    ident("__v"),
                    crate::emitter::ts_types::ts_type_of(other),
                )],
            )));
            // A flat body: the printer's compact object literal takes no multi-line block, so each
            // statement prints on the one line (1A.39a, Decision E). The tree is the arrow itself, so
            // its parameters and locals bind and the names it uses are read from it.
            TsExpr::Arrow {
                params: vec![
                    TsParam::new("__j".to_string(), Some(TsType::named("__JsonValue"))),
                    TsParam::new("__p".to_string(), Some(TsType::named("string"))),
                ],
                is_async: false,
                generics: Vec::new(),
                return_type: None,
                body: Box::new(TsArrowBody::Flat(body)),
            }
        }
    };
    TsExpr::object(vec![
        ("encode".to_string(), encode),
        ("decode".to_string(), decode),
    ])
}

/// #1678: does an agent-call position carry a value with no wire form, so the
/// workers Durable Object call passes it through unencoded, as it did before
/// #1678? True when the type holds a `Connection`, a function, a `Stream`, a
/// `Query` or a `History` anywhere in its structure (an `Effect` wrapper is
/// peeled, as the codec peels it). Every other type has a codec, including a
/// generic application (`Box[Bytes]`), whose helper the codec closure emits.
pub(crate) fn agent_wire_passes_through(t: &TypeRef) -> bool {
    match t {
        TypeRef::Connection(..)
        | TypeRef::Fn(..)
        | TypeRef::Stream(..)
        | TypeRef::Query(..)
        | TypeRef::History(..) => true,
        TypeRef::Effect(inner, _)
        | TypeRef::Option(inner, _)
        | TypeRef::List(inner, _)
        | TypeRef::Set(inner, _)
        | TypeRef::HttpResult(inner, _, _) => agent_wire_passes_through(inner),
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            agent_wire_passes_through(a) || agent_wire_passes_through(b)
        }
        TypeRef::App { args, .. } => args.iter().any(agent_wire_passes_through),
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_) => false,
    }
}

/// An `Effect[T]` in a handler signature wraps the *handler*, not the wire
/// payload — the caller awaits the Promise, so the codec is `T`'s.
fn strip_effect(t: &TypeRef) -> &TypeRef {
    if let TypeRef::Effect(inner, _) = t {
        strip_effect(inner)
    } else {
        t
    }
}

/// A field's type with one `List` or `Option` wrapper peeled, and which wrapper it was:
/// `(inner, is_list, is_option)`. Any other type is its own `inner` with neither flag.
pub(crate) fn peel_list_option(t: &TypeRef) -> (&TypeRef, bool, bool) {
    match t {
        TypeRef::List(inner, _) => (inner.as_ref(), true, false),
        TypeRef::Option(inner, _) => (inner.as_ref(), false, true),
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::Result(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::Map(..)
        | TypeRef::Set(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..)
        | TypeRef::App { .. } => (t, false, false),
    }
}

/// `HttpResult<T>` over the type `inner` spells: generic with no default type argument, so a bare
/// `HttpResult` in type position is `tsc`'s TS2314.
fn http_result_ty(inner: &TypeRef) -> TsType {
    TsType::named_with_args(
        "HttpResult",
        vec![crate::emitter::ts_types::ts_type_of(inner)],
    )
}

/// v0.22b: an expression-form deserialise call for a codec target. Named
/// types and generic instantiations go through their (module-local)
/// helpers; bases inline the structural check.
pub(crate) fn deserialise_expr(t: &TypeRef, json: &TsExpr, path: &str, ru: &RuntimeUse) -> TsExpr {
    deserialise_expr_via(t, json, path, "", ru)
}

/// v0.176 (#642): the one deserialise dispatch for the workers cross-context
/// boundary, reaching helpers through `ns`. Replaces `workers_entry.rs`'s
/// `deserialise_call`; the `Json.decode` entry (`deserialise_expr`) is the same
/// function with an empty prefix.
///
/// This carries two arms the `Json` codec path never needs, because the
/// checker's codec-domain rule rejects them there but the cross-context
/// boundary admits them: `Unit` (an `on call` may return `Effect[Result[(), E]]`)
/// and the runtime-owned error types.
pub(crate) fn deserialise_expr_via(
    t: &TypeRef,
    json: &TsExpr,
    path: &str,
    ns: &str,
    ru: &RuntimeUse,
) -> TsExpr {
    // The runtime-owned-error-type arms below (`ValidationError`/`DecodeError`/
    // `QueueResult`/`HttpResult`) all share this one shape: `Ok(json as
    // unknown as {ty}) as Result<{ty}, BoundaryError>` — the deserialise-side
    // mirror of `serialise_field_expr_wire`'s own `WireRef::Unchecked` arm
    // (see that arm's own doc for why a nested `As`-under-`As` is safe here:
    // this function's result is never rendered through `render_operand`).
    let ok_unknown_cast = |ty: TsType| -> TsExpr {
        as_expr(
            call(
                ident("Ok"),
                vec![as_expr(
                    as_expr(json.clone(), TsType::named("unknown")),
                    ty.clone(),
                )],
            ),
            TsType::named_with_args("Result", vec![ty, TsType::named("__BoundaryError")]),
        )
    };
    // `Err({ kind: "StructuralMismatch", path, expected, actual } as
    // BoundaryError)` — shared by the `Bytes` arm (below) and the plain
    // `Base` arm further down, the one error shape both build.
    let structural_mismatch = |expected: &str, actual: TsExpr| -> TsExpr {
        call(
            ident("Err"),
            vec![as_expr(
                TsExpr::object(vec![
                    ("kind".to_string(), str_lit("StructuralMismatch")),
                    ("path".to_string(), str_lit(path)),
                    ("expected".to_string(), str_lit(expected)),
                    ("actual".to_string(), actual),
                ]),
                TsType::named("__BoundaryError"),
            )],
        )
    };
    match t {
        TypeRef::Named(id) => call(
            ident(format!("{ns}__deserialise_{}", bynk_check::emitted_names::codec_key(&id.name))),
            vec![json.clone(), str_lit(path)],
        ),
        TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::List(..)
        | TypeRef::Set(..)
        | TypeRef::Map(..)
        // v0.174 (#592): a generic-record instantiation decodes through its
        // monomorphised codec (`deserialise_Paginated_User`).
        | TypeRef::App { .. } => call(
            ident(format!("{ns}__deserialise_{}", inner_ts_name(t))),
            vec![json.clone(), str_lit(path)],
        ),
        TypeRef::Effect(inner, _) => deserialise_expr_via(inner, json, path, ns, ru),
        // A `()` carries no wire content — the wire slot is `null` and the value
        // is `undefined`. Nothing to validate, so `Ok` is the honest answer here
        // rather than an erosion.
        //
        // Reached only by a **bare** `()` in a wire position. A `Result`-wrapped
        // one — `on call () -> Effect[Result[(), E]]`, the common shape — strips
        // its `Effect` and then goes through `deserialise_Result_Unit_E`, whose
        // generated body handles the `Unit` payload itself (`emit_generic_helpers`),
        // so it never lands here. No fixture currently exercises this arm; it is
        // defensive, and saying so is more useful than implying coverage.
        TypeRef::Unit(_) => as_expr(
            call(ident("Ok"), vec![ident("undefined")]),
            TsType::named("Result<void, __BoundaryError>"),
        ),
        // The runtime-owned error types: no generated codec to name, so the
        // deserialised value casts through unchecked — same shape as
        // `emit_field_deserialise_wire`'s `WireRef::Unchecked` arm.
        //
        // #1319: closes the `ts_any` residual — each arm now casts through
        // its own real, exported runtime type instead of sharing one `any`
        // arm, since `t` (the matched `TypeRef` itself) already tells this
        // function precisely which of the four it is; no information is
        // missing here the way it briefly was one layer down at the
        // `WireRef::Unchecked`-with-a-`reason`-field call site. Bridged
        // through `unknown` — the same fix, for the same TS2352 reason, as
        // `emit_field_deserialise_wire`'s own `WireRef::Unchecked` arm (this
        // function's `json` is not always statically a `JsonValue` the way
        // that arm's is, but nothing about a real-vs-real interface cast
        // gets *safer* when the source type is narrower, so the same
        // defensive bridge applies uniformly). No fixture currently reaches
        // this function for one of these four types directly (the field
        // path above is what real code exercises today); named rather than
        // silently assumed correct, the same "not exercised, defensive"
        // precedent this file's own `TypeRef::Unit` arm above already sets.
        TypeRef::ValidationError(_) => ok_unknown_cast(TsType::named("ValidationError")),
        TypeRef::DecodeError(_) => ok_unknown_cast(TsType::named("DecodeError")),
        // Review of #1319/#1320, finding 1: `HttpResult<T>` (`bynk-emit/
        // runtime/src/http.ts:6`) is generic with no default type argument
        // — a bare `HttpResult` in type position is `tsc`'s TS2314, not the
        // `any`-compatible cast the other three residual types get. This
        // arm already holds the element type (`TypeRef::HttpResult(Box<
        // TypeRef>, Span)`, `bynk-syntax/src/ast.rs:2196`), so it builds
        // `HttpResult<T>` through `http_result_ty` — the same type node every
        // other `HttpResult<...>` position in this file already uses —
        // instead of dropping the payload.
        TypeRef::HttpResult(inner, _, _) => ok_unknown_cast(http_result_ty(inner)),
        TypeRef::QueueResult(_) => ok_unknown_cast(TsType::named("QueueResult")),
        // #1883, slice 1a: unreachable in a checked program
        // (`bynk.types.asset_at_boundary`).
        TypeRef::Asset(_) => ok_unknown_cast(TsType::named("{ readonly url: string }")),
        // v0.110 (ADR 0142 D5): a `Bytes` wires as a base64 string; decode it
        // (rejecting a non-string or invalid base64) to a `Uint8Array`.
        TypeRef::Base(base @ (BaseType::Bytes | BaseType::Decimal), _) => {
            let (parse, expected, invalid) = if *base == BaseType::Decimal {
                ru.note_decimal("__bynkDecimalParseWire");
                ("__bynkDecimalParseWire", "decimal string", "invalid decimal")
            } else {
                ru.note_bytes();
                ("__bynkBytesFromBase64", "base64 string", "invalid base64")
            };
            call(
                TsExpr::Arrow {
                    params: vec![TsParam::new("__v".to_string(), None)],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Conditional {
                        test: Box::new(TsExpr::Binary {
                            op: TsBinaryOp::StrictEq,
                            left: Box::new(TsExpr::Unary {
                                op: TsUnaryOp::Typeof,
                                expr: Box::new(ident("__v")),
                            }),
                            right: Box::new(str_lit("string")),
                        }),
                        consequent: Box::new(call(
                            TsExpr::Arrow {
                                params: vec![TsParam::new("__b".to_string(), None)],
                                is_async: false,
                                generics: Vec::new(),
                                return_type: None,
                                body: Box::new(TsArrowBody::Expr(Box::new(
                                    TsExpr::Conditional {
                                        test: Box::new(TsExpr::Binary {
                                            op: TsBinaryOp::StrictEq,
                                            left: Box::new(member(ident("__b"), "tag")),
                                            right: Box::new(str_lit("Some")),
                                        }),
                                        consequent: Box::new(call(
                                            ident("Ok"),
                                            vec![member(ident("__b"), "value")],
                                        )),
                                        alternate: Box::new(structural_mismatch(
                                            expected,
                                            str_lit(invalid),
                                        )),
                                    },
                                ))),
                            },
                            vec![call(ident(parse), vec![ident("__v")])],
                        )),
                        alternate: Box::new(structural_mismatch(
                            expected,
                            TsExpr::Unary {
                                op: TsUnaryOp::Typeof,
                                expr: Box::new(ident("__v")),
                            },
                        )),
                    }))),
                },
                vec![json.clone()],
            )
        }
        TypeRef::Base(b, _) => {
            let typeof_str = match b {
                BaseType::Int => "number",
                BaseType::String => "string",
                BaseType::Bool => "boolean",
                BaseType::Float => "number",
                BaseType::Duration | BaseType::Instant => "number",
                // Unreachable: handled by the dedicated `Bytes`/`Decimal` arm above.
                BaseType::Bytes | BaseType::Decimal => "string",
            };
            // `Some(predicate)`: the arm needs a second, narrower runtime check
            // beyond `typeof` — a real `TsExpr`, not text, now that `extra`'s
            // only past job (interpolating into a hand-built `format!`) is
            // gone.
            let predicate: Option<TsExpr> = match b {
                BaseType::Float => Some(call(
                    member(ident("globalThis.Number"), "isFinite"),
                    vec![ident("__v")],
                )),
                // v0.86 (ADR 0112 D6): a `Duration` is whole milliseconds —
                // reject a non-integer from the wire, as a refined `Int` does.
                // #1657: whole *and* within ±(2^53 − 1), the `Int` domain.
                BaseType::Int | BaseType::Duration | BaseType::Instant => Some(call(
                    member(ident("globalThis.Number"), "isSafeInteger"),
                    vec![ident("__v")],
                )),
                BaseType::String | BaseType::Bool | BaseType::Bytes | BaseType::Decimal => None,
            };
            // v0.176 (#642): report what was *required*, not just the `typeof`
            // that was tested. For the arms carrying a `predicate` the two
            // differ, and reporting the bare `typeof` makes the error useless in
            // exactly the case the predicate exists to catch: a `3.5` for an `Int`
            // would read `expected: "number", actual: "number"`.
            let expected = match b {
                BaseType::Int | BaseType::Duration | BaseType::Instant => "safe integer",
                BaseType::Float => "finite number",
                BaseType::String | BaseType::Bool | BaseType::Bytes | BaseType::Decimal => {
                    typeof_str
                }
            };
            let typeof_v = || TsExpr::Unary {
                op: TsUnaryOp::Typeof,
                expr: Box::new(ident("__v")),
            };
            let Some(predicate) = predicate else {
                return call(
                    TsExpr::Arrow {
                        params: vec![TsParam::new("__v".to_string(), None)],
                        is_async: false,
                        generics: Vec::new(),
                        return_type: None,
                        body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Conditional {
                            test: Box::new(TsExpr::Binary {
                                op: TsBinaryOp::StrictEq,
                                left: Box::new(typeof_v()),
                                right: Box::new(str_lit(typeof_str)),
                            }),
                            consequent: Box::new(call(ident("Ok"), vec![ident("__v")])),
                            alternate: Box::new(structural_mismatch(expected, typeof_v())),
                        }))),
                    },
                    vec![json.clone()],
                );
            };
            // The two failure modes are **not** the same error, and collapsing
            // them is what made both predecessors imprecise in opposite
            // directions. The `Json` path reported `typeof` for both, losing the
            // predicate failure's detail; the workers path reported
            // `String(value)` for both, which echoes an arbitrary caller-supplied
            // value into a 400 response body (an `Int` sent `"hunter2"` reported
            // `actual: "hunter2"`) and violates the ADR 0107 discipline of never
            // reporting the offending value.
            //
            // Split them and both problems go away. A wrong `typeof` reports the
            // `typeof` — the value could be anything, so it is never echoed. A
            // *failed predicate* means the `typeof` already matched, so the value
            // is provably a **number**: `String(__v)` is `"3.5"` for a
            // non-integer `Int`, and provably one of `"NaN"` / `"Infinity"` /
            // `"-Infinity"` for a non-finite `Float` — a closed set. That is
            // strictly more precise than either predecessor, with strictly less
            // exposure.
            call(
                TsExpr::Arrow {
                    params: vec![TsParam::new("__v".to_string(), None)],
                    is_async: false,
                    generics: Vec::new(),
                    return_type: None,
                    body: Box::new(TsArrowBody::Expr(Box::new(TsExpr::Conditional {
                        test: Box::new(TsExpr::Binary {
                            op: TsBinaryOp::StrictNotEq,
                            left: Box::new(typeof_v()),
                            right: Box::new(str_lit(typeof_str)),
                        }),
                        consequent: Box::new(structural_mismatch(expected, typeof_v())),
                        alternate: Box::new(TsExpr::Conditional {
                            test: Box::new(predicate),
                            consequent: Box::new(call(ident("Ok"), vec![ident("__v")])),
                            alternate: Box::new(structural_mismatch(
                                expected,
                                call(ident("String"), vec![ident("__v")]),
                            )),
                        }),
                    }))),
                },
                vec![json.clone()],
            )
        }
        // Everything else is rejected by the checker's codec-domain rule (the
        // `Json` path) or by the boundary rules (the workers path). Shared by
        // three callers, so the message names the type rather than one caller.
        other @ (
            TypeRef::RequestHeaders(_)
            | TypeRef::RequestQuery(_)
            | TypeRef::RequestCookies(_)
            | TypeRef::TokenClaims(_)
            | TypeRef::BoundaryRejection(_)
            | TypeRef::Query(..)
            | TypeRef::Stream(..)
            | TypeRef::Connection(..)
            | TypeRef::History(..)
            | TypeRef::Fn(..)
        ) => unreachable!("non-codable type reached a codec lowering: {other:?}"),
    }
}

/// Review of #1319/#1320, finding 2: no unit test covered any of
/// `deserialise_expr_via`'s own four runtime-owned-error-type arms — this
/// path is not reached by any current fixture (see those arms' own doc),
/// so a fixture-corpus regression would not have caught a text-level
/// mistake here the way it caught the `HttpResult<unknown>` gap one layer
/// down. Pins the exact generated text directly.
#[cfg(test)]
mod deserialise_expr_via_error_type_tests {
    use super::*;
    use bynk_syntax::span::Span;

    fn sp() -> Span {
        Span::new(0, 0)
    }

    #[test]
    fn validation_error_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let t = TypeRef::ValidationError(sp());
        assert_eq!(
            bynk_ts::print_expr(&deserialise_expr(&t, &ident("json"), "path", &ru)),
            "Ok(json as unknown as ValidationError) as Result<ValidationError, __BoundaryError>"
        );
    }

    #[test]
    fn json_error_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let t = TypeRef::DecodeError(sp());
        assert_eq!(
            bynk_ts::print_expr(&deserialise_expr(&t, &ident("json"), "path", &ru)),
            "Ok(json as unknown as DecodeError) as Result<DecodeError, __BoundaryError>"
        );
    }

    #[test]
    fn http_result_names_its_real_element_type_and_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let t = TypeRef::HttpResult(Box::new(TypeRef::Base(BaseType::Int, sp())), None, sp());
        assert_eq!(
            bynk_ts::print_expr(&deserialise_expr(&t, &ident("json"), "path", &ru)),
            "Ok(json as unknown as HttpResult<number>) as Result<HttpResult<number>, __BoundaryError>"
        );
    }

    #[test]
    fn queue_result_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let t = TypeRef::QueueResult(sp());
        assert_eq!(
            bynk_ts::print_expr(&deserialise_expr(&t, &ident("json"), "path", &ru)),
            "Ok(json as unknown as QueueResult) as Result<QueueResult, __BoundaryError>"
        );
    }
}

#[cfg(test)]
mod peel_tests {
    use super::*;
    use bynk_syntax::span::Span;

    fn int() -> TypeRef {
        TypeRef::Base(BaseType::Int, Span::default())
    }

    #[test]
    fn a_list_or_option_peels_one_layer_and_says_which() {
        let list = TypeRef::List(Box::new(int()), Span::default());
        let (inner, is_list, is_option) = peel_list_option(&list);
        assert!(matches!(inner, TypeRef::Base(BaseType::Int, _)) && is_list && !is_option);
        let option = TypeRef::Option(Box::new(int()), Span::default());
        let (inner, is_list, is_option) = peel_list_option(&option);
        assert!(matches!(inner, TypeRef::Base(BaseType::Int, _)) && !is_list && is_option);
    }

    #[test]
    fn any_other_type_is_its_own_inner_and_one_layer_only_is_peeled() {
        let plain = int();
        let (inner, is_list, is_option) = peel_list_option(&plain);
        assert!(matches!(inner, TypeRef::Base(BaseType::Int, _)) && !is_list && !is_option);
        let nested = TypeRef::List(
            Box::new(TypeRef::Option(Box::new(int()), Span::default())),
            Span::default(),
        );
        let (inner, is_list, is_option) = peel_list_option(&nested);
        assert!(matches!(inner, TypeRef::Option(..)) && is_list && !is_option);
        let set = TypeRef::Set(Box::new(int()), Span::default());
        let (inner, is_list, is_option) = peel_list_option(&set);
        assert!(matches!(inner, TypeRef::Set(..)) && !is_list && !is_option);
    }
}

/// 1A.21 (#3722, Decision B): the serialiser entry points take their value as a node. This
/// corpus is the old-versus-new equality check: `serialiser_corpus.txt` was printed by the
/// text-valued entry points (`serialise_expr(t, "v", ..)`, `serialise_expr_via(t, "a.b", ns,
/// ..)`) before they changed, over every base type and a spread of nested shapes, and the
/// node-valued entries must print exactly the same.
#[cfg(test)]
mod serialiser_corpus_tests {
    use super::*;
    use bynk_syntax::ast::Ident;
    use bynk_syntax::span::Span;

    fn sp() -> Span {
        Span::new(0, 0)
    }

    fn named(n: &str) -> TypeRef {
        TypeRef::Named(Ident {
            name: n.to_string(),
            span: sp(),
        })
    }

    fn b(t: TypeRef) -> Box<TypeRef> {
        Box::new(t)
    }

    fn elements() -> Vec<(&'static str, TypeRef)> {
        let base = |n, t| (n, TypeRef::Base(t, sp()));
        vec![
            base("Int", BaseType::Int),
            base("String", BaseType::String),
            base("Bool", BaseType::Bool),
            base("Float", BaseType::Float),
            base("Duration", BaseType::Duration),
            base("Instant", BaseType::Instant),
            base("Bytes", BaseType::Bytes),
            base("Decimal", BaseType::Decimal),
            ("Foo", named("Foo")),
            ("ValidationError", TypeRef::ValidationError(sp())),
            ("DecodeError", TypeRef::DecodeError(sp())),
            ("QueueResult", TypeRef::QueueResult(sp())),
        ]
    }

    fn shapes(e: &TypeRef) -> Vec<(&'static str, TypeRef)> {
        let s = TypeRef::Base(BaseType::String, sp());
        vec![
            ("bare", e.clone()),
            ("Option", TypeRef::Option(b(e.clone()), sp())),
            ("List", TypeRef::List(b(e.clone()), sp())),
            ("Set", TypeRef::Set(b(e.clone()), sp())),
            ("Map", TypeRef::Map(b(s.clone()), b(e.clone()), sp())),
            ("Result", TypeRef::Result(b(e.clone()), b(s), sp())),
            ("Effect", TypeRef::Effect(b(e.clone()), sp())),
            ("HttpResult", TypeRef::HttpResult(b(e.clone()), None, sp())),
            (
                "Option<List>",
                TypeRef::Option(b(TypeRef::List(b(e.clone()), sp())), sp()),
            ),
            (
                "List<Option>",
                TypeRef::List(b(TypeRef::Option(b(e.clone()), sp())), sp()),
            ),
            (
                "Map<Foo,List>",
                TypeRef::Map(b(named("Foo")), b(TypeRef::List(b(e.clone()), sp())), sp()),
            ),
            (
                "App",
                TypeRef::App {
                    name: Ident {
                        name: "Pair".to_string(),
                        span: sp(),
                    },
                    args: vec![e.clone(), named("Foo")],
                    span: sp(),
                },
            ),
        ]
    }

    /// Every case's printed text, one `label | entry | text` line each.
    fn corpus() -> String {
        let ru = RuntimeUse::default();
        let mut out = String::new();
        let mut cases = vec![("Unit", TypeRef::Unit(sp()))];
        for (en, e) in elements() {
            for (sn, t) in shapes(&e) {
                cases.push((Box::leak(format!("{sn}[{en}]").into_boxed_str()) as &str, t));
            }
        }
        for (label, t) in &cases {
            for value in ["v", "value.items"] {
                let node = serialise_expr(t, &ident(value), &ru);
                out.push_str(&format!(
                    "{label} | {value} | {}\n",
                    bynk_ts::print_expr(&node)
                ));
            }
            for ns in ["", "handlers."] {
                let node = serialise_expr_via(t, &ident("a.b"), ns, &ru);
                out.push_str(&format!(
                    "{label} | via {ns:?} | {}\n",
                    bynk_ts::print_expr(&node)
                ));
            }
        }
        out
    }

    #[test]
    fn the_node_entry_prints_what_the_text_entry_printed() {
        let want = include_str!("serialiser_corpus.txt");
        let got = corpus();
        if std::env::var_os("BYNK_WRITE_SERIALISER_CORPUS").is_some() {
            std::fs::write(
                concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/src/emitter/serialisation/serialiser_corpus.txt"
                ),
                &got,
            )
            .unwrap();
            return;
        }
        assert_eq!(got.lines().count(), want.lines().count(), "corpus size");
        for (g, w) in got.lines().zip(want.lines()) {
            assert_eq!(g, w);
        }
    }
}
