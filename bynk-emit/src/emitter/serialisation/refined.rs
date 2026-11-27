//! Refined scalars (back-half paydown 2.3b, #4088): the `Bytes` and `Decimal` named codecs, the refined
//! scalar codec and the inline refinement checks, with their tests. Moved out of `serialisation.rs`
//! byte for byte.

use std::sync::Arc;

use bynk_check::wire::{BaseGuard, Provenance, Revalidation, WireBody, WireType, wire_type};
use bynk_syntax::ast::{BaseType, PredKind, TypeDecl};
use bynk_ts::{TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt, TsType, TsTypeMember};

use crate::emitter::RuntimeUse;
use crate::ts_build::{
    as_expr, block, call, const_, ident, if_, member, not_expr, ret, str_lit, strict_eq,
    strict_neq, typeof_expr,
};

use super::support::{deserialise_path_param, err_structural_mismatch_top, json_kind_ts};
use super::{Qual, qual_provenance, qual_type};

/// v0.110 (ADR 0142 D5): the codec for a named opaque/refined type over
/// `Bytes` (`type Digest = Bytes`). Unlike the `number`-erased base types, a
/// `Bytes` does not round-trip as itself — it is base64-encoded on serialise
/// and decoded (rejecting a non-string or invalid-base64 wire value) on
/// deserialise. There are no `Bytes` refinement predicates, so there is no
/// `.of` re-validation to thread.
///
/// #1441 (Arc E slice 4): returns the `[serialise, deserialise]` pair as
/// real `bynk_ts::TsDecl` nodes instead of writing into an `out: &mut
/// String` — `emit_refined`'s own early return threads them straight
/// through unchanged (its only real caller, confirmed by grep), and
/// `emit_one` prints both at the boundary. A `(TsDecl, TsDecl)` pair would
/// say the same thing but `Vec<TsDecl>` is what the caller (`emit_one`,
/// looping to print+blank-line each declaration) actually wants to iterate,
/// and matches `emit_refined`'s own identical return shape below exactly —
/// one shape for both halves of this slice's call tree, not two.
fn emit_bytes_named_codec(name: &str, qual: &Qual, ru: &RuntimeUse) -> Vec<TsDecl> {
    ru.note_bytes();
    let ty = qual_type(qual, name);

    let serialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__serialise_{name}"),
        generics: Vec::new(),
        params: vec![TsParam::new(
            "value".to_string(),
            Some(TsType::named(ty.clone())),
        )],
        return_type: Some(TsType::named("__JsonValue")),
        body: vec![ret(call(
            ident("__bynkBytesToBase64"),
            vec![as_expr(
                as_expr(ident("value"), TsType::named("unknown")),
                TsType::named("globalThis.Uint8Array"),
            )],
        ))],
        is_async: false,
        inline: false,
    }));

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{name}"),
        generics: Vec::new(),
        params: vec![
            TsParam::new("json".to_string(), Some(TsType::named("__JsonValue"))),
            deserialise_path_param(),
        ],
        return_type: Some(TsType::named(format!("Result<{ty}, __BoundaryError>"))),
        body: vec![
            if_(
                strict_neq(typeof_expr(ident("json")), str_lit("string")),
                block(vec![ret(err_structural_mismatch_top(
                    "base64 string",
                    typeof_expr(ident("json")),
                ))]),
            ),
            const_(
                "__b",
                call(ident("__bynkBytesFromBase64"), vec![ident("json")]),
            ),
            if_(
                strict_eq(member(ident("__b"), "tag"), str_lit("None")),
                block(vec![ret(err_structural_mismatch_top(
                    "base64 string",
                    str_lit("invalid base64"),
                ))]),
            ),
            ret(call(
                ident("Ok"),
                vec![as_expr(
                    as_expr(member(ident("__b"), "value"), TsType::named("unknown")),
                    TsType::named(ty.clone()),
                )],
            )),
        ],
        is_async: false,
        inline: false,
    }));

    vec![serialise, deserialise]
}

