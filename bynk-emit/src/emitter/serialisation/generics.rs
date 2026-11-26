//! Generic instantiations (back-half paydown 2.3b, #4088): the helpers for each generic instantiation a
//! module needs, and the lenient `Option` decode, with their tests. Moved out of `serialisation.rs`
//! byte for byte.

use std::sync::Arc;

use bynk_check::wire::{WireField, WireSum, WireVariant, recursive_codec_suffixes};
use bynk_syntax::ast::{BaseType, TypeBody, TypeDecl, TypeRef};
use bynk_ts::{
    TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt, TsType,
    TsTypeMember,
};

use crate::emitter::RuntimeUse;
use crate::ts_build::{
    and_expr, arrow, as_expr, binary, block, call, const_, ident, if_, index, member, not_expr,
    or_expr, ret, str_lit, strict_eq, strict_neq, typeof_expr,
};

use super::field::{emit_field_deserialise, serialise_field_expr};
use super::record_sum::{emit_record_codec, emit_sum_codec};
use super::support::{
    Depth, array_shape_guard, depth_expected, depth_for, deserialise_params,
    err_structural_mismatch, err_structural_mismatch_top, index_signature_record_ty, obj_prop,
    object_shape_guard, path_elem, path_entry, path_field, serialise_params, value_prop,
};
use super::{GenericInst, app_ts_name, inner_ts_name, record_inst_fields, sum_inst_variants};
use super::{Qual, qual_type, qualified_ts_type, wire_ref_of};

/// `(v) => <serialise v>`: the element serialiser a `List` or `Set` codec maps over.
fn element_serialiser(elem: &TypeRef, depth: Option<Depth<'_>>, ru: &RuntimeUse) -> TsExpr {
    arrow(
        vec![TsParam::new("v", None)],
        serialise_field_expr(elem, &ident("v"), depth, ru),
    )
}

/// `(globalThis as { __bynkPlainJson?: boolean }).__bynkPlainJson === true`: whether `withPlainJson`
/// has the plain-JSON flag set. The cast prints bracketed as the member's receiver.
pub(super) fn plain_json_test() -> TsExpr {
    strict_eq(
        member(
            TsExpr::As {
                expr: Box::new(ident("globalThis")),
                ty: TsType::Object(vec![TsTypeMember::optional_prop(
                    "__bynkPlainJson",
                    TsType::named("boolean"),
                )]),
            },
            "__bynkPlainJson",
        ),
        TsExpr::Lit(TsLit::Bool(true)),
    )
}

/// Constrained generics slice 2: whether a `Map` key / `Set` element declared as
/// `t` is a JS primitive, so a plain `Map`/`Set` compares it by value. Every
/// other `Key` type decodes into the keyed collections (`keys.ts`).
fn is_plain_key_ref(t: &TypeRef, types: &std::collections::HashMap<String, Arc<TypeDecl>>) -> bool {
    match t {
        TypeRef::Base(b, _) => !matches!(b, BaseType::Bytes | BaseType::Decimal),
        TypeRef::Named(id) => matches!(
            types.get(&id.name).map(|d| &d.body),
            Some(TypeBody::Refined { base, .. } | TypeBody::Opaque { base, .. })
                if !matches!(base, BaseType::Bytes | BaseType::Decimal)
        ),
        TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::List(..)
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
        | TypeRef::App { .. } => false,
    }
}

