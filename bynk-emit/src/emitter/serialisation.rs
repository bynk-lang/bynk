//! Per-type serialise / deserialise helper generation for workers mode
//! (v0.8 §3.4 / §5.2).
//!
//! Every Bynk type that crosses a context boundary needs:
//!   - `serialise_<Type>(value): JsonValue` — structural lowering.
//!   - `deserialise_<Type>(json): Result<<Type>, BoundaryError>` —
//!     structural validation + refinement re-validation, then a nominal
//!     cast back to the receiving context's view.
//!
//! Helpers live in the *owning* module — commons modules emit helpers for
//! commons types, context modules emit helpers for the types they declare.

mod expr;
mod field;
mod support;

use field::{
    emit_field_deserialise, emit_field_deserialise_wire, serialise_field_expr,
    serialise_field_expr_wire,
};
use support::{
    Depth, DepthCodecs, array_shape_guard, boundary_result, case_, default_case, depth_expected,
    depth_for, depth_guard_decode, depth_guard_encode, depth_param, deserialise_params,
    deserialise_path_param, err_structural_mismatch, err_structural_mismatch_top,
    index_signature_record_ty, json_kind_ts, obj_prop, object_shape_guard, path_elem, path_entry,
    path_field, serialise_params, value_prop,
};

#[cfg(test)]
use bynk_check::wire::JsonKind;

pub(crate) use expr::{
    agent_wire_passes_through, deserialise_expr, deserialise_expr_via, peel_list_option,
    serialise_expr, serialise_ref_via, witness_codec_expr,
};
pub use expr::{deserialise_ref_via, serialise_expr_via};

use std::sync::Arc;

use bynk_syntax::ast::{BaseType, PredKind, TypeBody, TypeDecl, TypeRef};

use crate::emitter::default_value::wire_default_node;
use crate::emitter::{RuntimeUse, payload_prop};
use crate::ts_build::{
    and_expr, arrow, as_expr, binary, block, call, const_, ident, if_, index, member, not_expr,
    or_expr, ret, str_lit, strict_eq, strict_neq, switch_stmt, typeof_expr,
};
use bynk_check::wire_default::field_default_value;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt,
    TsSwitchCase, TsType, TsTypeMember,
};

// -- Helpers over the shared builders (`crate::ts_build`) ----------------------

/// `(v) => <serialise v>`: the element serialiser a `List` or `Set` codec maps over.
fn element_serialiser(elem: &TypeRef, depth: Option<Depth<'_>>, ru: &RuntimeUse) -> TsExpr {
    arrow(
        vec![TsParam::new("v", None)],
        serialise_field_expr(elem, &ident("v"), depth, ru),
    )
}

/// #661: a *type qualifier* — maps a callee-owned type name to the type-only
/// namespace prefix (`"commerce_payment."`) the caller must use to *name* it,
/// while the caller generates that type's codec **locally** under a bare name.
///
/// A name absent from the map (or mapped to `""`) is named bare: the owner's
/// own module, a base/generic type, or a commons type the caller already
/// declares or imports locally. Only a consumed context's *own* boundary types
/// (`AuthId`, `PaymentError`) are qualified — the caller has no local
/// declaration to name, so the codec's type positions reach through the
/// `import type * as <ns>` alias. Codec *function* names are never qualified:
/// the caller's `deserialise_AuthId` calls its own local `deserialise_*`, which
/// is the whole point of the increment.
///
/// #1846: a value with no trailing `.` is a whole qualified name instead
/// (`"t_vault.Code"`), for a callee's unexported type the caller renamed;
/// see [`qual_type`].
type Qual = std::collections::HashMap<String, String>;

/// #1437 (Arc E slice 2): [`crate::emitter::ts_types::ts_type_ref_qualified_multi_ts_type`]
/// as a real `TsType`, over this file's own dotted-prefix [`Qual`] convention
/// — a real, empirically-found convention mismatch, not assumed compatible
/// from the two functions' identical `HashMap<String, String>` shapes alone.
/// `Qual`'s own values already carry the trailing separator (`"shop_payment."`,
/// this type's own doc), concatenated directly by [`qual_type`]; the
/// existing renderer's own qualify closure (`emitter.rs`'s `ts_type_ref_to_ts_type`,
/// `TypeRef::Named`'s arm) instead appends its OWN `.` (`format!("{ns}.{}",
/// id.name)`), so passing `Qual` through unmodified doubles the separator
/// (`shop_payment..PayError`) — caught by `coverage_behaviour.rs`'s real `tsc
/// --strict` run (`TS1003: Identifier expected`), not by the byte-golden
/// fixture corpus, which has no consumed-context boundary type reaching this
/// exact generic-instantiation path. An absent/empty `Qual` entry (this
/// type's own "named bare" case) maps to `None` here, not `Some("")` — the
/// latter would render a stray leading `.` the same way a non-empty value's
/// doubled separator does. `pub(crate)`, not private: `emitter/lower.rs`'s
/// own `Json.decode[T]` test-scaffold arm is this file's one external
/// caller, replacing its own direct call to the now-deleted
/// `ts_type_ref_qualified`.
pub(crate) fn qualified_ts_type(t: &TypeRef, qual: &Qual) -> bynk_ts::TsType {
    // Review of #1438: the deleted `ts_inner_type` carried an explicit
    // `unreachable!()` for these five variants (`bynk.types.
    // function_at_boundary` rejects them before the serialisation machinery
    // ever sees one); `ts_type_ref_qualified_multi_ts_type` has no such arm
    // (it also serves non-boundary positions, where all five are legal), so
    // folding into it silently turned a loud panic into a plausible-looking
    // annotation next to a `serialise_*`/`deserialise_*` helper that cannot
    // codec it. Unreachable today, made loud rather than silently relied on
    // — the same precedent this crate's own unreachable-invariant
    // `debug_assert!`s already set (e.g. the empty-args `TypeRef::App` one
    // in `emitter.rs`).
    debug_assert!(
        !matches!(
            t,
            TypeRef::Fn(..)
                | TypeRef::Query(..)
                | TypeRef::Stream(..)
                | TypeRef::Connection(..)
                | TypeRef::History(..)
        ),
        "function/query/stream types are rejected at boundaries"
    );
    let bare: Qual = qual
        .iter()
        .filter(|(_, v)| v.ends_with('.'))
        .filter_map(|(k, v)| {
            let ns = v.trim_end_matches('.');
            (!ns.is_empty()).then(|| (k.clone(), ns.to_string()))
        })
        .collect();
    // #1846: a whole-name entry replaces the name outright (see [`qual_type`]).
    let whole: std::collections::HashMap<String, String> = qual
        .iter()
        .filter(|(_, v)| !v.is_empty() && !v.ends_with('.'))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if whole.is_empty() {
        crate::emitter::ts_types::ts_type_ref_qualified_multi_ts_type(t, &bare)
    } else {
        crate::emitter::ts_types::ts_type_ref_qualified_multi_ts_type(
            &rename_type_ref(t, &whole),
            &bare,
        )
    }
}