/// #2622: the codec for a named type over `Decimal`. It serialises to the
/// canonical decimal text and deserialises by parsing that text (a non-string or
/// a malformed string is a `StructuralMismatch` at the path), then applies the
/// declared predicates: through the owner's `.of` for a type this module owns,
/// inline for a transparent type it consumes, not at all for a consumed opaque
/// type (whose predicate is the owner's secret, as for every other base).
fn emit_decimal_named_codec(
    name: &str,
    scalar: &bynk_check::wire::WireScalar,
    qual: &Qual,
    owned: bool,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    ru.note_decimal("__bynkDecimalToString");
    ru.note_decimal("__bynkDecimalParseWire");
    let ty = qual_type(qual, name);

    let serialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__serialise_{name}"),
        generics: Vec::new(),
        params: vec![TsParam::new(
            "value".to_string(),
            Some(TsType::named(ty.clone())),
        )],
        return_type: Some(TsType::named("__JsonValue")),
        body: vec![ret(as_expr(
            call(
                ident("__bynkDecimalToString"),
                vec![as_expr(
                    as_expr(ident("value"), TsType::named("unknown")),
                    TsType::named(crate::emitter::ts_types::DECIMAL_TS_TYPE),
                )],
            ),
            TsType::named("__JsonValue"),
        ))],
        is_async: false,
        inline: false,
    }));

    let mut body = vec![
        if_(
            strict_neq(typeof_expr(ident("json")), str_lit("string")),
            block(vec![ret(err_structural_mismatch_top(
                "decimal string",
                typeof_expr(ident("json")),
            ))]),
        ),
        const_(
            "__d",
            call(ident("__bynkDecimalParseWire"), vec![ident("json")]),
        ),
        if_(
            strict_eq(member(ident("__d"), "tag"), str_lit("None")),
            block(vec![ret(err_structural_mismatch_top(
                "decimal string",
                str_lit("invalid decimal"),
            ))]),
        ),
    ];
    let cast_ok = || {
        ret(call(
            ident("Ok"),
            vec![as_expr(
                as_expr(member(ident("__d"), "value"), TsType::named("unknown")),
                TsType::named(ty.clone()),
            )],
        ))
    };
    if scalar.predicates.is_empty() || (!owned && scalar.opaque) {
        body.push(cast_ok());
    } else if owned {
        // The owner's own `.of` applies the refinement.
        body.push(of_probe(name, member(ident("__d"), "value")));
        body.push(if_(
            strict_eq(member(ident("validated"), "tag"), str_lit("Err")),
            block(vec![ret(call(
                ident("Err"),
                vec![TsExpr::object_entries(vec![
                    TsObjectEntry::prop("kind".to_string(), str_lit("RefinementViolation")),
                    TsObjectEntry::Shorthand("path".to_string()),
                    TsObjectEntry::prop(
                        "violation".to_string(),
                        member(ident("validated"), "error"),
                    ),
                ])],
            ))]),
        ));
        body.push(ret(call(
            ident("Ok"),
            vec![as_expr(
                member(ident("validated"), "value"),
                TsType::named(name.to_string()),
            )],
        )));
    } else {
        // A consumed transparent type: the predicates inline, over the parsed
        // value, as the same `RefinementViolation` the owner's `.of` would raise.
        let field = ty.rsplit('.').next().unwrap_or(name);
        body.push(const_("__v", member(ident("__d"), "value")));
        for pred in bynk_syntax::ast::in_check_order(&scalar.predicates, |p| p) {
            let (cond, msg) = super::super::pred_condition_and_message_for(
                pred,
                "__v",
                Some(bynk_syntax::ast::BaseType::Decimal),
            );
            body.push(if_(
                not_expr(TsExpr::Paren(Box::new(cond))),
                block(vec![ret(call(
                    ident("Err"),
                    vec![TsExpr::object_entries(vec![
                        TsObjectEntry::prop("kind".to_string(), str_lit("RefinementViolation")),
                        TsObjectEntry::Shorthand("path".to_string()),
                        TsObjectEntry::prop(
                            "violation".to_string(),
                            TsExpr::object(vec![
                                ("field".to_string(), str_lit(field)),
                                (
                                    "message".to_string(),
                                    TsExpr::Lit(TsLit::Raw(format!("\"{msg}\""))),
                                ),
                                ("rule".to_string(), bynk_ts::pred_rule_expr(pred)),
                                ("value".to_string(), ident("json")),
                            ]),
                        ),
                    ])],
                ))]),
            ));
        }
        body.push(cast_ok());
    }

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{name}"),
        generics: Vec::new(),
        params: vec![
            TsParam::new("json".to_string(), Some(TsType::named("__JsonValue"))),
            deserialise_path_param(),
        ],
        return_type: Some(TsType::named(format!("Result<{ty}, __BoundaryError>"))),
        body,
        is_async: false,
        inline: false,
    }));

    vec![serialise, deserialise]
}