/// #3974: whether a `Map` key declared as `t` is a `String`, a refined or opaque `String`, so the
/// map also has the JSON object form (`{"k": v}`). Any other key type keeps the pair-array form.
pub(crate) fn is_string_key_ref(
    t: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> bool {
    let named_string = |id: &bynk_syntax::ast::Ident| {
        matches!(
            types.get(&id.name).map(|d| &d.body),
            Some(
                TypeBody::Refined {
                    base: BaseType::String,
                    ..
                } | TypeBody::Opaque {
                    base: BaseType::String,
                    ..
                }
            )
        )
    };
    matches!(t, TypeRef::Base(BaseType::String, _))
        || matches!(t, TypeRef::Named(id) if named_string(id))
}

/// The `Ok`/`Err` (or `Some`/`None`) cascade of a generic sum's wire decoder:
///
/// ```text
/// if (obj["kind"] === "A") { <guard_a> <return_a> } else if (obj["kind"] === "B") { <guard_b> <return_b> }
/// ```
///
/// One `If` with `same_line_else` whose else branch is the second `If`, so each
/// branch's guards sit at their true depth (1A.38a; they used to be spliced one
/// level shallow, as text).
fn wire_kind_dispatch(
    key_a: &str,
    guard_a: Vec<TsStmt>,
    return_a: TsStmt,
    key_b: &str,
    guard_b: Vec<TsStmt>,
    return_b: TsStmt,
) -> TsStmt {
    let wire_kind_eq = |key: &str| strict_eq(index(ident("obj"), str_lit("kind")), str_lit(key));
    let branch = |mut guard: Vec<TsStmt>, ret: TsStmt| {
        guard.push(ret);
        block(guard)
    };
    TsStmt::if_else_same_line_stmt(
        wire_kind_eq(key_a),
        branch(guard_a, return_a),
        if_(wire_kind_eq(key_b), branch(guard_b, return_b)),
        None,
    )
}

/// The text appended to a bare value's `StructuralMismatch.expected` when it
/// fails to decode as an `Option[U]`'s `U` (#1887), so the error names every
/// form the `Option` decoder would have taken, not only `U`'s.
const OPTION_FORMS_SUFFIX: &str =
    " | null | {\"kind\": \"None\"} | {\"kind\": \"Some\", \"value\": ...}";

/// `Ok(<value> as Option<U>)`: the `Option` decoder's one success shape.
fn ok_option(value: TsExpr, option_ty: &TsType) -> TsStmt {
    ret(call(ident("Ok"), vec![as_expr(value, option_ty.clone())]))
}

/// The body of `__deserialise_Option_<U>` (#1887, ADR amending 0045). The
/// decoder is lenient: it accepts the forms the rest of the JSON world uses
/// for an optional value, as well as the tagged form Bynk's encoder writes.
/// It is the one `Option` codec, so the rule holds at every boundary that
/// decodes JSON: `Json.decode`, an HTTP request body, a service-to-service
/// call, agent-store rehydration and a WebSocket frame.
///
/// In order:
///
/// 1. `undefined` (the key is absent from the enclosing record, whose codec
///    reads `obj["f"]` with no presence check) or `null` decodes to `None`.
/// 2. An object whose only key is `kind`, set to `"None"`, decodes to `None`.
/// 3. An object whose only keys are `kind` and `value`, with `kind` set to
///    `"Some"`, decodes to `Some(U(value))`, the error path at `$.value`.
/// 4. Anything else is a bare value and decodes to `Some(U(json))`.
///
/// The tagged forms are tried before the bare one, so they win when a value
/// could be read both ways. That keeps `decode(encode(x)) == x` for every
/// `x`, because the encoder always writes a tagged form: `Some(None)` of an
/// `Option[Option[T]]` is `{kind: "Some", value: {kind: "None"}}`, and
/// `Some(r)` for an `r` that itself has `kind`/`value` fields wraps `r` in
/// an outer tag, so the inner decoder sees `r` intact. The cost falls on
/// non-Bynk input only: a bare `{"kind": "None"}` for an `Option[Option[T]]`
/// is `None`, not `Some(None)`.
///
/// `U` is decoded by a local `__decode(raw, at)` arrow, so its checks are
/// emitted once and shared by rules 3 and 4. Its result is typed
/// `Result<unknown, …>` and cast to `U` on success, as the old decoder cast
/// `Some(__v)`, because `U`'s codec may return an unbranded record where
/// this module's `U` is a branded rebrand (#527). When a bare value fails as
/// a `StructuralMismatch` at the `Option`'s own path, the error's `expected`
/// is extended with [`OPTION_FORMS_SUFFIX`]. A deeper error, or a
/// `RefinementViolation`, is returned as `U` reported it.
fn lenient_option_decode_body(
    value_fn: &str,
    inner_ty: &TsType,
    option_ty: &TsType,
    depth: Option<Depth<'_>>,
) -> Vec<TsStmt> {
    // #2656: the value decoder takes the depth when the `Option` is on a type
    // cycle ([`option_value_decoder`]).
    let value_args = |raw: TsExpr, at: TsExpr| -> Vec<TsExpr> {
        let mut args = vec![raw, at];
        if depth.is_some() {
            args.push(ident("depth"));
        }
        args
    };
    let some_of = |result: &str| {
        call(
            ident("Some"),
            vec![as_expr(member(ident(result), "value"), inner_ty.clone())],
        )
    };
    let obj_kind_is = |kind: &str| strict_eq(index(ident("obj"), str_lit("kind")), str_lit(kind));
    let keys_len_is = |n: &str| {
        strict_eq(
            member(ident("__keys"), "length"),
            TsExpr::Lit(TsLit::Num(n.to_string())),
        )
    };
    let tagged = vec![
        TsStmt::const_stmt(
            TsBindingName::Ident("obj".to_string()),
            None,
            as_expr(ident("json"), index_signature_record_ty()),
            None,
        ),
        const_(
            "__keys",
            call(
                member(ident("globalThis.Object"), "keys"),
                vec![ident("obj")],
            ),
        ),
        // Rule 2: exactly `{kind: "None"}`.
        if_(
            and_expr(obj_kind_is("None"), keys_len_is("1")),
            block(vec![ok_option(ident("None"), option_ty)]),
        ),
        // Rule 3: exactly `{kind: "Some", value}`.
        if_(
            and_expr(
                and_expr(obj_kind_is("Some"), keys_len_is("2")),
                binary(TsBinaryOp::In, str_lit("value"), ident("obj")),
            ),
            block(vec![
                const_(
                    "__t",
                    call(
                        ident(value_fn),
                        value_args(index(ident("obj"), str_lit("value")), path_field("value")),
                    ),
                ),
                if_(
                    strict_eq(member(ident("__t"), "tag"), str_lit("Err")),
                    block(vec![ret(ident("__t"))]),
                ),
                ok_option(some_of("__t"), option_ty),
            ]),
        ),
    ];
    let bare_error = member(ident("__b"), "error");
    let mut extend_cond = and_expr(
        and_expr(
            strict_eq(
                member(bare_error.clone(), "kind"),
                str_lit("StructuralMismatch"),
            ),
            strict_eq(member(bare_error.clone(), "path"), ident("path")),
        ),
        not_expr(call(
            member(member(bare_error.clone(), "expected"), "endsWith"),
            vec![str_lit(OPTION_FORMS_SUFFIX)],
        )),
    );
    // #2656: the nesting bound's error is not about the `Option`'s forms, so
    // it is returned as the inner decoder reported it.
    if depth.is_some() {
        extend_cond = and_expr(
            extend_cond,
            strict_neq(
                member(bare_error.clone(), "expected"),
                str_lit(depth_expected()),
            ),
        );
    }
    vec![
        // Rule 1: absent or `null`.
        if_(
            or_expr(
                strict_eq(ident("json"), ident("undefined")),
                strict_eq(ident("json"), TsExpr::Lit(TsLit::Null)),
            ),
            block(vec![ok_option(ident("None"), option_ty)]),
        ),
        if_(
            and_expr(
                strict_eq(typeof_expr(ident("json")), str_lit("object")),
                not_expr(call(
                    member(ident("globalThis.Array"), "isArray"),
                    vec![ident("json")],
                )),
            ),
            block(tagged),
        ),
        // Rule 4: a bare value.
        const_(
            "__b",
            call(ident(value_fn), value_args(ident("json"), ident("path"))),
        ),
        if_(
            strict_eq(member(ident("__b"), "tag"), str_lit("Ok")),
            block(vec![ok_option(some_of("__b"), option_ty)]),
        ),
        // The suffix is appended once: for an `Option[Option[T]]` the inner
        // decoder has already extended an error at this same path.
        if_(
            extend_cond,
            block(vec![ret(call(
                ident("Err"),
                vec![TsExpr::object_entries(vec![
                    TsObjectEntry::Spread(bare_error.clone()),
                    TsObjectEntry::prop(
                        "expected".to_string(),
                        binary(
                            TsBinaryOp::Add,
                            member(bare_error, "expected"),
                            str_lit(OPTION_FORMS_SUFFIX),
                        ),
                    ),
                ])],
            ))]),
        ),
        ret(ident("__b")),
    ]
}

/// `function <name>(raw, at): Result<unknown, __BoundaryError>`: decodes one
/// `Option[U]`'s `U` from `raw`, reporting errors at `at` (#1887). The
/// lenient `Option` decoder ([`lenient_option_decode_body`]) calls it for
/// both the tagged `value` and a bare value, so `U`'s checks are emitted
/// once. Module-local, not exported: only its own module's
/// `__deserialise_Option_<U>` calls it, and an export would collide under
/// a barrel's `export *` the way a shared helper name can.
fn option_value_decoder(
    name: String,
    inner: &TypeRef,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> TsDecl {
    let mut body = emit_field_deserialise("v", inner, &ident("raw"), &ident("at"), depth, ru);
    body.push(ret(call(ident("Ok"), vec![ident("__v")])));
    let mut params = vec![
        TsParam::new("raw".to_string(), Some(TsType::named("__JsonValue"))),
        TsParam::new("at".to_string(), Some(TsType::named("string"))),
    ];
    // #2656: on a type cycle, the `Option` decoder hands its depth through.
    if depth.is_some() {
        params.push(TsParam::new(
            "depth".to_string(),
            Some(TsType::named("number")),
        ));
    }
    TsDecl::Function {
        name,
        generics: Vec::new(),
        params,
        return_type: Some(TsType::named_with_args(
            "Result",
            vec![TsType::named("unknown"), TsType::named("__BoundaryError")],
        )),
        body,
        is_async: false,
        inline: false,
    }
}

/// Emit specialised helpers for each `Result<A, B>` / `Option<A>`
/// instantiation. They delegate to the named-type serialisers for A and B.
/// v0.174 (#592): also emits a monomorphised record codec per generic
/// instantiation (`RecordInst`), which needs the declarations to substitute
/// its type parameters.
pub(crate) fn emit_generic_helpers(
    insts: &[GenericInst],
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    emit_generic_helpers_qualified(insts, types, &Qual::new(), ru)
}

/// #661: as `emit_generic_helpers`, but the value-type positions of each
/// specialised helper are named through the type `Qual` — so a consumer's
/// `deserialise_Result_AuthId_PaymentError` returns
/// `Result<commerce_payment.AuthId, commerce_payment.PaymentError>` while its
/// codec calls stay local. The codec *suffix* (`Result_AuthId_PaymentError`) is
/// namespace-independent by construction, which is exactly what keeps the
/// caller's and callee's names in agreement across the wire.
///
/// Arc F slice 1 (#1451): returns the real `bynk_ts::TsDecl` nodes directly —
/// no `out: &mut String` parameter. The boundary-collect step
/// ([`super::decls_as_stmts`]) now happens once, at the caller, instead of inline in
/// each of this function's six match arms.
pub fn emit_generic_helpers_qualified(
    insts: &[GenericInst],
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    let mut decls = Vec::new();
    let codecs = recursive_codec_suffixes(types);
    for inst in insts {
        // #2656: a container codec on a type cycle passes the nesting depth
        // through unchanged; a generic record or sum counts a level, as a
        // non-generic one does (`emit_record_codec`/`emit_sum_codec`).
        let depth = depth_for(&codecs, &inst.ts_name(), false);
        match inst {
            // v0.174 (#592): a generic-record instantiation `Paginated[User]`
            // emits `serialise_Paginated_User` / `deserialise_Paginated_User`,
            // its fields specialised to the concrete arguments. The value type
            // is the erased generic `Paginated<User>`.
            GenericInst::RecordInst { name, args } => {
                let fn_suffix = app_ts_name(name, args);
                let ts_type = TsType::named_with_args(
                    qual_type(qual, name),
                    args.iter().map(|a| qualified_ts_type(a, qual)).collect(),
                );
                // `record_inst_fields` is `None` only for an unknown name, a
                // non-record body, or an arity mismatch — all of which the
                // resolver rejects (`generic_non_record` / `type_arg_count`)
                // before a `RecordInst` is ever collected. Panic loudly rather
                // than silently emit a call to an undefined codec (the file's
                // convention for a checker-guaranteed invariant).
                let fields = record_inst_fields(name, args, types).unwrap_or_else(|| {
                    unreachable!("RecordInst `{name}` is not a resolved generic record")
                });
                // #2607 (Decision I): a field's default rides along, paired
                // with the field's *substituted* type, so it renders at the
                // instantiation exactly as it would in a non-generic record —
                // the checker only admits a default whose value holds for
                // every instantiation (`None`, `[]`, a concrete field's
                // literal). #855 (Phase 2 step 9): a
                // `WireField` for a substituted instantiation type is built
                // straight from `record_inst_fields`' resolved `TypeRef`s via
                // `wire_ref_of`, mirroring what `wire_type`/`wire_fields`
                // would derive for a non-generic declaration's own fields.
                //
                // #1969: the instantiation keeps each field's inline
                // refinement, read off the declaration and paired by name, so
                // a field the two lists disagree on panics rather than
                // silently losing its check (the contract hash would still
                // include it).
                #[allow(clippy::type_complexity)]
                let declared: std::collections::HashMap<
                    &str,
                    (
                        Vec<bynk_syntax::ast::PredKind>,
                        Option<&bynk_syntax::ast::Expr>,
                    ),
                > = match types.get(name.as_str()).map(|d| &d.body) {
                    Some(bynk_syntax::ast::TypeBody::Record(r)) => r
                        .fields
                        .iter()
                        .map(|f| {
                            (
                                f.name.name.as_str(),
                                (bynk_check::wire::field_predicates(f), f.init.as_ref()),
                            )
                        })
                        .collect(),
                    _ => unreachable!("RecordInst `{name}` is not a resolved generic record"),
                };
                let fields: Vec<WireField> = fields
                    .into_iter()
                    .map(|(n, t)| {
                        let (predicates, init) =
                            declared.get(n.as_str()).cloned().unwrap_or_else(|| {
                                unreachable!("`{name}` has no declared field `{n}`")
                            });
                        // #3975: the generic declaration's `@name` for this field.
                        let wire = match types.get(name.as_str()).map(|d| &d.body) {
                            Some(bynk_syntax::ast::TypeBody::Record(r)) => r
                                .fields
                                .iter()
                                .find(|f| f.name.name == n)
                                .and_then(|f| f.wire_name())
                                .unwrap_or_else(|| n.clone()),
                            _ => n.clone(),
                        };
                        WireField {
                            shape: wire_ref_of(&t),
                            predicates,
                            key: wire.clone(),
                            path_segment: wire,
                            default: init.map(|e| (e.clone(), t.clone())),
                            // #3793: a `replaces` on the generic declaration's field.
                            renamed_from: match types.get(name.as_str()).map(|d| &d.body) {
                                Some(bynk_syntax::ast::TypeBody::Record(r)) => r
                                    .fields
                                    .iter()
                                    .find(|f| f.name.name == n)
                                    .map(bynk_check::wire::field_renamed_from)
                                    .unwrap_or_default(),
                                _ => Vec::new(),
                            },
                            name: n,
                        }
                    })
                    .collect();
                // #1443 (Arc E slice 5): `emit_record_codec` itself now
                // builds real `bynk_ts::TsDecl` nodes — this whole function
                // stays `String`-based (out of this slice's scope, per the
                // issue's own "real complications" note 3), so this call
                // site needs the same boundary-print treatment `emit_one`'s
                // `Record`/`Refined`/`Opaque` arms already use (#1441,
                // #1443).
                decls.extend(emit_record_codec(
                    &fn_suffix, &ts_type, &fields, types, &codecs, ru,
                ));
            }
            // #593: a generic-sum instantiation `ApiResult[User]` emits
            // `serialise_ApiResult_User` / `deserialise_ApiResult_User`, its
            // variant payloads specialised to the concrete arguments. The value
            // type is the erased generic `ApiResult<User>`. Mirrors `RecordInst`.
            GenericInst::SumInst { name, args } => {
                let fn_suffix = app_ts_name(name, args);
                // #1736: qualified like `RecordInst` above: a consumed
                // context's generic sum is named through its namespace.
                let ts_type = TsType::named_with_args(
                    qual_type(qual, name),
                    args.iter().map(|a| qualified_ts_type(a, qual)).collect(),
                );
                let variants = sum_inst_variants(name, args, types).unwrap_or_else(|| {
                    unreachable!("SumInst `{name}` is not a resolved generic sum")
                });
                // #855 (Phase 2 step 9): a `WireSum` for a substituted
                // instantiation is built straight from `sum_inst_variants`'
                // resolved `TypeRef`s via `wire_ref_of`, mirroring what
                // `wire_type`/`wire_sum` would derive for a non-generic
                // declaration's own variants. The wire/memory discriminants
                // are the same fixed `"kind"`/`"tag"` pair `wire_sum` uses.
                // #3975: the generic declaration's `@discriminator`, and its variants' and
                // payload fields' `@name`s.
                let decl_sum = match types.get(name.as_str()).map(|d| &d.body) {
                    Some(bynk_syntax::ast::TypeBody::Sum(s)) => Some(s),
                    _ => None,
                };
                let sum = WireSum {
                    wire_discriminant: types
                        .get(name.as_str())
                        .and_then(|d| d.discriminator())
                        .unwrap_or_else(|| bynk_check::wire::WIRE_DISCRIMINANT.to_string()),
                    memory_discriminant: "tag",
                    variants: variants
                        .into_iter()
                        .map(|(vname, payload)| {
                            let decl_variant = decl_sum
                                .and_then(|s| s.variants.iter().find(|v| v.name.name == vname));
                            WireVariant {
                                wire_name: decl_variant
                                    .and_then(|v| v.wire_name())
                                    .unwrap_or_else(|| vname.clone()),
                                name: vname,
                                payload: payload
                                    .into_iter()
                                    .map(|(fname, t)| {
                                        let wire = decl_variant
                                            .and_then(|v| {
                                                v.payload.iter().find(|p| p.name.name == fname)
                                            })
                                            .and_then(|p| p.wire_name())
                                            .unwrap_or_else(|| fname.clone());
                                        WireField {
                                            shape: wire_ref_of(&t),
                                            key: wire.clone(),
                                            path_segment: wire,
                                            name: fname,
                                            default: None,
                                            predicates: Vec::new(),
                                            renamed_from: Vec::new(),
                                        }
                                    })
                                    .collect(),
                            }
                        })
                        .collect(),
                };
                // #1445 (Arc E slice 6): `emit_sum_codec` itself now builds
                // real `bynk_ts::TsDecl` nodes — this whole function stays
                // `String`-based (out of this slice's scope, per the
                // issue's own "real complications" note 4, mirroring
                // `RecordInst`'s own identical treatment in #1443), so this
                // call site needs the same boundary-print treatment
                // `emit_one`'s `Sum`/`Record`/`Refined`/`Opaque` arms
                // already use (#1441, #1443, #1445).
                decls.extend(emit_sum_codec(&fn_suffix, &ts_type, &sum, &codecs, ru));
            }
            // #1447 (Arc E slice 7): builds the `[serialise, deserialise]`
            // pair as real `bynk_ts::TsDecl` nodes — the same shape
            // `emit_record_codec`/`emit_sum_codec`/`emit_bytes_named_codec`
            // already established (#1443/#1445/#1441) — and boundary-prints
            // them the same way `RecordInst`/`SumInst` above already do.
            // Unlike those two, there is no separate `emit_result_codec` to
            // delegate to: this match arm IS the codec body (the issue's own
            // scope note), so the `TsDecl::Function` construction sits
            // inline here rather than behind a further indirection.
            GenericInst::ResultInst { ok, err } => {
                let ok_ts = inner_ts_name(ok);
                let err_ts = inner_ts_name(err);
                let ok_inner = qualified_ts_type(ok, qual);
                let err_inner = qualified_ts_type(err, qual);
                let result_ty = TsType::named_with_args("Result", vec![ok_inner, err_inner]);

                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_Result_{ok_ts}_{err_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(result_ty.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body: vec![
                        if_(
                            strict_eq(member(ident("value"), "tag"), str_lit("Ok")),
                            ret(TsExpr::object(vec![
                                ("kind".to_string(), str_lit("Ok")),
                                (
                                    "value".to_string(),
                                    serialise_field_expr(ok, &value_prop("value"), depth, ru),
                                ),
                            ])),
                        ),
                        ret(TsExpr::object(vec![
                            ("kind".to_string(), str_lit("Err")),
                            (
                                "error".to_string(),
                                serialise_field_expr(err, &value_prop("error"), depth, ru),
                            ),
                        ])),
                    ],
                    is_async: false,
                    inline: false,
                }));

                let mut body = object_shape_guard();
                body.push(wire_kind_dispatch(
                    "Ok",
                    emit_field_deserialise(
                        "v",
                        ok,
                        &obj_prop("value"),
                        &path_field("value"),
                        depth,
                        ru,
                    ),
                    ret(call(
                        ident("Ok"),
                        vec![as_expr(
                            call(ident("Ok"), vec![ident("__v")]),
                            result_ty.clone(),
                        )],
                    )),
                    "Err",
                    emit_field_deserialise(
                        "e",
                        err,
                        &obj_prop("error"),
                        &path_field("error"),
                        depth,
                        ru,
                    ),
                    ret(call(
                        ident("Ok"),
                        vec![as_expr(
                            call(ident("Err"), vec![ident("__e")]),
                            result_ty.clone(),
                        )],
                    )),
                ));
                body.push(ret(err_structural_mismatch_top(
                    "Ok | Err",
                    call(ident("String"), vec![index(ident("obj"), str_lit("kind"))]),
                )));

                let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__deserialise_Result_{ok_ts}_{err_ts}"),
                    generics: Vec::new(),
                    params: deserialise_params(depth),
                    return_type: Some(TsType::named_with_args(
                        "Result",
                        vec![result_ty, TsType::named("__BoundaryError")],
                    )),
                    body,
                    is_async: false,
                    inline: false,
                }));

                decls.extend([serialise, deserialise]);
            }
            GenericInst::OptionInst { inner } => {
                let inner_ts = inner_ts_name(inner);
                let inner_ty = qualified_ts_type(inner, qual);
                let option_ty = TsType::named_with_args("Option", vec![inner_ty.clone()]);

                // #2858: in a project with a plain-JSON route the one `Option`
                // encoder also carries the plain branch: while `withPlainJson` has
                // `__bynkPlainJson` set, `Some(v)` is `v` and `None` is `null`.
                // Otherwise (and in every project without such a route) it is the
                // tagged encoder alone.
                let some_value =
                    serialise_field_expr(inner, &member(ident("value"), "value"), depth, ru);
                let tagged_some = TsExpr::object(vec![
                    ("kind".to_string(), str_lit("Some")),
                    ("value".to_string(), some_value.clone()),
                ]);
                let tagged_none = TsExpr::object(vec![("kind".to_string(), str_lit("None"))]);
                let body = if ru.plain_json() {
                    let plain = plain_json_test;
                    let pick = |plain_branch: TsExpr, tagged_branch: TsExpr| TsExpr::Conditional {
                        test: Box::new(plain()),
                        consequent: Box::new(plain_branch),
                        alternate: Box::new(tagged_branch),
                    };
                    vec![
                        if_(
                            strict_eq(member(ident("value"), "tag"), str_lit("Some")),
                            ret(pick(some_value, tagged_some)),
                        ),
                        ret(pick(ident("null"), tagged_none)),
                    ]
                } else {
                    vec![
                        if_(
                            strict_eq(member(ident("value"), "tag"), str_lit("Some")),
                            ret(tagged_some),
                        ),
                        ret(tagged_none),
                    ]
                };
                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_Option_{inner_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(option_ty.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body,
                    is_async: false,
                    inline: false,
                }));

                let value_fn = format!("__option_value_{inner_ts}");
                let body = lenient_option_decode_body(&value_fn, &inner_ty, &option_ty, depth);
                let value_decoder = option_value_decoder(value_fn, inner, depth, ru);

                let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__deserialise_Option_{inner_ts}"),
                    generics: Vec::new(),
                    params: deserialise_params(depth),
                    return_type: Some(TsType::named_with_args(
                        "Result",
                        vec![option_ty, TsType::named("__BoundaryError")],
                    )),
                    body,
                    is_async: false,
                    inline: false,
                }));

                decls.extend([serialise, value_decoder, deserialise]);
            }
            // v0.20b: `List[T]` — element-wise wire format (a JSON array).
            GenericInst::ListInst { elem } => {
                let elem_ts = inner_ts_name(elem);
                let elem_ty = qualified_ts_type(elem, qual);
                let readonly_elem_array = TsType::readonly_array(elem_ty.clone());

                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_List_{elem_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(readonly_elem_array.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body: vec![ret(call(
                        member(ident("value"), "map"),
                        vec![element_serialiser(elem, depth, ru)],
                    ))],
                    is_async: false,
                    inline: false,
                }));

                // Bind the element before validating: `json[i]` with a
                // mutable index does not narrow under a typeof guard.
                let mut loop_body = vec![const_("item", index(ident("json"), ident("i")))];
                loop_body.extend(emit_field_deserialise(
                    "el",
                    elem,
                    &ident("item"),
                    &path_elem(),
                    depth,
                    ru,
                ));
                // The element deserialiser may come from the declaring
                // commons and return the *unbranded* record; this module's
                // element type may be the context's branded rebrand. Assert
                // the element like the Option codec above does (#527).
                loop_body.push(TsStmt::expr_stmt(
                    call(
                        member(ident("out"), "push"),
                        vec![as_expr(ident("__el"), elem_ty.clone())],
                    ),
                    None,
                ));

                let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__deserialise_List_{elem_ts}"),
                    generics: Vec::new(),
                    params: deserialise_params(depth),
                    return_type: Some(TsType::named_with_args(
                        "Result",
                        vec![readonly_elem_array, TsType::named("__BoundaryError")],
                    )),
                    body: vec![
                        array_shape_guard(),
                        TsStmt::const_stmt(
                            TsBindingName::Ident("out".to_string()),
                            Some(TsType::array(elem_ty)),
                            TsExpr::array(vec![]),
                            None,
                        ),
                        TsStmt::for_stmt(
                            "i",
                            TsExpr::Lit(TsLit::Num("0".to_string())),
                            binary(
                                TsBinaryOp::LessThan,
                                ident("i"),
                                member(ident("json"), "length"),
                            ),
                            block(loop_body),
                            None,
                        ),
                        ret(call(ident("Ok"), vec![ident("out")])),
                    ],
                    is_async: false,
                    inline: false,
                }));

                decls.extend([serialise, deserialise]);
            }
            // K6a (#2596): `Set[T]` — a JSON array in insertion order. The
            // decoder refuses a duplicate element: a Bynk sender never writes
            // one, so it means the payload is not the set it claims to be.
            GenericInst::SetInst { elem } => {
                let elem_ts = inner_ts_name(elem);
                let elem_ty = qualified_ts_type(elem, qual);
                let set_ty = TsType::named_with_args("ReadonlySet", vec![elem_ty.clone()]);
                // Slice 2 (DECISION H): a composite element decodes into the keyed
                // set, whose `has` is value equality, so a wire duplicate is
                // rejected whatever the element's shape.
                let set_ctor = if is_plain_key_ref(elem, types) {
                    "globalThis.Set"
                } else {
                    ru.note_key("__BynkKeySet");
                    "__BynkKeySet"
                };
                let set_ctor = TsExpr::TypeApp {
                    callee: Box::new(ident(set_ctor)),
                    type_args: vec![elem_ty.clone()],
                };

                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_Set_{elem_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(set_ty.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body: vec![ret(call(
                        member(
                            call(ident("globalThis.Array.from"), vec![ident("value")]),
                            "map",
                        ),
                        vec![element_serialiser(elem, depth, ru)],
                    ))],
                    is_async: false,
                    inline: false,
                }));

                let mut loop_body = vec![const_("item", index(ident("json"), ident("i")))];
                loop_body.extend(emit_field_deserialise(
                    "el",
                    elem,
                    &ident("item"),
                    &path_elem(),
                    depth,
                    ru,
                ));
                loop_body.push(if_(
                    call(
                        member(ident("out"), "has"),
                        vec![as_expr(ident("__el"), elem_ty.clone())],
                    ),
                    block(vec![ret(err_structural_mismatch(
                        &path_elem(),
                        "a distinct set element",
                        str_lit("a duplicate"),
                    ))]),
                ));
                loop_body.push(TsStmt::expr_stmt(
                    call(
                        member(ident("out"), "add"),
                        vec![as_expr(ident("__el"), elem_ty.clone())],
                    ),
                    None,
                ));

                let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__deserialise_Set_{elem_ts}"),
                    generics: Vec::new(),
                    params: deserialise_params(depth),
                    return_type: Some(TsType::named_with_args(
                        "Result",
                        vec![set_ty, TsType::named("__BoundaryError")],
                    )),
                    body: vec![
                        array_shape_guard(),
                        const_(
                            "out",
                            TsExpr::New {
                                callee: Box::new(set_ctor.clone()),
                                args: vec![],
                            },
                        ),
                        TsStmt::for_stmt(
                            "i",
                            TsExpr::Lit(TsLit::Num("0".to_string())),
                            binary(
                                TsBinaryOp::LessThan,
                                ident("i"),
                                member(ident("json"), "length"),
                            ),
                            block(loop_body),
                            None,
                        ),
                        ret(call(ident("Ok"), vec![ident("out")])),
                    ],
                    is_async: false,
                    inline: false,
                }));

                decls.extend([serialise, deserialise]);
            }
            // v0.20b: `Map[K, V]` — entries-array wire format `[[k, v], …]`,
            // uniform across String/Int keys and insertion-ordered
            // (normative, §7).
            GenericInst::MapInst { key, val } => {
                decls.extend(emit_map_codec(key, val, types, qual, depth, ru));
            }
        }
    }
    decls
}