#[cfg(test)]
mod qualified_ts_type_tests {
    use super::*;
    use bynk_syntax::ast::Ident;
    use bynk_syntax::span::Span;

    fn named(name: &str) -> TypeRef {
        TypeRef::Named(Ident {
            name: name.to_string(),
            span: Span::new(0, 0),
        })
    }

    /// Review of #1438, finding 2: the exact regression this slice
    /// introduced and fixed (`shop_payment..PayError`, a doubled
    /// separator) — pinned directly, not left to depend on a real `tsc`
    /// run in `bynk-tests/tests/coverage_behaviour.rs`.
    #[test]
    fn dotted_qual_entry_renders_a_single_separator() {
        let qual: Qual = [("PayError".to_string(), "shop_payment.".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            bynk_ts::print_type(&qualified_ts_type(&named("PayError"), &qual)),
            "shop_payment.PayError"
        );
    }

    #[test]
    fn absent_qual_entry_renders_bare() {
        let qual: Qual = Qual::new();
        assert_eq!(
            bynk_ts::print_type(&qualified_ts_type(&named("PayError"), &qual)),
            "PayError"
        );
    }

    /// An empty-string entry is `Qual`'s own "named bare" convention (this
    /// type's own doc) — must map to `None`, not `Some("")`, or it renders
    /// a stray leading `.`.
    #[test]
    fn empty_qual_entry_renders_bare_not_a_stray_dot() {
        let qual: Qual = [("PayError".to_string(), String::new())]
            .into_iter()
            .collect();
        assert_eq!(
            bynk_ts::print_type(&qualified_ts_type(&named("PayError"), &qual)),
            "PayError"
        );
    }
}

/// The namespace prefix for a type name under `qual` (`""` when unqualified).
/// The TS type a codec for `name` names. A [`Qual`] value is either a
/// namespace prefix ending in `.` (`"t_vault."`, giving `t_vault.Code`) or,
/// #1846, a whole qualified name with no trailing `.` (`"t_vault.Code"`): the
/// form a callee's *unexported* type takes, renamed in the caller's copy of
/// the callee's table (`t_vault__Code`) so its codecs can't collide with a
/// caller type of the same name, while its TS type stays the callee's own.
fn qual_type(qual: &Qual, name: &str) -> String {
    match qual.get(name) {
        Some(v) if v.ends_with('.') => format!("{v}{name}"),
        Some(v) if !v.is_empty() => v.clone(),
        _ => name.to_string(),
    }
}

/// [`Provenance::Consumed`] for a name [`Qual`] qualifies, either form, owned
/// by the namespace's unit; [`Provenance::Owned`] otherwise.
fn qual_provenance(qual: &Qual, name: &str) -> Provenance {
    match qual.get(name) {
        Some(v) if !v.is_empty() => Provenance::Consumed {
            owner_unit: v.split('.').next().unwrap_or_default().to_string(),
        },
        _ => Provenance::Owned,
    }
}

// #855 (Phase 1): `collect_boundary_types`, `collect_type_names`,
// `recursive_generic_names`, `subst_type_ref`, `record_inst_fields`,
// `sum_inst_variants`, and `app_ts_name` moved to `bynk-check`'s wire IR
// (`bynk_check::wire`) as pure, AST-only walks with no TS emission — see that
// module's doc comment for the seam. Re-exported here under their original
// names (and, for `app_ts_name`, its original signature under an alias to
// its new `inst_codec_suffix` name) so every call site in this file and
// elsewhere in `bynk-emit` keeps compiling unchanged; `recursive_generic_names`,
// `collect_type_names`, and `subst_type_ref` had no callers outside the
// functions that moved with them, so they are not re-exported.
pub(crate) use bynk_check::wire::collect_boundary_types;
pub(crate) use bynk_check::wire::inst_codec_suffix as app_ts_name;
pub(crate) use bynk_check::wire::record_inst_fields;
pub(crate) use bynk_check::wire::rename_type_ref;
pub(crate) use bynk_check::wire::sum_inst_variants;

// #855 (Phase 2 step 5): the scalar-codec decision vocabulary — which TS
// branch `emit_refined` takes is now read off a `WireScalar`'s
// `Revalidation` (built via `wire_type`) rather than re-derived inline from
// `qual`/`decl.body`. `json_kind_of` replaces `ts_base_for_serialisation`'s
// *classification*; this file keeps the TS-token spelling (`json_kind_ts`,
// below) per the seam in `wire.rs`'s module doc.
use bynk_check::wire::{
    BaseGuard, Provenance, Revalidation, WireBody, WireField, WireRef, WireSum, WireType,
    WireVariant, recursive_codec_suffixes, wire_ref, wire_type,
};

/// #855 (Phase 2 step 6): resolve one `TypeRef` occurrence to its [`WireRef`]
/// shape for `emit_field_deserialise` / `serialise_field_expr_via`. `wire_ref`
/// documents its `types` parameter as unconsulted (single-level resolution,
/// kept only for signature symmetry with the transitive walks) — an empty
/// table costs nothing (`HashMap::new()` does not allocate) and avoids
/// threading a real one through `emit_record_codec` / `emit_sum_codec` /
/// `emit_generic_helpers_qualified`, which is out of scope for this step
/// (records/sums/generic-helpers are steps 7/8/9).
fn wire_ref_of(t: &TypeRef) -> WireRef {
    wire_ref(t, &std::collections::HashMap::new())
}

/// Emit `serialise_<T>` and `deserialise_<T>` for every named type the
/// owner declares that crosses a boundary. `owner_qualified` is the
/// qualified name used as the brand path so that refinement-violation
/// messages identify the origin context.
pub(crate) fn emit_helpers_for_owner(
    type_names: &[String],
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    owner_qualified: &str,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    emit_helpers_for_owner_qualified(type_names, types, owner_qualified, &Qual::new(), ru)
}

/// Collects `decls` into real statements — the shared shape every caller of
/// `emit_helpers_for_owner`/`emit_generic_helpers` (and their
/// `_qualified` twins) collects into its own returned `Vec<TsStmt>`. Arc F
/// slice 1 (#1451): consolidates what used to be ten separate `write!`-family
/// call sites into this one; #1478 made it the one real implementation
/// (dropping a hand-duplicated `out: &mut String`-printing sibling); #1479
/// removed that sibling's own last two production callers (`project/
/// tests_emit.rs`'s codec-helper glue), so every caller in the crate is on
/// this `Vec<TsStmt>` form directly now. #1486: no more explicit blank
/// `TsStmt` after each declaration — none of these are import/comment-kind,
/// so the printer's automatic top-level spacing policy already supplies
/// exactly one blank between every adjacent pair (and after the last one,
/// before whatever follows) once a real caller prints through it.
pub fn decls_as_stmts(decls: Vec<TsDecl>) -> Vec<TsStmt> {
    decls.into_iter().map(|d| TsStmt::decl(d, None)).collect()
}

/// As [`decls_as_stmts`], plus one *extra* forced trailing blank `TsStmt`
/// when anything was collected — the shared `if emitted_any { ... }`
/// trailer every `emit_helpers_for_owner`/[`emit_helpers_for_owner_
/// qualified`] caller needs. Under the pre-#1486 flat-print regime this
/// trailer's own blank stacked on top of `decls_as_stmts`'s own per-decl
/// blank (one `TsStmt::blank` after every decl, including the last),
/// giving this group's own tail *two* blank lines total — a genuine,
/// stronger "owner group" section break (confirmed against
/// `118_workers_target_with_consume`'s own real fixture output: the
/// `local_boundary` helpers group and the following `by_commons`
/// re-export group are separated by two blank lines, not one), not a
/// redundant duplicate of the ordinary one-blank-between-decls spacing
/// [`decls_as_stmts`] now gets for free from the printer's automatic
/// policy. `no_blank_before` on the extra blank itself suppresses *its
/// own* automatic lead-in (which would otherwise stack with the ordinary
/// one the policy already supplies between the last real decl and this
/// blank), leaving exactly one blank line from this statement's own
/// render plus the policy's own ordinary blank before whatever follows —
/// two lines total, matching the pre-#1486 text exactly.
pub fn decls_as_stmts_block(decls: Vec<TsDecl>) -> Vec<TsStmt> {
    let emitted_any = !decls.is_empty();
    let mut stmts = decls_as_stmts(decls);
    if emitted_any {
        let mut extra_blank = TsStmt::blank(None);
        extra_blank.no_blank_before = true;
        stmts.push(extra_blank);
    }
    stmts
}

/// #661: as `emit_helpers_for_owner`, but the caller supplies a type
/// `Qual`. With an empty qualifier this is the owner's own module (every
/// type named bare, refined validation through `.of`). With a non-empty one it
/// is a *consumer* generating its own view of another context's boundary
/// types: the qualified names reach through the `import type * as <ns>` alias,
/// and refined validation inlines (transparent) or casts structurally (opaque)
/// because the owner's `.of` is not importable.
pub fn emit_helpers_for_owner_qualified(
    type_names: &[String],
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    _owner_qualified: &str,
    qual: &Qual,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    // Only emit helpers for *named* types declared by this owner. Skip
    // unknown names — they belong to another module or to the runtime's
    // generic helpers (Result / Option).
    let mut decls = Vec::new();
    let codecs = recursive_codec_suffixes(types);
    for name in type_names {
        let Some(decl) = types.get(name) else {
            continue;
        };
        // v0.174 (#592): a generic record has no single `serialise_<Name>` —
        // each boundary instantiation gets its own monomorphised codec
        // (`serialise_Paginated_User`) via `emit_generic_helpers`. Never emit a
        // bare, un-parameterised helper for the declaration itself.
        if !decl.type_params.is_empty() {
            continue;
        }
        // K-streams (#2623): a type carrying a `Stream` has no codec. The
        // checker keeps it off every boundary, so nothing calls one.
        if bynk_syntax::ast::type_carries_stream(name, types) {
            continue;
        }
        decls.extend(emit_one(name, decl, types, qual, &codecs, ru));
    }
    decls
}

/// Arc F slice 1 (#1451): returns the real `bynk_ts::TsDecl` nodes for
/// `name` directly — no `out: &mut String` parameter. The boundary-collect
/// step ([`decls_as_stmts`]) now happens once, at the caller.
fn emit_one(
    name: &str,
    decl: &TypeDecl,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    match &decl.body {
        // #1441 (Arc E slice 4): `emit_refined` itself now builds real
        // `bynk_ts::TsDecl` nodes.
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => {
            emit_refined(name, decl, types, qual, ru)
        }
        // #1443 (Arc E slice 5): `emit_record` itself now builds real
        // `bynk_ts::TsDecl` nodes.
        TypeBody::Record(_) => emit_record(name, decl, types, qual, codecs, ru),
        // #1445 (Arc E slice 6): `emit_sum` itself now builds real
        // `bynk_ts::TsDecl` nodes.
        TypeBody::Sum(_) => emit_sum(name, decl, types, qual, codecs, ru),
    }
}

/// `(globalThis as { __bynkPlainJson?: boolean }).__bynkPlainJson === true`: whether `withPlainJson`
/// has the plain-JSON flag set. The cast prints bracketed as the member's receiver.
fn plain_json_test() -> TsExpr {
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
            let (cond, msg) = super::pred_condition_and_message_for(
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
fn emit_refined(
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
        body: vec![ret(as_expr(
            as_expr(ident("value"), TsType::named("unknown")),
            TsType::named(prim),
        ))],
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
    let violation = |msg: &str| -> TsStmt {
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
                block(vec![violation("must be a safe integer")]),
            )),
            BaseGuard::Finite => stmts.push(if_(
                not_expr(call(
                    member(ident("globalThis.Number"), "isFinite"),
                    vec![ident("json")],
                )),
                block(vec![violation("must be a finite number")]),
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
fn emit_field_refinement_checks(
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
    let violation = |msg: &str| -> TsStmt {
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
                        ("value".to_string(), shown.clone()),
                    ]),
                ),
            ])],
        ))
    };
    let mut stmts = Vec::new();
    for pred in bynk_syntax::ast::in_check_order(predicates, |p| p) {
        let (cond, msg) = super::pred_condition_and_message_for(pred, &value, base);
        stmts.push(if_(
            not_expr(TsExpr::Paren(Box::new(cond))),
            block(vec![violation(&msg)]),
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
fn emit_inline_pred_check(pred: &PredKind, violation: &dyn Fn(&str) -> TsStmt) -> Vec<TsStmt> {
    let (cond, msg) = super::pred_condition_and_message(pred, "json");
    vec![if_(
        not_expr(TsExpr::Paren(Box::new(cond))),
        block(vec![violation(&msg)]),
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
            "  if (!globalThis.Number.isFinite(json)) {\n    return Err({ kind: \"RefinementViolation\", path, violation: { field: \"name\", message: \"must be a finite number\", value: json } });\n  }\n"
        );
    }
}

/// #855 (Phase 2 step 7): builds the [`WireField`] list via
/// `bynk_check::wire::wire_type` — the same declaration-order shape (field
/// name, [`WireRef`] shape, and raw `(Expr, TypeRef)` default) both the
/// codec and a future peek would derive — rather than re-walking
/// `body.fields` inline. `default_value::wire_default_node` is the
/// one place a default's wire-JSON literal is produced, called from
/// `emit_record_codec` at the same point it always was (Part 1's seam: the
/// IR carries the raw default, the emitter renders it).
///
/// #1443 (Arc E slice 5): returns [`emit_record_codec`]'s own `Vec<TsDecl>`
/// straight through — a plain passthrough, the same shape
/// [`emit_refined`]'s own early return to `emit_bytes_named_codec` already
/// established (#1441).
fn emit_record(
    name: &str,
    decl: &TypeDecl,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    let prov = qual_provenance(qual, name);
    let fields = match wire_type(name, decl, types, prov) {
        Some(WireType {
            body: WireBody::Record { fields },
            ..
        }) => fields,
        _ => {
            unreachable!("emit_record is only ever called for a non-generic Record declaration")
        }
    };
    // #661: a consumed record's TS value type reaches through the type-only
    // namespace (`commerce_payment.Receipt`); the codec function name stays
    // bare and local. Its field codec calls are unqualified too — they resolve
    // to the caller's own locally-generated helpers.
    let ts_type = TsType::named(qual_type(qual, name));
    emit_record_codec(name, &ts_type, &fields, types, codecs, ru)
}

/// v0.174 (#592): the shared record codec body. `fn_suffix` is the codec name
/// suffix (`Order`, or the monomorphised `Paginated_User`); `ts_type` is the
/// TypeScript value type the codec accepts / returns (`Order`, or the erased
/// generic `Paginated<User>`). The two coincide for a non-generic record and
/// diverge for a generic-record instantiation.
///
/// #855 (Phase 2 step 7): takes `&[WireField]` — the field's shape as a
/// [`WireRef`] (rendered via [`serialise_field_expr_wire`] /
/// [`emit_field_deserialise_wire`], no re-derivation from a raw `TypeRef`)
/// and its default as a raw `(Expr, TypeRef)`, built into its wire-JSON
/// literal right here via `field_default_value` and `wire_default_node` — the same point it was
/// always called from, per Part 1's seam (`bynk-check` carries the boundary
/// fact, `bynk-emit` renders it). Only `deserialise_<fn_suffix>` consults
/// a default; `serialise_<fn_suffix>` is untouched (Decision B, #972). A
/// generic-record instantiation's default arrives typed at the
/// instantiation (#2607, Decision I).
///
/// #1443 (Arc E slice 5): returns the `[serialise, deserialise]` pair as
/// real `bynk_ts::TsDecl` nodes — the same `Vec<TsDecl>` shape
/// `emit_bytes_named_codec`/`emit_refined` already established (#1441), for
/// the same reason (this function's two real callers, `emit_record` above
/// and `emit_generic_helpers_qualified`'s own `RecordInst` arm — as of Arc F
/// slice 1, #1451, both `extend` a `Vec<TsDecl>` directly rather than
/// printing; [`decls_as_stmts`] does the one shared boundary-collect, once,
/// at each caller's own caller). Two internal calls stay unprinted:
/// `serialise_field_expr_wire` (`-> TsExpr` since #1435) and
/// `emit_field_deserialise_wire` (`-> Vec<TsStmt>` since #1439) are both
/// already tree-native, and this function itself is becoming tree-native
/// too — consuming their return values directly (no
/// `bynk_ts::print_expr`/`splice_stmts` boundary-print) is correct, not a
/// gap; that print/splice treatment is only for a caller that is itself
/// still `String`-based (`emit_generic_helpers_qualified`'s own
/// `ResultInst`/`OptionInst`/etc. arms, still out of this slice's scope).
fn emit_record_codec(
    fn_suffix: &str,
    ts_type: &TsType,
    fields: &[WireField],
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    // #2656: a record on a type cycle threads the nesting depth.
    let depth = depth_for(codecs, fn_suffix, true);
    let mut serialise_params = vec![TsParam::new("value".to_string(), Some(ts_type.clone()))];
    let mut serialise_body = Vec::new();
    if depth.is_some() {
        serialise_params.push(depth_param());
        serialise_body.push(depth_guard_encode());
    }
    serialise_body.push(ret(TsExpr::multiline_object(
        fields
            .iter()
            .map(|field| {
                (
                    field.name.clone(),
                    serialise_field_expr_wire(
                        &field.shape,
                        &ident(format!("value.{}", field.name)),
                        "",
                        depth,
                        ru,
                    ),
                )
            })
            .collect(),
    )));
    let serialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__serialise_{fn_suffix}"),
        generics: Vec::new(),
        params: serialise_params,
        return_type: Some(TsType::named("__JsonValue")),
        body: serialise_body,
        is_async: false,
        inline: false,
    }));

    // `typeof json !== "object" || json === null || Array.isArray(json)` —
    // a genuine 3-term `Or` chain (see `or_expr`'s own doc for why it
    // prints flat with no parens).
    let mut body: Vec<TsStmt> = depth.iter().map(|_| depth_guard_decode()).collect();
    body.extend([
        if_(
            or_expr(
                or_expr(
                    strict_neq(typeof_expr(ident("json")), str_lit("object")),
                    strict_eq(ident("json"), TsExpr::Lit(TsLit::Null)),
                ),
                call(
                    member(ident("globalThis.Array"), "isArray"),
                    vec![ident("json")],
                ),
            ),
            block(vec![ret(err_structural_mismatch_top(
                "object",
                typeof_expr(ident("json")),
            ))]),
        ),
        TsStmt::const_stmt(
            TsBindingName::Ident("obj".to_string()),
            None,
            as_expr(ident("json"), index_signature_record_ty()),
            None,
        ),
    ]);
    for field in fields {
        // Events slice 3a (#972): a defaulted field is read through a
        // pre-validated `__d_<field>` binding instead of the raw
        // `obj["<field>"]` access — `"fname" in obj`, not `!== undefined`,
        // is the only test that distinguishes a genuinely absent wire key
        // from one present with an explicit value (Decision D; this is also
        // what makes `Option[T]`'s two absences fall out with no
        // special-casing — a wire `{"kind":"None"}` already passed the `in`
        // test, so it flows through to a real `None`, untouched by the
        // default; so does an explicit `null`, since #1887). Everything
        // downstream (`emit_field_deserialise_wire`) is unchanged either
        // way. An undefaulted field reads `obj["<field>"]` with no presence
        // check, so an absent key reaches its codec as `undefined`, which
        // the lenient `Option` decoder (#1887) reads as `None` and every
        // other codec rejects.
        let default = field
            .default
            .as_ref()
            .and_then(|(e, t)| field_default_value(e, t, types).ok());
        let fname = &field.name;
        let access = if let Some(d) = &default {
            // `"<field>" in obj ? obj["<field>"] : <default>`: `d` is the default's tree
            // (`field_default_value`), rendered in its wire form (`wire_default_node`). As a
            // `Conditional`'s own `alternate` it sits in an `AssignmentExpression` grammar
            // position, which (per `bynk-ts/src/printer/expr.rs`'s own `Conditional` render arm)
            // never adds parens around either branch, whatever the branch's shape.
            let d_name = format!("__d_{fname}");
            body.push(TsStmt::const_stmt(
                TsBindingName::Ident(d_name.clone()),
                Some(TsType::named("__JsonValue")),
                TsExpr::Conditional {
                    test: Box::new(binary(TsBinaryOp::In, str_lit(fname.clone()), ident("obj"))),
                    consequent: Box::new(index(ident("obj"), str_lit(fname.clone()))),
                    alternate: Box::new(wire_default_node(d)),
                },
                None,
            ));
            ident(d_name)
        } else {
            index(ident("obj"), str_lit(fname.clone()))
        };
        let sub_path = path_field(&field.path_segment);
        // #1439 (Arc E slice 3): `emit_field_deserialise_wire` returns a
        // real `Vec<bynk_ts::TsStmt>` — extended straight into this
        // function's own body, no `splice_stmts` (that boundary-print
        // helper is only for a caller that still prints into a `String`;
        // this function's own body is a real `Vec<TsStmt>` now).
        body.extend(emit_field_deserialise_wire(
            fname,
            &field.shape,
            &access,
            &sub_path,
            depth,
            ru,
        ));
        // #1969: an inline field refinement (`qty: Int where Positive`) is
        // checked on the decoded value, after the field's own codec, and fails
        // as a `RefinementViolation` at the field's path — the same envelope a
        // named refined type's decode gives.
        body.extend(emit_field_refinement_checks(
            fname,
            &field.predicates,
            &sub_path,
            match &field.shape {
                WireRef::Base { base, .. } => Some(*base),
                WireRef::Decimal => Some(BaseType::Decimal),
                WireRef::Bytes
                | WireRef::Named { .. }
                | WireRef::Inst { .. }
                | WireRef::Unit
                | WireRef::Unchecked { .. } => None,
            },
            ru,
        ));
    }
    // Review of #1444: a *zero-field* record — `type X = {}`, parse-legal
    // Bynk that never reaches a wire boundary in the current corpus, so no
    // fixture exercises this — hits `TsExpr::object`'s own single-line
    // collapse (`{}`), where the pre-conversion `write!(out, "  return Ok({{
    // ")` + empty `parts.join(", ")` + `" }} as {ts_type});"` always built
    // `Ok({  } as T)` (a literal double space, the same "`{  }`, not the
    // tight `{}` shortcut" quirk `workers.rs`'s/`project.rs`'s own
    // `deps`/`{ns}Deps` objects already carry). TS-parse-identical either
    // way — documented rather than silently changed, the same "name a real,
    // unreachable-today formatting hazard rather than fix it inline"
    // precedent P7.9's own `Array`-over-`Union` hazard already set (that one
    // since closed, #1898).
    body.push(ret(call(
        ident("Ok"),
        vec![as_expr(
            TsExpr::object(
                fields
                    .iter()
                    .map(|field| (field.name.clone(), ident(format!("__{}", field.name))))
                    .collect(),
            ),
            ts_type.clone(),
        )],
    )));

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{fn_suffix}"),
        generics: Vec::new(),
        params: deserialise_params(depth),
        return_type: Some(boundary_result(ts_type.clone())),
        body,
        is_async: false,
        inline: false,
    }));

    vec![serialise, deserialise]
}