/// The runtime `.of` probe of a refined type's owner module:
///
/// ```text
/// const validated = (typeof (T as unknown as { of?: (json: unknown) => Result<unknown, ValidationError> }).of === "function")
///   ? (T as unknown as { of: (json: unknown) => Result<unknown, ValidationError> }).of(ARG)
///   : (Ok(ARG as unknown as T) as Result<unknown, ValidationError>);
/// ```
///
/// `T`'s declared type does not know about a conditional `.of` constructor (that is
/// why this probes at runtime), so the marker type states exactly the shape the
/// code depends on. `.error` is a `ValidationError` (a runtime interface), not
/// `unknown`, so the `RefinementViolation` built from it type-checks; the `else`
/// branch is annotated to match, so both arms unify to one `Result`.
fn of_probe(name: &str, arg: TsExpr) -> TsStmt {
    let result_ty = || {
        TsType::named_with_args(
            "Result",
            vec![TsType::named("unknown"), TsType::named("ValidationError")],
        )
    };
    let of_ty = |optional: bool| {
        let f = TsType::Fn {
            rest: false,
            params: vec![TsType::named("unknown")],
            names: vec!["json".to_string()],
            ret: Box::new(result_ty()),
        };
        TsType::Object(vec![if optional {
            TsTypeMember::optional_prop("of", f)
        } else {
            TsTypeMember::prop("of", f)
        }])
    };
    let cast = |optional: bool| {
        TsExpr::Paren(Box::new(as_expr(
            as_expr(ident(name), TsType::named("unknown")),
            of_ty(optional),
        )))
    };
    let test = TsExpr::Paren(Box::new(strict_eq(
        typeof_expr(member(cast(true), "of")),
        str_lit("function"),
    )));
    let present = call(member(cast(false), "of"), vec![arg.clone()]);
    let absent = TsExpr::Paren(Box::new(as_expr(
        call(
            ident("Ok"),
            vec![as_expr(
                as_expr(arg, TsType::named("unknown")),
                TsType::named(name),
            )],
        ),
        result_ty(),
    )));
    const_(
        "validated",
        TsExpr::multiline_conditional(test, present, absent),
    )
}

/// #855 (Phase 2 step 5): which of the four TS shapes a named scalar's codec
/// takes — owner `.of`, consumed-opaque structural cast, consumed-transparent
/// inline re-check, or the dedicated `Bytes` base64 codec — is now read off a
/// [`WireScalar`]'s [`Revalidation`], built once via
/// `bynk_check::wire::wire_type` from `decl` + this call's [`Provenance`],
/// instead of re-deriving `consumed`/`consumed_opaque`/`base == Bytes`
/// inline from `qual` + `decl.body` as this function used to. `wire_type`
/// only needs *Owned-vs-Consumed* + opaque-vs-transparent to pick a
/// `Revalidation` — the `owner_unit` string it carries otherwise is not
/// consumed by anything on this path (it exists for the Phase 4 peek), so
/// the qualifier prefix stands in for it here.
///
/// #1441 (Arc E slice 4): returns the `[serialise, deserialise]` pair as
/// real `bynk_ts::TsDecl` nodes — see [`emit_bytes_named_codec`]'s own doc
/// for why `Vec<TsDecl>`, not a `(TsDecl, TsDecl)` pair, and why the two
/// share one return shape. The `Revalidation::ViaConstructor` arm's runtime
/// `.of` probe (below) is [`of_probe`], real nodes: its inline object type
/// (`{ of?: (json: unknown) => Result<unknown, ValidationError> }`) is a
/// [`TsType::Fn`] with named parameters (1A.18b), and its three-line ternary a
/// [`TsExpr::multiline_conditional`] (1A.38a).
/// The JSON a refined or opaque value serialises to: the value itself, as its wire primitive.
/// #3939 (D2 D): an `ApiKeyValue` is a runtime `BynkSecret` whose text no generic path reads, so
/// its one encoding, inside the `ApiKeySecret` a handler returns from `mint`, reveals the key.
/// The checker refuses an `ApiKeyValue` at every other boundary.
fn serialised_scalar(decl: &TypeDecl, prim: &str) -> bynk_ts::TsExpr {
    if bynk_check::context_checks::secret_kind(decl)
        == Some(bynk_check::context_checks::SecretKind::ApiKeyValue)
    {
        let revealer = TsType::Object(vec![bynk_ts::TsTypeMember::method(
            "reveal",
            Vec::new(),
            TsType::named("string"),
        )]);
        return crate::ts_build::method_call(
            crate::ts_build::paren(as_expr(
                as_expr(ident("value"), TsType::named("unknown")),
                revealer,
            )),
            "reveal",
            Vec::new(),
        );
    }
    as_expr(
        as_expr(ident("value"), TsType::named("unknown")),
        TsType::named(prim),
    )
}