/// `Map[K, V]` — entries-array wire format `[[k, v], …]`, uniform across String/Int keys and
/// insertion-ordered (normative, §7); a `String`-keyed map also has the object form (#3974).
fn emit_map_codec(
    key: &TypeRef,
    val: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    let key_ts = inner_ts_name(key);
    let val_ts = inner_ts_name(val);
    let key_ty = qualified_ts_type(key, qual);
    let val_ty = qualified_ts_type(val, qual);
    let map_ty = TsType::named_with_args("ReadonlyMap", vec![key_ty.clone(), val_ty.clone()]);
    // `new Map<{key_ty}, {val_ty}>()`'s own callee has no real
    // `TsExpr::New` generics field to hold `<key_ty, val_ty>`
    // (see `TsStmtKind::For`'s own sibling doc for this file's
    // one other real algebra gap this slice found) — printing a
    // real `TsType::named_with_args("Map", ..)` node through the
    // same `bynk_ts::print_type` boundary this arm already uses
    // for `key_ty`/`val_ty` themselves builds the exact callee
    // text with no `format!` needed, spliced as an opaque
    // `Ident` the same way `ident(format!("deserialise_{type_name}"))`
    // already reuses that variant as a raw-text callee slot
    // elsewhere in this file (`emit_field_deserialise_wire`'s
    // own `Named`/`Inst` arm).
    let map_ctor = if is_plain_key_ref(key, types) {
        "globalThis.Map"
    } else {
        ru.note_key("__BynkKeyMap");
        "__BynkKeyMap"
    };
    let map_ctor = TsExpr::TypeApp {
        callee: Box::new(ident(map_ctor)),
        type_args: vec![key_ty.clone(), val_ty.clone()],
    };

    let string_key = is_string_key_ref(key, types);
    let mut serialise_body = Vec::new();
    if string_key && ru.plain_json() {
        serialise_body.push(plain_map_object(key, val, depth, ru));
    }
    serialise_body.extend([
        TsStmt::const_stmt(
            TsBindingName::Ident("entries".to_string()),
            Some(TsType::array(TsType::named("__JsonValue"))),
            TsExpr::array(vec![]),
            None,
        ),
        TsStmt::for_of(
            "[k, v]",
            ident("value"),
            block(vec![TsStmt::expr_stmt(
                call(
                    member(ident("entries"), "push"),
                    vec![TsExpr::array(vec![
                        serialise_field_expr(key, &ident("k"), depth, ru),
                        serialise_field_expr(val, &ident("v"), depth, ru),
                    ])],
                ),
                None,
            )]),
            None,
        ),
        ret(ident("entries")),
    ]);
    let serialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__serialise_Map_{key_ts}_{val_ts}"),
        generics: Vec::new(),
        params: serialise_params(map_ty.clone(), depth),
        return_type: Some(TsType::named("__JsonValue")),
        body: serialise_body,
        is_async: false,
        inline: false,
    }));

    let mut loop_body = vec![
        const_("entry", index(ident("json"), ident("i"))),
        if_(
            or_expr(
                not_expr(call(
                    member(ident("globalThis.Array"), "isArray"),
                    vec![ident("entry")],
                )),
                strict_neq(
                    member(ident("entry"), "length"),
                    TsExpr::Lit(TsLit::Num("2".to_string())),
                ),
            ),
            block(vec![ret(err_structural_mismatch(
                &path_elem(),
                "[key, value] entry",
                typeof_expr(ident("entry")),
            ))]),
        ),
        const_(
            "entryK",
            index(ident("entry"), TsExpr::Lit(TsLit::Num("0".to_string()))),
        ),
        const_(
            "entryV",
            index(ident("entry"), TsExpr::Lit(TsLit::Num("1".to_string()))),
        ),
    ];
    loop_body.extend(emit_field_deserialise(
        "k",
        key,
        &ident("entryK"),
        &path_entry(0),
        depth,
        ru,
    ));
    // #2707: a repeated key is refused, never last-entry-wins. `has`
    // is value equality: a plain `Map` for a primitive key, the
    // keyed class (canonical encoding, so `1.20` and `1.2` are one
    // `Decimal` key) for a composite or `Decimal`/`Bytes` key.
    loop_body.push(if_(
        call(
            member(ident("out"), "has"),
            vec![as_expr(ident("__k"), key_ty.clone())],
        ),
        block(vec![ret(err_structural_mismatch(
            &path_entry(0),
            "a distinct map key",
            str_lit("a duplicate"),
        ))]),
    ));
    loop_body.extend(emit_field_deserialise(
        "v",
        val,
        &ident("entryV"),
        &path_entry(1),
        depth,
        ru,
    ));
    // Same brand assertion as the List codec (#527).
    loop_body.push(TsStmt::expr_stmt(
        call(
            member(ident("out"), "set"),
            vec![as_expr(ident("__k"), key_ty), as_expr(ident("__v"), val_ty)],
        ),
        None,
    ));

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_Map_{key_ts}_{val_ts}"),
        generics: Vec::new(),
        params: deserialise_params(depth),
        return_type: Some(TsType::named_with_args(
            "Result",
            vec![map_ty, TsType::named("__BoundaryError")],
        )),
        body: [
            string_key
                .then(|| object_form_map(&format!("__deserialise_Map_{key_ts}_{val_ts}"), depth))
                .into_iter()
                .collect::<Vec<_>>(),
            vec![array_shape_guard()],
            vec![TsStmt::const_stmt(
                TsBindingName::Ident("out".to_string()),
                None,
                TsExpr::New {
                    callee: Box::new(map_ctor.clone()),
                    args: vec![],
                },
                None,
            )],
            vec![TsStmt::for_stmt(
                "i",
                TsExpr::Lit(TsLit::Num("0".to_string())),
                binary(
                    TsBinaryOp::LessThan,
                    ident("i"),
                    member(ident("json"), "length"),
                ),
                block(loop_body),
                None,
            )],
            vec![ret(call(ident("Ok"), vec![ident("out")]))],
        ]
        .concat(),
        is_async: false,
        inline: false,
    }));
    vec![serialise, deserialise]
}