/// #855 (Phase 2 step 8): builds the [`WireSum`] via
/// `bynk_check::wire::wire_type` — the same declaration-order variant/payload
/// shape a peek would derive — instead of re-walking `body.variants` inline.
///
/// #1445 (Arc E slice 6): returns [`emit_sum_codec`]'s own `Vec<TsDecl>`
/// straight through — a plain passthrough, the same shape [`emit_record`]'s
/// own early return to `emit_record_codec` already established (#1443).
fn emit_sum(
    name: &str,
    decl: &TypeDecl,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    // #661: a consumed sum's TS value type is namespace-qualified; the codec
    // function name and its per-variant codec calls stay bare and local. #593:
    // the codec body is the shared `emit_sum_codec` (also reused, unqualified,
    // for a generic-sum instantiation), so the qualified value type is threaded
    // in as its `ts_type`.
    let prov = qual_provenance(qual, name);
    let sum = match wire_type(name, decl, types, prov) {
        Some(WireType {
            body: WireBody::Sum(s),
            ..
        }) => s,
        _ => unreachable!("emit_sum is only ever called for a non-generic Sum declaration"),
    };
    let ty = TsType::named(qual_type(qual, name));
    emit_sum_codec(name, &ty, &sum, codecs, ru)
}

/// The serialise/deserialise pair for a sum type, over an already-resolved
/// [`WireSum`]. `fn_suffix` names the emitted functions (`Opt` / `Opt_Int`),
/// `ts_type` is their value type (`Opt` / `Opt<number>` / a namespace-qualified
/// `shop.Opt`). #593: a generic-sum instantiation reuses this with substituted
/// payload types, exactly as a generic record reuses [`emit_record_codec`].
///
/// #855 (Phase 2 step 8): the wire discriminant (`kind`) and in-memory
/// discriminant (`tag`) are read off `sum.wire_discriminant` /
/// `sum.memory_discriminant` rather than hard-coded string literals — the
/// same two values as before (`wire.rs`'s module doc: `memory_discriminant`
/// is the softest part of the seam, carried beside `wire_discriminant`
/// because this codec's whole job is translating between them), so this is
/// purely reading the fact from the IR, not a behaviour change.
///
/// #1445 (Arc E slice 6): returns the `[serialise, deserialise]` pair as real
/// `bynk_ts::TsDecl` nodes, the same `Vec<TsDecl>` shape #1441/#1443 already
/// established — this function's two real callers ([`emit_sum`] above and
/// `emit_generic_helpers_qualified`'s own `SumInst` arm) — as of Arc F slice
/// 1 (#1451), both `extend` a `Vec<TsDecl>` directly rather than printing;
/// [`decls_as_stmts`] does the one shared boundary-collect, once, at each
/// caller's own caller.
///
/// **The `(value as any).<field>` investigation (#1423's own residual,
/// re-examined per this issue's own explicit instruction):** narrowed, not
/// kept. Empirically confirmed under `tsc --strict` (not assumed): once
/// `value.{tag}` is the `switch` discriminant, TypeScript's own control-flow
/// analysis narrows `value` to the exact variant member inside each `case`
/// block — the same narrowing `if (value.tag === "...")` gets, and `switch`
/// on a discriminant property is documented TS behaviour, not a guess. This
/// is a genuinely different shape from P7.2's own failed attempt (the
/// comment this replaces): that attempt cast `value` itself to `Record<
/// string, unknown>` BEFORE any narrowing, which throws away the literal
/// discriminant entirely and hands `serialise_field_expr_wire`'s per-shape
/// helpers (`(v: number) => ...` for `Float`, etc.) a field typed `unknown`
/// no narrowing could ever recover. Reading the field through the
/// switch-narrowed `value` directly, with NO cast at all, needs no such
/// recovery — confirmed against both a plain-`string` field and a `Float`
/// field (`Number.isFinite` guard intact) under `tsc --strict --lib es2020`,
/// and again against a *generic*-sum instantiation (`ApiResult<User>`,
/// `value.value` narrowed to the concrete `User` argument, not just the
/// erased type parameter). Net effect: `ts_any` drops by exactly this one
/// Rust-source site (31 → 30) — the opposite of slice 3's own "don't claim
/// a false win" trap, a real, verified win. The fixture corpus's own
/// `(value as any).<field>` bytes (`212_json_codec`,
/// `407_workers_generic_sum_boundary`, et al.) change to a bare
/// `value.<field>` as a direct, intended consequence — reblessed, not a
/// zero-diff violation (this one narrow site is the explicitly-authorised
/// exception to this slice's own zero-diff mandate, not an accidental
/// formatting drift).
fn emit_sum_codec(
    fn_suffix: &str,
    ts_type: &TsType,
    sum: &WireSum,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    let kind = sum.wire_discriminant;
    // #2656: a sum on a type cycle threads the nesting depth.
    let depth = depth_for(codecs, fn_suffix, true);
    let tag = sum.memory_discriminant;

    let serialise_cases: Vec<TsSwitchCase> = sum
        .variants
        .iter()
        .map(|variant| {
            let vname = &variant.name;
            if variant.payload.is_empty() {
                case_(
                    str_lit(vname.clone()),
                    vec![ret(TsExpr::object(vec![(
                        kind.to_string(),
                        str_lit(vname.clone()),
                    )]))],
                    false,
                )
            } else {
                let mut entries = vec![(kind.to_string(), str_lit(vname.clone()))];
                for field in &variant.payload {
                    // Narrowed (see this function's own doc): `value` is
                    // already the exact variant member inside this `case`
                    // block (TypeScript's own discriminated-union narrowing
                    // through the `switch (value.{tag})` above), so the
                    // field reads through it directly — no `as any`, no
                    // recovery cast.
                    entries.push((
                        field.name.clone(),
                        serialise_field_expr_wire(
                            &field.shape,
                            &ident(format!("value.{}", payload_prop(&field.name))),
                            "",
                            depth,
                            ru,
                        ),
                    ));
                }
                case_(
                    str_lit(vname.clone()),
                    vec![ret(TsExpr::object(entries))],
                    true,
                )
            }
        })
        .collect();
    let mut serialise_params = vec![TsParam::new("value".to_string(), Some(ts_type.clone()))];
    let mut serialise_body = Vec::new();
    if depth.is_some() {
        serialise_params.push(depth_param());
        serialise_body.push(depth_guard_encode());
    }
    serialise_body.push(switch_stmt(member(ident("value"), tag), serialise_cases));
    let serialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__serialise_{fn_suffix}"),
        generics: Vec::new(),
        params: serialise_params,
        return_type: Some(TsType::named("__JsonValue")),
        body: serialise_body,
        is_async: false,
        inline: false,
    }));

    // The top-level structural guard + `obj` cast — the same shape
    // `emit_record_codec`'s own deserialise body opens with (#1443), kept
    // as a second, file-local copy rather than shared (this file's own
    // "own builder set" convention, `index_signature_record_ty`'s own doc).
    let mut deserialise_body: Vec<TsStmt> = depth.iter().map(|_| depth_guard_decode()).collect();
    deserialise_body.extend([
        if_(
            or_expr(
                or_expr(
                    strict_neq(typeof_expr(ident("json")), str_lit("object")),
                    strict_eq(ident("json"), TsExpr::Lit(TsLit::Null)),
                ),
                call(
                    member(ident("globalThis.Array"), "isArray"),
                    vec![ident("json")],
                ),
            ),
            block(vec![ret(err_structural_mismatch_top(
                "object",
                typeof_expr(ident("json")),
            ))]),
        ),
        TsStmt::const_stmt(
            TsBindingName::Ident("obj".to_string()),
            None,
            as_expr(ident("json"), index_signature_record_ty()),
            None,
        ),
        const_(
            kind.to_string(),
            index(ident("obj"), str_lit(kind.to_string())),
        ),
    ]);
    let mut deserialise_cases: Vec<TsSwitchCase> = sum
        .variants
        .iter()
        .map(|variant| {
            let vname = &variant.name;
            if variant.payload.is_empty() {
                case_(
                    str_lit(vname.clone()),
                    vec![ret(call(
                        ident("Ok"),
                        vec![as_expr(
                            TsExpr::object(vec![(tag.to_string(), str_lit(vname.clone()))]),
                            ts_type.clone(),
                        )],
                    ))],
                    false,
                )
            } else {
                let mut stmts = Vec::new();
                for field in &variant.payload {
                    let access = index(ident("obj"), str_lit(field.name.clone()));
                    let sub_path = path_field(&field.path_segment);
                    stmts.extend(emit_field_deserialise_wire(
                        &field.name,
                        &field.shape,
                        &access,
                        &sub_path,
                        depth,
                        ru,
                    ));
                }
                let mut entries = vec![(tag.to_string(), str_lit(vname.clone()))];
                for field in &variant.payload {
                    entries.push((
                        payload_prop(&field.name),
                        ident(format!("__{}", field.name)),
                    ));
                }
                stmts.push(ret(call(
                    ident("Ok"),
                    vec![as_expr(TsExpr::object(entries), ts_type.clone())],
                )));
                case_(str_lit(vname.clone()), stmts, true)
            }
        })
        .collect();
    deserialise_cases.push(default_case(vec![ret(err_structural_mismatch_top(
        "sum variant kind",
        call(ident("String"), vec![ident(kind)]),
    ))]));
    deserialise_body.push(switch_stmt(ident(kind), deserialise_cases));

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{fn_suffix}"),
        generics: Vec::new(),
        params: deserialise_params(depth),
        return_type: Some(boundary_result(ts_type.clone())),
        body: deserialise_body,
        is_async: false,
        inline: false,
    }));

    vec![serialise, deserialise]
}