pub(super) fn emit_refined(
    name: &str,
    decl: &TypeDecl,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    let ty = qual_type(qual, name);
    let prov = qual_provenance(qual, name);
    let owned = matches!(prov, Provenance::Owned);
    let scalar = match wire_type(name, decl, types, prov) {
        Some(WireType {
            body: WireBody::Scalar(s),
            ..
        }) => s,
        _ => unreachable!(
            "emit_refined is only ever called for a non-generic Refined/Opaque declaration"
        ),
    };

    // #2622: a `Decimal`-based opaque/refined type has a bespoke codec: the
    // wire value is a decimal string, parsed (never cast) and then checked
    // against the declared predicates.
    if scalar.revalidation == Revalidation::DecimalParse {
        return emit_decimal_named_codec(name, &scalar, qual, owned, ru);
    }

    // v0.110: a `Bytes`-based opaque/refined type has a bespoke base64 codec —
    // `emit_refined`'s early return to it, mirrored from `Revalidation` rather
    // than a bare `base == BaseType::Bytes` check.
    if scalar.revalidation == Revalidation::Base64Decode {
        return emit_bytes_named_codec(name, qual, ru);
    }

    // #661: a *consumed* type (one the caller qualifies through the callee's
    // type-only namespace) has no importable `.of`, so its deserialiser cannot
    // route validation through the owner's constructor. An **opaque** consumed
    // type casts structurally after the base check (Decision C — its predicate
    // is the owner's secret and is not re-checked, which is sound because the
    // value was produced by the owner's typed code and skew is caught by the
    // v0.177 contract hash). A **transparent refined** consumed type inlines
    // its predicate checks (Decision D — the consumer knows the shape by
    // declaration, so it validates, just not through `.of`).
    let prim = json_kind_ts(scalar.json);
    let typeof_str = prim;

    let serialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__serialise_{name}"),
        generics: Vec::new(),
        params: vec![TsParam::new(
            "value".to_string(),
            Some(TsType::named(ty.clone())),
        )],
        return_type: Some(TsType::named("__JsonValue")),
        body: vec![ret(serialised_scalar(decl, prim))],
        is_async: false,
        inline: false,
    }));

    let mut body = vec![if_(
        strict_neq(typeof_expr(ident("json")), str_lit(typeof_str)),
        block(vec![ret(err_structural_mismatch_top(
            typeof_str,
            typeof_expr(ident("json")),
        ))]),
    )];
    match scalar.revalidation {
        Revalidation::StructuralOnly => {
            // Decision C: structural cast only — never reach for the owner's
            // `.of`, which would resurrect the value import this increment
            // removes and leak the opaque predicate into the consumer.
            body.push(ret(call(
                ident("Ok"),
                vec![as_expr(
                    as_expr(ident("json"), TsType::named("unknown")),
                    TsType::named(ty.clone()),
                )],
            )));
        }
        Revalidation::Inline => {
            // Decision D: a transparent refined consumed type validates inline.
            // The base-integrality / finiteness guards and the declared
            // predicates, in the same order the owner's `.of` applies them, but
            // wrapped as this codec's `BoundaryError` rather than a
            // `ValidationError`.
            // #1846: a renamed unexported type reports its declared name,
            // the one its owner's `.of` reports (`Code`, not `t_vault__Code`).
            let field = ty.rsplit('.').next().unwrap_or(name);
            body.extend(emit_inline_refinement_checks(
                field,
                &scalar.base_guards,
                &scalar.predicates,
            ));
            body.push(ret(call(
                ident("Ok"),
                vec![as_expr(
                    as_expr(ident("json"), TsType::named("unknown")),
                    TsType::named(ty.clone()),
                )],
            )));
        }
        Revalidation::ViaConstructor => {
            // Owner's own module: re-validate via the type's own constructor
            // (`.of`), which applies the refinement. If the type has no
            // refinement, `.of` doesn't exist for refined-base types; fall back
            // to a direct cast.
            // The `.of` probe, [`of_probe`]: `name`'s declared type does not know of a
            // conditional `.of`, which is why it probes at runtime.
            body.push(of_probe(name, ident("json")));
            body.push(if_(
                strict_eq(member(ident("validated"), "tag"), str_lit("Err")),
                block(vec![ret(call(
                    ident("Err"),
                    vec![TsExpr::object_entries(vec![
                        TsObjectEntry::prop("kind".to_string(), str_lit("RefinementViolation")),
                        TsObjectEntry::Shorthand("path".to_string()),
                        TsObjectEntry::prop(
                            "violation".to_string(),
                            member(ident("validated"), "error"),
                        ),
                    ])],
                ))]),
            ));
            body.push(ret(call(
                ident("Ok"),
                vec![as_expr(
                    member(ident("validated"), "value"),
                    TsType::named(name.to_string()),
                )],
            )));
        }
        Revalidation::Base64Decode | Revalidation::DecimalParse => {
            unreachable!("handled by the early return above")
        }
    }

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{name}"),
        generics: Vec::new(),
        params: vec![
            TsParam::new("json".to_string(), Some(TsType::named("__JsonValue"))),
            deserialise_path_param(),
        ],
        return_type: Some(TsType::named(format!("Result<{ty}, __BoundaryError>"))),
        body,
        is_async: false,
        inline: false,
    }));

    vec![serialise, deserialise]
}