/// #3974: the plain-mode branch of a `String`-keyed map's encoder: while `__bynkPlainJson` is
/// set, the map is a JSON object (insertion-ordered, keys as written). The object has no
/// prototype, so a `"__proto__"` key is an own property, not the prototype setter.
fn plain_map_object(
    key: &TypeRef,
    val: &TypeRef,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> TsStmt {
    let record = TsType::named_with_args(
        "globalThis.Record",
        vec![TsType::named("string"), TsType::named("__JsonValue")],
    );
    let key_text = as_expr(
        serialise_field_expr(key, &ident("k"), depth, ru),
        TsType::named("string"),
    );
    if_(
        plain_json_test(),
        block(vec![
            TsStmt::const_stmt(
                TsBindingName::Ident("object".to_string()),
                Some(record),
                call(
                    member(ident("globalThis.Object"), "create"),
                    vec![ident("null")],
                ),
                None,
            ),
            TsStmt::for_of(
                "[k, v]",
                ident("value"),
                block(vec![TsStmt::assign(
                    index(ident("object"), key_text),
                    serialise_field_expr(val, &ident("v"), depth, ru),
                    None,
                )]),
                None,
            ),
            ret(ident("object")),
        ]),
    )
}

/// #3974: the object form of a `String`-keyed map's decoder: a JSON object is read as its
/// `[key, value]` pairs, in the object's own order, by this same decoder, so the key decodes
/// (and any refinement is checked) exactly as it does in the pair-array form. `JSON.parse` lists
/// integer-like keys first, in ascending order, and keeps only the last of a repeated key; the
/// reference says so.
fn object_form_map(decoder: &str, depth: Option<Depth<'_>>) -> TsStmt {
    let is_object = and_expr(
        and_expr(
            strict_eq(typeof_expr(ident("json")), str_lit("object")),
            strict_neq(ident("json"), TsExpr::Lit(TsLit::Null)),
        ),
        not_expr(call(
            member(ident("globalThis.Array"), "isArray"),
            vec![ident("json")],
        )),
    );
    let pairs = as_expr(
        call(
            member(ident("globalThis.Object"), "entries"),
            vec![as_expr(ident("json"), index_signature_record_ty())],
        ),
        TsType::named("__JsonValue"),
    );
    let mut args = vec![pairs, ident("path")];
    if depth.is_some() {
        args.push(ident("depth"));
    }
    if_(is_object, block(vec![ret(call(ident(decoder), args))]))
}

/// #1887: the moved `bynk-tests` test `lenient_option_decode` pins the suffix a failed bare
/// `Option` value appends as a literal (1B.2b, #4028: it drives the driver's pipeline, so it
/// cannot read this private constant). This ties the two together.
#[cfg(test)]
mod option_forms_suffix_tests {
    #[test]
    fn option_forms_suffix_spelling() {
        assert_eq!(
            super::OPTION_FORMS_SUFFIX,
            " | null | {\"kind\": \"None\"} | {\"kind\": \"Some\", \"value\": ...}"
        );
    }
}

/// 1A.38a: the codec sites that were `TsStmt::raw` text, pinned as the text they emit.
#[cfg(test)]
mod converted_site_tests {
    use super::*;

    /// A wire-kind cascade nests each branch's guards at their true depth: inside the
    /// `if`, one level deeper than the `if` itself.
    #[test]
    fn the_wire_kind_cascade_nests_its_guards_at_their_true_depth() {
        let guard = |n: &str| {
            if_(
                strict_eq(ident(n), str_lit("x")),
                block(vec![ret(ident("fail"))]),
            )
        };
        let stmt = wire_kind_dispatch(
            "Ok",
            vec![guard("a")],
            ret(ident("ra")),
            "Err",
            vec![guard("b")],
            ret(ident("rb")),
        );
        assert_eq!(
            bynk_ts::print_stmt(&stmt, 1),
            "  if (obj[\"kind\"] === \"Ok\") {\n    if (a === \"x\") {\n      return fail;\n    }\n    return ra;\n  } else if (obj[\"kind\"] === \"Err\") {\n    if (b === \"x\") {\n      return fail;\n    }\n    return rb;\n  }\n"
        );
    }
}