#[cfg(test)]
mod emit_sum_codec_tests {
    use super::*;
    use bynk_check::wire::Expected;

    fn render_decls(decls: Vec<TsDecl>) -> String {
        decls
            .into_iter()
            .map(|d| bynk_ts::print_stmt(&TsStmt::decl(d, None), 0))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Direct unit coverage for the one real shape no fixture in the current
    /// corpus reaches (confirmed by grep — a Python scan over every emitted
    /// `.ts` file in the tree for a `case "<variant>": { return { kind:
    /// "...", f1: ..., f2: ..., ... }` shape with 2+ payload fields found
    /// zero matches — not assumed): a payload-carrying variant with TWO OR
    /// MORE fields. The one real multi-field sum in the fixture corpus
    /// (`bynk-tests/tests/fixtures/positive/474_or_patterns`'s own
    /// `BookingState`) never crosses the wire boundary, so `serialise_
    /// BookingState`/`deserialise_BookingState` are never generated; every
    /// sum type that DOES cross the boundary (`Status`, `MessageArg`,
    /// `ApiResult<T>`, ...) happens to carry exactly one field per
    /// payload-carrying variant. This test also re-pins, directly rather
    /// than only end-to-end: the mixed payload-free/payload-carrying
    /// `case_braced` shape (`212_json_codec`'s own real fixture, `Status`),
    /// and the narrowed (no `as any`) field access this slice's own
    /// investigation established (this function's own doc).
    #[test]
    fn multi_field_variant_and_payload_free_variant_share_one_switch() {
        let ru = RuntimeUse::default();
        let sum = WireSum {
            wire_discriminant: "kind",
            memory_discriminant: "tag",
            variants: vec![
                WireVariant {
                    name: "Pending".to_string(),
                    payload: vec![],
                },
                WireVariant {
                    name: "Shipped".to_string(),
                    payload: vec![
                        WireField {
                            name: "tracking".to_string(),
                            shape: WireRef::Base {
                                base: BaseType::String,
                                json: JsonKind::String,
                                guards: vec![],
                                expected: Expected::Json(JsonKind::String),
                            },
                            path_segment: "tracking".to_string(),
                            default: None,
                            predicates: Vec::new(),
                        },
                        WireField {
                            name: "weight".to_string(),
                            shape: WireRef::Base {
                                base: BaseType::Int,
                                json: JsonKind::Number,
                                guards: vec![BaseGuard::Integral],
                                expected: Expected::Json(JsonKind::Number),
                            },
                            path_segment: "weight".to_string(),
                            default: None,
                            predicates: Vec::new(),
                        },
                    ],
                },
            ],
        };
        let decls = emit_sum_codec(
            "Parcel",
            &TsType::named("Parcel"),
            &sum,
            &DepthCodecs::new(),
            &ru,
        );
        assert_eq!(
            render_decls(decls),
            "export function __serialise_Parcel(value: Parcel): __JsonValue {\n  switch (value.tag) {\n    case \"Pending\":\n      return { kind: \"Pending\" };\n    case \"Shipped\": {\n      return { kind: \"Shipped\", tracking: value.tracking as __JsonValue, weight: ((v: number) => { if (!globalThis.Number.isSafeInteger(v)) throw new globalThis.Error(\"Int outside the safe-integer range at boundary\"); return v as __JsonValue; })(value.weight) };\n    }\n  }\n}\n\nexport function __deserialise_Parcel(json: __JsonValue, path: string = \"$\"): Result<Parcel, __BoundaryError> {\n  if (typeof json !== \"object\" || json === null || globalThis.Array.isArray(json)) {\n    return Err({ kind: \"StructuralMismatch\", path, expected: \"object\", actual: typeof json });\n  }\n  const obj = json as { [k: string]: __JsonValue };\n  const kind = obj[\"kind\"];\n  switch (kind) {\n    case \"Pending\":\n      return Ok({ tag: \"Pending\" } as Parcel);\n    case \"Shipped\": {\n      if (typeof obj[\"tracking\"] !== \"string\") {\n        return Err({ kind: \"StructuralMismatch\", path: `${path}.tracking`, expected: \"string\", actual: typeof obj[\"tracking\"] });\n      }\n      const __tracking = obj[\"tracking\"];\n      if (typeof obj[\"weight\"] !== \"number\") {\n        return Err({ kind: \"StructuralMismatch\", path: `${path}.weight`, expected: \"number\", actual: typeof obj[\"weight\"] });\n      }\n      if (!globalThis.Number.isSafeInteger(obj[\"weight\"])) {\n        return Err({ kind: \"StructuralMismatch\", path: `${path}.weight`, expected: \"safe integer\", actual: String(obj[\"weight\"]) });\n      }\n      const __weight = obj[\"weight\"];\n      return Ok({ tag: \"Shipped\", tracking: __tracking, weight: __weight } as Parcel);\n    }\n    default:\n      return Err({ kind: \"StructuralMismatch\", path, expected: \"sum variant kind\", actual: String(kind) });\n  }\n}\n"
        );
    }
}

// #855 (Phase 1): `inner_ts_name` moved to `bynk_check::wire` as
// `codec_suffix`; `collect_codec_closure` moved verbatim (pure, AST-only —
// see that module's doc for the seam). Re-exported under their original
// names so call sites here and elsewhere in `bynk-emit` keep compiling
// unchanged.
pub(crate) use bynk_check::wire::codec_suffix as inner_ts_name;
pub use bynk_check::wire::collect_codec_closure;

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

// #855 (Phase 1): `collect_generic_instantiations` and `GenericInst` (now
// `WireInst` — see that type's doc for why its variant names are kept as
// `ResultInst`/`OptionInst`/… rather than the plan's bare spelling, deferred
// to Phase 2) moved to `bynk_check::wire`; `walk_generic_inst` moved with
// them (no callers outside the functions that moved). Re-exported under
// their original names so call sites here and elsewhere in `bynk-emit` keep
// compiling unchanged.
pub(crate) use bynk_check::wire::WireInst as GenericInst;
pub(crate) use bynk_check::wire::collect_generic_instantiations;

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

/// Prefix of the stand-in name a codec factory gives a generic declaration's
/// type parameter. The prefix keeps it from colliding with a declared type of
/// the same name; the helpers nested in the factory name it in their suffixes.
const FACTORY_PARAM_PREFIX: &str = "__P_";

/// The TypeScript name of the codec factory for generic declaration `name`.
/// Keyed on the bare declaration name, as every other codec name is: a module's
/// type table (`TypedCommons::types`) holds one declaration per name, so two
/// same-named generic declarations from different units cannot both be in scope.
pub(crate) fn codec_factory_ident(name: &str) -> String {
    format!("__codecOf_{name}")
}

/// #2792: the root a codec factory is requested with: the generic declaration
/// `name` applied to its own type parameters, each as a stand-in name. `None`
/// when `name` is not a generic record or sum.
pub(crate) fn codec_factory_root(
    name: &str,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Option<TypeRef> {
    let decl = types.get(name)?;
    if decl.type_params.is_empty()
        || !matches!(
            decl.body,
            bynk_syntax::ast::TypeBody::Record(_) | bynk_syntax::ast::TypeBody::Sum(_)
        )
    {
        return None;
    }
    let sp = bynk_syntax::span::Span::new(0, 0);
    Some(TypeRef::App {
        name: bynk_syntax::ast::Ident {
            name: name.to_string(),
            span: sp,
        },
        args: decl
            .type_params
            .iter()
            .map(|p| {
                TypeRef::Named(bynk_syntax::ast::Ident {
                    name: format!("{FACTORY_PARAM_PREFIX}{}", p.name.name),
                    span: sp,
                })
            })
            .collect(),
        span: sp,
    })
}

/// Whether `t` is a codec factory root ([`codec_factory_root`]), and for which
/// declaration.
pub(crate) fn codec_factory_name(t: &TypeRef) -> Option<&str> {
    match t {
        TypeRef::App { name, args, .. }
            if args.iter().all(
                |a| matches!(a, TypeRef::Named(id) if id.name.starts_with(FACTORY_PARAM_PREFIX)),
            ) && !args.is_empty() =>
        {
            Some(&name.name)
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::Result(..)
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
        | TypeRef::App { .. } => None,
    }
}

fn mentions_factory_param(t: &TypeRef) -> bool {
    match t {
        TypeRef::Named(id) => id.name.starts_with(FACTORY_PARAM_PREFIX),
        TypeRef::App { args, .. } => args.iter().any(mentions_factory_param),
        TypeRef::Option(a, _) | TypeRef::List(a, _) | TypeRef::Set(a, _) => {
            mentions_factory_param(a)
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            mentions_factory_param(a) || mentions_factory_param(b)
        }
        TypeRef::Base(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..) => false,
    }
}

/// The type-parameter-free parts of the declaration's fields: the types whose
/// ordinary codecs the factory calls, which the module must emit beside it.
fn factory_ground_parts(t: &TypeRef, out: &mut Vec<TypeRef>) {
    if !mentions_factory_param(t) {
        out.push(t.clone());
        return;
    }
    match t {
        TypeRef::App { args, .. } => args.iter().for_each(|a| factory_ground_parts(a, out)),
        TypeRef::Option(a, _) | TypeRef::List(a, _) | TypeRef::Set(a, _) => {
            factory_ground_parts(a, out)
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            factory_ground_parts(a, out);
            factory_ground_parts(b, out);
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..) => {}
    }
}

/// The field and payload types of the factory's declaration, instantiated at
/// its stand-in parameter names.
fn factory_field_types(
    root: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Vec<TypeRef> {
    let TypeRef::App { name, args, .. } = root else {
        return Vec::new();
    };
    if let Some(fields) = record_inst_fields(&name.name, args, types) {
        return fields.into_iter().map(|(_, t)| t).collect();
    }
    sum_inst_variants(&name.name, args, types)
        .map(|vs| {
            vs.into_iter()
                .flat_map(|(_, fields)| fields.into_iter().map(|(_, t)| t))
                .collect()
        })
        .unwrap_or_default()
}

/// #2792: the roots the module's ordinary codec closure needs for the
/// type-parameter-free parts of a factory's declaration.
pub(crate) fn codec_factory_ground_roots(
    root: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Vec<TypeRef> {
    let mut out = Vec::new();
    for t in factory_field_types(root, types) {
        factory_ground_parts(&t, &mut out);
    }
    out
}

/// #2792: the codec factory for a generic record or sum, `root` being its
/// [`codec_factory_root`]. A function from the witnesses of the declaration's
/// type parameters to the `{ encode, decode }` codec of the instantiation.
///
/// The body is the monomorphised codec for the declaration applied to its
/// stand-in parameter names, nested so that the calls it makes for a parameter
/// (`__serialise___P_T`, `__deserialise___P_T`) reach the witness. Every other
/// codec it calls is the module's ordinary one. So the JSON, the error paths
/// and the revalidation are the monomorphised codec's, by construction.
pub(crate) fn emit_codec_factory(
    root: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    ru: &RuntimeUse,
) -> TsDecl {
    let TypeRef::App { name, args, .. } = root else {
        unreachable!("bynk internal error: a codec factory root is a generic application");
    };
    let params: Vec<String> = args
        .iter()
        .map(|a| match a {
            TypeRef::Named(id) => id.name.clone(),
            TypeRef::Base(..)
            | TypeRef::Result(..)
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
            | TypeRef::App { .. } => {
                unreachable!("bynk internal error: a codec factory root names its parameters")
            }
        })
        .collect();
    let (_, insts) = bynk_check::wire::collect_codec_closure(std::slice::from_ref(root), types);
    // The helpers that mention a parameter nest in the factory; the rest are
    // the module's own, emitted from `codec_factory_ground_roots`.
    let nested: Vec<GenericInst> = insts
        .into_iter()
        .filter(|i| i.ts_name().contains(FACTORY_PARAM_PREFIX))
        .collect();
    let mut body: Vec<TsStmt> = Vec::new();
    for p in &params {
        let witness = |method: &str, args: Vec<TsExpr>| {
            call(member(ident(format!("__codec_{p}")), method), args)
        };
        let arrow = |params: Vec<TsParam>, body: TsExpr| TsExpr::Arrow {
            params,
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(body))),
        };
        body.push(const_(
            format!("__serialise_{p}"),
            arrow(
                vec![TsParam::new("v", Some(TsType::named(p.clone())))],
                witness("encode", vec![ident("v")]),
            ),
        ));
        body.push(const_(
            format!("__deserialise_{p}"),
            arrow(
                vec![
                    TsParam::new("j", Some(TsType::named("__JsonValue"))),
                    deserialise_path_param(),
                ],
                witness("decode", vec![ident("j"), ident("path")]),
            ),
        ));
    }
    for d in emit_generic_helpers(&nested, types, ru) {
        let d = if let TsDecl::Export(inner) = d {
            *inner
        } else {
            d
        };
        body.push(TsStmt::decl(d, None));
    }
    let suffix = app_ts_name(&name.name, args);
    let ts_type = format!("{}<{}>", name.name, params.join(", "));
    body.push(ret(TsExpr::object(vec![
        ("encode".to_string(), ident(format!("__serialise_{suffix}"))),
        (
            "decode".to_string(),
            ident(format!("__deserialise_{suffix}")),
        ),
    ])));
    let witness_ty = |p: &str| {
        format!(
            "{{ encode: (v: {p}) => __JsonValue; decode: (j: __JsonValue, path: string) => Result<{p}, __BoundaryError> }}"
        )
    };
    TsDecl::Function {
        name: codec_factory_ident(&name.name),
        generics: params.clone(),
        params: params
            .iter()
            .map(|p| TsParam::new(format!("__codec_{p}"), Some(TsType::named(witness_ty(p)))))
            .collect(),
        return_type: Some(TsType::named(format!(
            "{{ encode: (v: {ts_type}) => __JsonValue; decode: (j: __JsonValue, path?: string) => Result<{ts_type}, __BoundaryError> }}"
        ))),
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
/// ([`decls_as_stmts`]) now happens once, at the caller, instead of inline in
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
                        WireField {
                            shape: wire_ref_of(&t),
                            predicates,
                            path_segment: n.clone(),
                            default: init.map(|e| (e.clone(), t.clone())),
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
                let sum = WireSum {
                    wire_discriminant: bynk_check::wire::WIRE_DISCRIMINANT,
                    memory_discriminant: "tag",
                    variants: variants
                        .into_iter()
                        .map(|(vname, payload)| WireVariant {
                            name: vname,
                            payload: payload
                                .into_iter()
                                .map(|(fname, t)| WireField {
                                    shape: wire_ref_of(&t),
                                    path_segment: fname.clone(),
                                    name: fname,
                                    default: None,
                                    predicates: Vec::new(),
                                })
                                .collect(),
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
                let key_ts = inner_ts_name(key);
                let val_ts = inner_ts_name(val);
                let key_ty = qualified_ts_type(key, qual);
                let val_ty = qualified_ts_type(val, qual);
                let map_ty =
                    TsType::named_with_args("ReadonlyMap", vec![key_ty.clone(), val_ty.clone()]);
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

                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_Map_{key_ts}_{val_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(map_ty.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body: vec![
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
                    ],
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
                    body: vec![
                        array_shape_guard(),
                        TsStmt::const_stmt(
                            TsBindingName::Ident("out".to_string()),
                            None,
                            TsExpr::New {
                                callee: Box::new(map_ctor.clone()),
                                args: vec![],
                            },
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
        }
    }
    decls
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

/// #3066: whether a codec root names one of `names` (a suite's own types)
/// anywhere, through generic arguments. Such a root is served by the suite
/// runner's own local codecs ([`super::emit::emit_suite_decls`]), not the
/// module-level closure, which cannot name a type declared inside a runner.
pub fn root_names_any(root: &TypeRef, names: &std::collections::HashSet<String>) -> bool {
    !names.is_empty()
        && super::workers::named_types_in(root)
            .iter()
            .any(|n| names.contains(n))
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