/// #661 (Decision D): inline the base-type and refinement checks a consumed
/// **transparent** refined type's deserialiser applies, reporting failures as
/// this codec's `BoundaryError` (`RefinementViolation` wrapping the same
/// `{ field, message, value }` the owner's `.of` would have produced). The
/// `typeof` guard is emitted by the caller; these are the checks that run once
/// the primitive type already matched. `json` is the value being validated.
/// #855 (Phase 2 step 5): driven off a [`WireScalar`]'s `base_guards` +
/// `predicates` (both **declaration order** — see `wire.rs`'s module doc)
/// rather than a `BaseType` + raw `Option<&Refinement>` pair.
///
/// #1441 (Arc E slice 4): returns a real `Vec<bynk_ts::TsStmt>` — the
/// statement-builder shape #1439 already established for
/// `emit_field_deserialise_wire` — feeding straight into `emit_refined`'s
/// own `Revalidation::Inline` arm, rather than writing into an `out: &mut
/// String`. `violation` returns a `TsStmt` (a `return Err(...)` statement)
/// instead of the pre-conversion `String`, for the same reason: both real
/// call sites (the base-guard loop below, `emit_inline_pred_check`'s own
/// loop) place it directly inside a real `block(vec![...])`.
fn emit_inline_refinement_checks(
    name: &str,
    base_guards: &[BaseGuard],
    predicates: &[PredKind],
) -> Vec<TsStmt> {
    // Review of #1442, finding 1: `msg` reaches here two ways — a static,
    // hand-written literal from the base-guard arms below (never needs
    // escaping) and, via `emit_inline_pred_check`, `pred_condition_and_
    // message`'s own `PredKind::Matches` message, which is ALREADY run
    // through `escape_ts_string` (that function's own doc names both
    // callers' shared "splice unescaped" contract). `str_lit`'s own
    // `TsLit::Str` re-escapes `\`/`"` unconditionally, so passing an
    // already-escaped `Matches` message through it doubled every backslash
    // (`must match /\d{4}/` → `must match /\\d{4}/`) — a real, silent
    // output change no fixture caught (no fixture reaches this predicate
    // through the inline-consumer path with a backslash/quote in its
    // pattern). `TsLit::Raw` (pre-quoted, no further escaping) matches
    // `pred_condition_and_message`'s own contract exactly, and is a no-op
    // for the base-guard arms' own escaping-free text.
    let violation = |msg: &str, rule: TsExpr| -> TsStmt {
        ret(call(
            ident("Err"),
            vec![TsExpr::object_entries(vec![
                TsObjectEntry::prop("kind".to_string(), str_lit("RefinementViolation")),
                TsObjectEntry::Shorthand("path".to_string()),
                TsObjectEntry::prop(
                    "violation".to_string(),
                    TsExpr::object(vec![
                        ("field".to_string(), str_lit(name)),
                        (
                            "message".to_string(),
                            TsExpr::Lit(TsLit::Raw(format!("\"{msg}\""))),
                        ),
                        ("rule".to_string(), rule),
                        ("value".to_string(), ident("json")),
                    ]),
                ),
            ])],
        ))
    };
    // Base guards mirror `emit_refined_checks`: an `Int` is whole, a `Float`
    // finite. (`Duration`/`Instant` are not exposed as named refined bases.)
    let mut stmts = Vec::new();
    for guard in base_guards {
        match guard {
            // #1657: an `Int` is a JS safe integer, so `1e300` is rejected too.
            BaseGuard::Integral => stmts.push(if_(
                not_expr(call(
                    member(ident("globalThis.Number"), "isSafeInteger"),
                    vec![ident("json")],
                )),
                block(vec![violation(
                    "must be a safe integer",
                    bynk_ts::well_formed_rule_expr(),
                )]),
            )),
            BaseGuard::Finite => stmts.push(if_(
                not_expr(call(
                    member(ident("globalThis.Number"), "isFinite"),
                    vec![ident("json")],
                )),
                block(vec![violation(
                    "must be a finite number",
                    bynk_ts::well_formed_rule_expr(),
                )]),
            )),
        }
    }
    for pred in bynk_syntax::ast::in_check_order(predicates, |p| p) {
        stmts.extend(emit_inline_pred_check(pred, &violation));
    }
    stmts
}

/// #1969: a record field's inline refinement, checked on the decoded field
/// (`__<field>`, which a refined field's codec leaves as its erased base
/// value). One `if (!…) return Err(RefinementViolation)` per predicate, in the
/// same check order and with the same messages as a named refined type's, at
/// the field's own path (`path`, a template literal).
pub(super) fn emit_field_refinement_checks(
    fname: &str,
    predicates: &[PredKind],
    path: &TsExpr,
    base: Option<BaseType>,
    ru: &RuntimeUse,
) -> Vec<TsStmt> {
    let value = format!("__{fname}");
    // #2622: a `Decimal` violation reports its canonical text; the decoded
    // value holds a `bigint`, which cannot be JSON-stringified.
    let shown = if base == Some(BaseType::Decimal) {
        ru.note_decimal("__bynkDecimalToString");
        call(ident("__bynkDecimalToString"), vec![ident(value.clone())])
    } else {
        ident(value.clone())
    };
    let violation = |msg: &str, rule: TsExpr| -> TsStmt {
        ret(call(
            ident("Err"),
            vec![TsExpr::object(vec![
                ("kind".to_string(), str_lit("RefinementViolation")),
                ("path".to_string(), path.clone()),
                (
                    "violation".to_string(),
                    TsExpr::object(vec![
                        ("field".to_string(), str_lit(fname)),
                        (
                            "message".to_string(),
                            TsExpr::Lit(TsLit::Raw(format!("\"{msg}\""))),
                        ),
                        ("rule".to_string(), rule),
                        ("value".to_string(), shown.clone()),
                    ]),
                ),
            ])],
        ))
    };
    let mut stmts = Vec::new();
    for pred in bynk_syntax::ast::in_check_order(predicates, |p| p) {
        let (cond, msg) = super::super::pred_condition_and_message_for(pred, &value, base);
        stmts.push(if_(
            not_expr(TsExpr::Paren(Box::new(cond))),
            block(vec![violation(&msg, bynk_ts::pred_rule_expr(pred))]),
        ));
    }
    stmts
}

/// #661 (Decision D): one refinement predicate as an inline `if (!…) return
/// Err(…)`, over the local `json` binding. The messages match
/// `emit::emit_pred_check` so a consumer-side rejection reads identically to an
/// owner-side one; only the error envelope differs (`BoundaryError` here,
/// `ValidationError` there).
///
/// #1441 (Arc E slice 4): returns `Vec<TsStmt>` (one `if`), same treatment
/// as [`emit_inline_refinement_checks`] above. #1471: `super::pred_
/// condition_and_message`'s own `cond` is now a real `TsExpr` (a
/// `Binary`/`Call` node), not opaque text spliced through [`ident`] — still
/// wrapped in an explicit [`TsExpr::Paren`] before negating, byte-identical
/// to the pre-#1471 output: the wrap was already unconditional (never
/// derived from `cond`'s own shape), so a real, already-well-formed node
/// underneath changes nothing about whether the surrounding parens print.
fn emit_inline_pred_check(
    pred: &PredKind,
    violation: &dyn Fn(&str, TsExpr) -> TsStmt,
) -> Vec<TsStmt> {
    let (cond, msg) = super::super::pred_condition_and_message(pred, "json");
    vec![if_(
        not_expr(TsExpr::Paren(Box::new(cond))),
        block(vec![violation(&msg, bynk_ts::pred_rule_expr(pred))]),
    )]
}

/// Review of #1442, finding 1/2: direct coverage for
/// [`emit_inline_refinement_checks`]/[`emit_inline_pred_check`], the two
/// branches no fixture exercises. `matches_predicate_with_a_backslash_is_
/// not_double_escaped` pins the actual regression (a `Matches` message
/// already run through `escape_ts_string` was silently re-escaped by
/// `str_lit`'s own `TsLit::Str`); `base_guard_finite_renders` closes the
/// coverage gap the review named alongside it (`Number.isFinite(json)`
/// appears in zero fixtures, unlike its `Integral` sibling).
#[cfg(test)]
mod emit_inline_refinement_checks_tests {
    use super::*;

    fn render(stmts: Vec<TsStmt>) -> String {
        stmts
            .iter()
            .map(|s| bynk_ts::print_stmt(s, 1))
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn matches_predicate_with_a_backslash_is_not_double_escaped() {
        // Pattern `\d{4}` (one real backslash) — `escape_ts_string` escapes
        // it to a valid JS string literal, `\\d{4}` (two real backslash
        // characters, correct and final); the bug re-escaped that already-
        // escaped text through `TsLit::Str`, doubling it again to `\\\\d{4}`
        // (four).
        let stmts =
            emit_inline_refinement_checks("name", &[], &[PredKind::Matches(r"\d{4}".to_string())]);
        let out = render(stmts);
        assert!(
            out.contains(r"must match /\\d{4}/"),
            "expected exactly one escaping round in the message, got: {out}"
        );
        assert!(
            !out.contains(r"must match /\\\\d{4}/"),
            "message was double-escaped: {out}"
        );
    }

    #[test]
    fn base_guard_finite_renders() {
        let stmts = emit_inline_refinement_checks("name", &[BaseGuard::Finite], &[]);
        assert_eq!(
            render(stmts),
            "  if (!globalThis.Number.isFinite(json)) {\n    return Err({ kind: \"RefinementViolation\", path, violation: { field: \"name\", message: \"must be a finite number\", rule: { tag: \"WellFormed\" }, value: json } });\n  }\n"
        );
    }
}

/// 1A.38a: the codec sites that were `TsStmt::raw` text, pinned as the text they emit.
#[cfg(test)]
mod converted_site_tests {
    use super::*;

    /// The refined-type `.of` probe prints as the three-line ternary it always was,
    /// at the depth-1 indent of a codec body.
    #[test]
    fn the_of_probe_prints_as_its_three_line_ternary() {
        let stmt = of_probe("Percent", ident("json"));
        assert_eq!(
            bynk_ts::print_stmt(&stmt, 1),
            "  const validated = (typeof (Percent as unknown as { of?: (json: unknown) => Result<unknown, ValidationError> }).of === \"function\")\n    ? (Percent as unknown as { of: (json: unknown) => Result<unknown, ValidationError> }).of(json)\n    : (Ok(json as unknown as Percent) as Result<unknown, ValidationError>);\n"
        );
    }

    /// The Decimal codec's probe takes the parsed value, not `json`, in both of the
    /// places the argument appears.
    #[test]
    fn the_of_probe_threads_its_argument() {
        let stmt = of_probe("Money", member(ident("__d"), "value"));
        let text = bynk_ts::print_stmt(&stmt, 1);
        assert!(text.contains(".of(__d.value)\n"), "{text}");
        assert!(text.contains("Ok(__d.value as unknown as Money)"), "{text}");
    }
}
