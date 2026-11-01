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

use std::sync::Arc;

use bynk_syntax::ast::{BaseType, PredKind, TypeBody, TypeDecl, TypeRef};

use crate::emitter::{RuntimeUse, payload_prop};
use bynk_check::wire_default::lower_field_default_wire;
use bynk_ts::{
    TsArrowBody, TsBinaryOp, TsBindingName, TsDecl, TsExpr, TsLit, TsObjectEntry, TsParam, TsStmt,
    TsSwitchCase, TsType, TsTypeMember, TsUnaryOp,
};

// #1435 (Arc E slice 1): this file's own local `TsExpr` builder set, the
// same "compose the real node algebra for this file's own repeated shapes"
// convention `workers.rs`/`workers_entry.rs` already established (#1321,
// #1323) — not part of the public node algebra, `bynk-ts` still owns every
// real constructor. Kept as this file's own private set rather than shared
// cross-file, matching this track's own established per-file scoping.

fn ident(s: impl Into<String>) -> TsExpr {
    TsExpr::Ident(s.into())
}

fn str_lit(s: impl Into<String>) -> TsExpr {
    TsExpr::Lit(TsLit::Str(s.into()))
}

fn member(object: TsExpr, property: impl Into<String>) -> TsExpr {
    TsExpr::Member {
        object: Box::new(object),
        property: property.into(),
    }
}

fn call(callee: TsExpr, args: Vec<TsExpr>) -> TsExpr {
    TsExpr::Call {
        callee: Box::new(callee),
        args,
    }
}

fn as_expr(expr: TsExpr, ty: TsType) -> TsExpr {
    TsExpr::As {
        expr: Box::new(expr),
        ty,
    }
}

// #1439 (Arc E slice 3): the statement-builder half of this file's own
// local node-builder set — slices 1/2 (#1436/#1438) only ever needed the
// `TsExpr` builders above (every prior conversion in this file returned one
// expression); this slice's `emit_field_deserialise_wire` returns a real
// `Vec<TsStmt>` per `WireRef` arm, so it needs the `TsStmt` half too. Same
// naming/shape convention `workers.rs`/`workers_entry.rs` already
// established for their own statement-builder sets (#1321/#1323).

fn not_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Unary {
        op: TsUnaryOp::Not,
        expr: Box::new(expr),
    }
}

fn typeof_expr(expr: TsExpr) -> TsExpr {
    TsExpr::Unary {
        op: TsUnaryOp::Typeof,
        expr: Box::new(expr),
    }
}

fn binary(op: TsBinaryOp, left: TsExpr, right: TsExpr) -> TsExpr {
    TsExpr::Binary {
        op,
        left: Box::new(left),
        right: Box::new(right),
    }
}

fn strict_eq(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::StrictEq, left, right)
}

fn strict_neq(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::StrictNotEq, left, right)
}

/// `left || right` — #1443 (Arc E slice 5)'s own real gap:
/// `emit_record_codec`'s structural-shape guard (`typeof json !== "object"
/// || json === null || Array.isArray(json)`) is a genuine 3-term `Or`
/// chain, this file's first (every prior `Or`/`And` use in this crate lives
/// in `workers.rs`/`workers_entry.rs`, not here). Left-folding two calls
/// (`or_expr(or_expr(a, b), c)`) prints flat with no parens — `||` is one
/// of the two operators [`render_binary_operand`] (`bynk-ts/src/printer.rs`)
/// already exempts from its "same-operator nesting still parenthesizes"
/// rule, the exact precedent `workers_entry.rs`'s own
/// `emit_call_handler_dispatch` 3-term chain established.
fn or_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::Or, left, right)
}

/// `left in right` — #1443 (Arc E slice 5): the "is this wire key present
/// at all" test `emit_record_codec`'s per-field default-value
/// prevalidation line needs. See [`bynk_ts::TsBinaryOp::In`]'s own doc for
/// why this is a new operator, not a re-derivation of an existing one.
fn in_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::In, left, right)
}

/// `left < right` — Arc E slice 7 (#1447): `emit_generic_helpers_qualified`'s
/// own `ListInst`/`MapInst` arms both need the loop-test half of a real
/// `for (let i = 0; i < json.length; i++)` (see `bynk_ts::TsStmt::for_stmt`'s
/// own doc, and `TsStmtKind::For`'s behind it, for why that construct exists
/// at all). `TsBinaryOp::LessThan`
/// itself already exists (Arc C slice 33, #1401) — this is this *file's*
/// first real use of it, not a new operator.
fn less_than(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::LessThan, left, right)
}

/// `object[index]` — a computed member access, distinct from [`member`]'s
/// dotted `object.property` form. #1443 (Arc E slice 5)'s own real gap:
/// `emit_record_codec`'s default-value prevalidation ternary's own
/// `obj["<field>"]` consequent is this *file's* first real computed-index
/// access (`workers_entry.rs`'s own `index_expr` — an identical two-line
/// builder — is the cross-file precedent for the shape; not shared here
/// since this file already keeps its own private builder set per #1435's
/// own doc).
fn index(object: TsExpr, idx: TsExpr) -> TsExpr {
    TsExpr::Index {
        object: Box::new(object),
        index: Box::new(idx),
    }
}

fn const_(name: impl Into<String>, init: TsExpr) -> TsStmt {
    TsStmt::const_stmt(TsBindingName::Ident(name.into()), None, init, None)
}

fn return_(expr: TsExpr) -> TsStmt {
    TsStmt::return_stmt(Some(expr), None)
}

fn if_(cond: TsExpr, then_branch: TsStmt) -> TsStmt {
    TsStmt::if_stmt(cond, then_branch, None)
}

fn block(stmts: Vec<TsStmt>) -> TsStmt {
    TsStmt::block(stmts, None)
}

/// `switch (<discriminant>) { <cases> }` — Arc E slice 6 (#1445),
/// [`emit_sum_codec`]'s own first real `switch` statement in this file
/// (`TsStmt::Switch`/[`TsSwitchCase`] themselves are Arc C slice 4/#1323,
/// `workers_entry.rs`). Kept as this file's own private wrapper (matching
/// `const_`/`return_`/`if_`/`block` just above) rather than calling
/// `TsStmt::switch_stmt` directly at each of `emit_sum_codec`'s two call
/// sites.
fn switch_stmt(discriminant: TsExpr, cases: Vec<TsSwitchCase>) -> TsStmt {
    TsStmt::switch_stmt(discriminant, cases, None)
}

/// `case <test>: <body>` — unlike `workers_entry.rs`'s own identically-named
/// helper (which always braces, since every one of its real cases wants a
/// `{ }` block), `emit_sum_codec` has BOTH real shapes side by side in the
/// same switch: a payload-free variant's case is unbraced, a
/// payload-carrying sibling's is braced (`212_json_codec`'s own mixed
/// `Status` fixture: `case "Pending": return ...;` beside `case "Shipped":
/// { ... }`). `braced` is the caller's own explicit choice rather than an
/// inferred one, so this stays a thin, honest wrapper over
/// [`TsSwitchCase::case_braced`] (`bynk-ts/src/program.rs`, #1445) instead
/// of a heuristic derived from `body`'s own shape.
fn case_(test: TsExpr, body: Vec<TsStmt>, braced: bool) -> TsSwitchCase {
    TsSwitchCase {
        test: Some(test),
        body,
        default_braced: false,
        case_braced: braced,
    }
}

/// `default: <body>` — unbraced, the same convention `workers_entry.rs`'s
/// own `default_case` helper already established for every real `default`
/// arm this file's own sum codec has (a `StructuralMismatch` one-liner).
fn default_case(body: Vec<TsStmt>) -> TsSwitchCase {
    TsSwitchCase {
        test: None,
        body,
        default_braced: false,
        case_braced: true,
    }
}

/// A real `Vec<bynk_ts::TsStmt>`, pre-rendered into ONE `TsStmt::Raw` blob
/// at the same depth-1 indent [`splice_stmts`] has always used for these
/// exact statements. [`emit_sum_codec`]'s deserialise-side payload-carrying
/// case needs `emit_field_deserialise_wire`'s own field guards spliced in
/// at a depth that does NOT match where they structurally sit (inside a
/// `TsStmt::Switch` case's own body, which `bynk-ts`'s printer would
/// otherwise render two levels deeper, at depth 3/6-space indent) — a real,
/// currently-shipped indentation quirk, not a hypothetical: confirmed
/// byte-for-byte against `212_json_codec/expected.ts`'s own
/// `deserialise_Status` (`if (typeof obj["tracking"] !== "string")` sits at
/// 2-space indent, one level, directly inside `case "Shipped": {`'s own
/// 6-space-indented block) and `407_workers_generic_sum_boundary`'s own
/// `deserialise_ApiResult_User` (identical shape for a generic-sum
/// instantiation). This slice's own zero-diff mandate rules out simply
/// letting the switch case render these at their structurally-correct
/// depth instead — that would be a real, deliberate formatting change with
/// no fixture-corpus backing either way, unlike the `(value as any)`
/// investigation this same issue explicitly authorises (see
/// `emit_sum_codec`'s own doc). `TsStmtKind::Raw`'s own doc
/// (`bynk-ts/src/program.rs`) names exactly this shape as a legitimate
/// third reason to use it, alongside its two existing ones (`lower.rs`'s
/// permanent exclusion; `emit_refined_type`'s own already-tree-sourced
/// splice): real, already-tree-native statement text this call site cannot
/// restructure into a properly-nested `Vec<TsStmt>` without changing bytes
/// this slice is not the one authorised to change.
fn raw_stmts_at_depth_one(stmts: Vec<TsStmt>) -> TsStmt {
    let mut text = String::new();
    for stmt in &stmts {
        text.push_str(&bynk_ts::print_stmt(stmt, 1));
    }
    TsStmt::raw(text, None)
}

/// `Err({ kind: "StructuralMismatch", path: <path_expr>, expected:
/// <expected>, actual: <actual> })` — the one wire-guard failure shape
/// every real arm of [`emit_field_deserialise_wire`] returns. Distinct from
/// `deserialise_expr_via`'s own `structural_mismatch` closure further down
/// this file in two ways that matter: `path` here is spliced in as an
/// already-rendered TS expression (`ident(path_expr)`), not a string
/// literal (`str_lit`) — this function's own `path_expr` parameter is
/// itself TS source text (a bare `path` identifier, or a template literal
/// like `` `${path}.value` ``), never a bare string to quote; and the
/// object is never cast `as BoundaryError` the way that closure's is (the
/// pre-conversion `writeln!` output this replaces never added one either —
/// only `deserialise_expr_via`'s own callers need the cast, since they
/// build a whole `Result<T, BoundaryError>` return value directly).
fn err_structural_mismatch(path_expr: &str, expected: &str, actual: TsExpr) -> TsExpr {
    call(
        ident("Err"),
        vec![TsExpr::object(vec![
            ("kind".to_string(), str_lit("StructuralMismatch")),
            ("path".to_string(), ident(path_expr)),
            ("expected".to_string(), str_lit(expected)),
            ("actual".to_string(), actual),
        ])],
    )
}

/// `Err({ kind: "StructuralMismatch", path, expected: <expected>, actual:
/// <actual> })` — [`emit_bytes_named_codec`]/[`emit_refined`]'s own
/// top-level boundary-guard failure shape (#1441, Arc E slice 4).
///
/// Deliberately a *second*, distinct helper rather than a call to
/// [`err_structural_mismatch`] above: that one's own `path_expr` parameter
/// is real call-site-supplied TS source text (a bare `path` identifier or a
/// template literal like `` `${path}.value` ``), rendered as an ordinary
/// `path: <path_expr>` property because every one of its real callers
/// (`emit_field_deserialise_wire`'s own field-level guards) passes a
/// rendered *sub*-path template, never the bare top-level `path` parameter
/// itself — confirmed empirically (no fixture anywhere renders the literal
/// text `path: path`). Every real call site *here*, by contrast, validates
/// a function's own top-level `path` parameter directly, which TypeScript's
/// object-literal shorthand prints as bare `path,` — confirmed byte-for-byte
/// against `bynkc/tests/fixtures/positive/254_multi_file_commons_workers_
/// codec/expected/money/cents.ts:26` (`emit_refined`'s own pre-conversion
/// text). [`TsExpr::object`]'s convenience constructor only builds
/// [`TsObjectEntry::Prop`] entries, which would print `path: path` instead
/// — a different, wrong byte sequence — so this builds the mixed
/// `Prop`/[`TsObjectEntry::Shorthand`] entry list directly.
fn err_structural_mismatch_top(expected: &str, actual: TsExpr) -> TsExpr {
    call(
        ident("Err"),
        // Review of #1442, finding 3: `TsExpr::object_entries` already
        // builds exactly this (a single-line object from a
        // `Vec<TsObjectEntry>`) — `TsExpr::object`'s own doc only rules out
        // the *other* convenience constructor, which takes `(String,
        // TsExpr)` pairs and can't represent `Shorthand`.
        vec![TsExpr::object_entries(vec![
            TsObjectEntry::Prop("kind".to_string(), str_lit("StructuralMismatch")),
            TsObjectEntry::Shorthand("path".to_string()),
            TsObjectEntry::Prop("expected".to_string(), str_lit(expected)),
            TsObjectEntry::Prop("actual".to_string(), actual),
        ])],
    )
}

/// Boundary-print — splices a real `Vec<bynk_ts::TsStmt>` into a still-
/// `write!`-based caller's own `out: &mut String` accumulator, one
/// `bynk_ts::print_stmt(&stmt, 1)` per statement. `depth: 1` is the
/// one-level indent (`"  "`) every real call site's own pre-conversion
/// `writeln!` calls used — confirmed against a real fixture's
/// before/after diff (#1439), not assumed, the same "verify, don't
/// assume" discipline #1437's own qualifier-prefix convention needed.
/// All 6 remaining real call sites (`emit_generic_helpers_qualified`'s own
/// still-`String`-based `ResultInst`/`OptionInst`/etc. arms) sit at exactly
/// this depth: a generated function's own top-level statement list. #1443
/// (Arc E slice 5): `emit_record_codec` no longer calls this — it builds a
/// real `Vec<TsStmt>` body directly now, so its own internal
/// `emit_field_deserialise_wire` calls `extend` straight in (no print/
/// re-parse round trip), dropping the count from 8 to 7. #1445 (Arc E
/// slice 6): `emit_sum_codec` no longer calls this either — its own
/// deserialise-side payload case needs the exact same depth-1 splice, but
/// pre-renders it into one `TsStmt::Raw` via [`raw_stmts_at_depth_one`]
/// instead (that function's own doc explains why: the surrounding function
/// body is itself a real `Vec<TsStmt>` now, not a `String`, so there is no
/// `out` here to push into) — dropping the count from 7 to 6.
fn splice_stmts(out: &mut String, stmts: Vec<TsStmt>) {
    // Review of #1440: every other boundary-print site in `bynk-emit`
    // spells this `push_str`, not `write!` — matches that convention (an
    // infallible append needs no `.unwrap()`, and doesn't itself add a
    // `write!`-family line to the very `ts_writes` probe this slice drives
    // down).
    for stmt in &stmts {
        out.push_str(&bynk_ts::print_stmt(stmt, 1));
    }
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

/// #1437 (Arc E slice 2): [`crate::emitter::ts_type_ref_qualified_multi_ts_type`]
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
        crate::emitter::ts_type_ref_qualified_multi_ts_type(t, &bare)
    } else {
        crate::emitter::ts_type_ref_qualified_multi_ts_type(&rename_type_ref(t, &whole), &bare)
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
    /// run in `bynkc/tests/coverage_behaviour.rs`.
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
    BaseGuard, JsonKind, MAX_RECURSIVE_NESTING, Provenance, Revalidation, UncheckedReason,
    WireBody, WireField, WireRef, WireSum, WireType, WireVariant, recursive_codec_suffixes,
    wire_ref, wire_type,
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
/// [`emit_helpers_for_owner`]/[`emit_generic_helpers`] (and their
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
pub(crate) fn decls_as_stmts(decls: Vec<TsDecl>) -> Vec<TsStmt> {
    decls.into_iter().map(|d| TsStmt::decl(d, None)).collect()
}

/// As [`decls_as_stmts`], plus one *extra* forced trailing blank `TsStmt`
/// when anything was collected — the shared `if emitted_any { ... }`
/// trailer every [`emit_helpers_for_owner`]/[`emit_helpers_for_owner_
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
pub(crate) fn decls_as_stmts_block(decls: Vec<TsDecl>) -> Vec<TsStmt> {
    let emitted_any = !decls.is_empty();
    let mut stmts = decls_as_stmts(decls);
    if emitted_any {
        let mut extra_blank = TsStmt::blank(None);
        extra_blank.no_blank_before = true;
        stmts.push(extra_blank);
    }
    stmts
}

/// #661: as [`emit_helpers_for_owner`], but the caller supplies a type
/// `Qual`. With an empty qualifier this is the owner's own module (every
/// type named bare, refined validation through `.of`). With a non-empty one it
/// is a *consumer* generating its own view of another context's boundary
/// types: the qualified names reach through the `import type * as <ns>` alias,
/// and refined validation inlines (transparent) or casts structurally (opaque)
/// because the owner's `.of` is not importable.
pub(crate) fn emit_helpers_for_owner_qualified(
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

/// The TS token a [`JsonKind`] spells as. `json_kind_of` (`bynk_check::wire`)
/// replaces `ts_base_for_serialisation`'s *classification* of a `BaseType`;
/// this is the TS-spelling half the wire.rs seam keeps in `bynk-emit`. Also
/// doubles as the boundary `typeof` check string — for every `BaseType` the
/// two coincide (a bare `Int`/`Float`/`Duration`/`Instant`/`String`/`Bytes`/
/// `Bool` field is validated against exactly the JSON `typeof` its kind
/// implies), so a single call replaces what used to be two identical `match
/// base` blocks.
fn json_kind_ts(k: JsonKind) -> &'static str {
    match k {
        JsonKind::Number => "number",
        JsonKind::String => "string",
        JsonKind::Boolean => "boolean",
        JsonKind::Object => "object",
        JsonKind::Array => "array",
        JsonKind::Null => "null",
    }
}

/// `path: string = "$"` — the one JS default-valued parameter anywhere in
/// this crate's generated output (#1441, Arc E slice 4). [`TsParam`] carries
/// no `default: Option<TsExpr>` field (its own doc names only `optional` —
/// "nothing in the grounding file needs a default value"), and adding one
/// would touch every one of the ~120 existing `TsParam { .. }` literal
/// construction sites across `bynk-emit`/`bynk-ts` for the sake of this
/// single real occurrence — wildly disproportionate to a 4-function slice.
/// `name` is already an established "raw-text slot" for a case bigger than
/// a bare identifier: [`bynk_ts::TsDecl::Import`]'s own `names` field docs
/// the identical move for a `"type KVNamespace"` specifier. Splicing the
/// whole clause in as one opaque `name` (with `ty: None`, so
/// `render_params` appends nothing further) reuses that precedent rather
/// than inventing a new one.
fn deserialise_path_param() -> TsParam {
    TsParam {
        name: "path: string = \"$\"".to_string(),
        ty: None,
        optional: false,
    }
}

/// #2656: the set of codec suffixes that carry a nesting depth
/// ([`bynk_check::wire::recursive_codec_suffixes`]), computed once per
/// emission call from the type table. A codec on a type cycle takes a trailing
/// `depth: number = 0` parameter and passes it on to every callee in the set;
/// every other codec is emitted exactly as before.
type DepthCodecs = std::collections::HashSet<String>;

/// #2656: how one codec body passes the nesting depth to its callees. `None`
/// for a codec off every type cycle, which has no `depth` in scope and calls
/// every codec with the default.
#[derive(Clone, Copy)]
struct Depth<'a> {
    codecs: &'a DepthCodecs,
    /// The depth argument a callee in `codecs` receives: `depth + 1` from a
    /// named type's codec, which is one level of nesting, and `depth` from a
    /// container's (`List_Dir`), which is not.
    next: &'static str,
}

/// #2656: the `depth` a codec passes on, or `None` when `depth` is not in
/// scope or `suffix` does not take one.
fn depth_arg(depth: Option<Depth<'_>>, suffix: &str) -> Option<TsExpr> {
    depth
        .filter(|d| d.codecs.contains(suffix))
        .map(|d| ident(d.next))
}

/// #2656: the [`Depth`] context for the body of the codec named `suffix`:
/// `Some` only when that codec is on a type cycle. `level` is true for a named
/// type's codec (record or sum, generic instantiations included), which counts
/// one level, and false for a container's.
fn depth_for<'a>(codecs: &'a DepthCodecs, suffix: &str, level: bool) -> Option<Depth<'a>> {
    codecs.contains(suffix).then_some(Depth {
        codecs,
        next: if level { "depth + 1" } else { "depth" },
    })
}

/// #2656: `depth: number = 0`, the trailing parameter of a codec on a type
/// cycle — raw text, as [`deserialise_path_param`] is, for the same reason.
fn depth_param() -> TsParam {
    TsParam {
        name: "depth: number = 0".to_string(),
        ty: None,
        optional: false,
    }
}

/// `json: __JsonValue, path: string = "$"`, plus [`depth_param`] for a codec
/// on a type cycle (#2656): the parameters of every generated decoder.
fn deserialise_params(depth: Option<Depth<'_>>) -> Vec<TsParam> {
    let mut params = vec![
        TsParam {
            name: "json".to_string(),
            ty: Some(TsType::named("__JsonValue")),
            optional: false,
        },
        deserialise_path_param(),
    ];
    if depth.is_some() {
        params.push(depth_param());
    }
    params
}

/// `value: <ty>`, plus [`depth_param`] for a codec on a type cycle (#2656):
/// the parameters of every generated encoder.
fn serialise_params(ty: TsType, depth: Option<Depth<'_>>) -> Vec<TsParam> {
    let mut params = vec![TsParam {
        name: "value".to_string(),
        ty: Some(ty),
        optional: false,
    }];
    if depth.is_some() {
        params.push(depth_param());
    }
    params
}

/// #2656: the text of the nesting bound's test, `depth >= <bound>`. Raw text,
/// as `TsBinaryOp` has no `>=`.
fn depth_exceeded() -> TsExpr {
    ident(format!("depth >= {MAX_RECURSIVE_NESTING}"))
}

/// #2656: the `expected` of the nesting bound's `StructuralMismatch`.
fn depth_expected() -> String {
    format!("a value nested at most {MAX_RECURSIVE_NESTING} levels deep")
}

/// #2656: the first statement of a recursive type's decoder: a value nested
/// past the bound is a `StructuralMismatch` at its own path, so the boundary
/// answers it as it answers any other malformed value (an HTTP `400`, an
/// `Err` from `Json.decode`), rather than recursing until the host's stack
/// overflows.
fn depth_guard_decode() -> TsStmt {
    if_(
        depth_exceeded(),
        block(vec![return_(err_structural_mismatch_top(
            &depth_expected(),
            str_lit("a value nested more deeply"),
        ))]),
    )
}

/// #2656: the first statement of a recursive type's encoder: a value nested
/// past the bound is a contract violation and throws, as a non-finite `Float`
/// does, so a context never sends or stores a value its peer, or its own
/// rehydration, must refuse.
fn depth_guard_encode() -> TsStmt {
    if_(
        depth_exceeded(),
        TsStmt::throw_stmt(
            TsExpr::New {
                callee: Box::new(ident("globalThis.Error")),
                args: vec![str_lit(format!(
                    "value nested more than {MAX_RECURSIVE_NESTING} levels deep at boundary"
                ))],
            },
            None,
        ),
    )
}

/// `{ [k: string]: JsonValue }` — the structural type
/// `emit_record_codec`'s own `const obj = json as { ... };` cast needs
/// before indexing it by field name. #1443 (Arc E slice 5): the identical
/// shape `workers_entry.rs`'s own `index_signature_record_ty` already
/// builds for its multi-param `on call` dispatch — kept as a second, local
/// copy rather than shared cross-file, matching this file's own "private
/// builder set" convention (#1435's own doc) rather than promoting a
/// two-caller shape into `bynk-ts` itself.
fn index_signature_record_ty() -> TsType {
    TsType::Object(vec![TsTypeMember::index(
        "k",
        TsType::named("string"),
        TsType::named("__JsonValue"),
    )])
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
        params: vec![TsParam {
            name: "value".to_string(),
            ty: Some(TsType::named(ty.clone())),
            optional: false,
        }],
        return_type: Some(TsType::named("__JsonValue")),
        body: vec![return_(call(
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
            TsParam {
                name: "json".to_string(),
                ty: Some(TsType::named("__JsonValue")),
                optional: false,
            },
            deserialise_path_param(),
        ],
        return_type: Some(TsType::named(format!("Result<{ty}, __BoundaryError>"))),
        body: vec![
            if_(
                strict_neq(typeof_expr(ident("json")), str_lit("string")),
                block(vec![return_(err_structural_mismatch_top(
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
                block(vec![return_(err_structural_mismatch_top(
                    "base64 string",
                    str_lit("invalid base64"),
                ))]),
            ),
            return_(call(
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
/// `.of` probe (below) stays one opaque [`TsStmt::raw`] block: its inline
/// object type (`{ of?: (json: unknown) => Result<unknown,
/// ValidationError> }`) needs a *named* function-type parameter (`json`),
/// but [`bynk_ts::TsType::Fn`]'s own doc is explicit that it only ever
/// numbers parameters positionally (`a0`, `a1`, …) — real TS grammar
/// requires *some* name, but not this exact one, so building it as a real
/// `TsType::Fn` would print `(a0: unknown) => ...` and break zero-diff. The
/// ternary itself is multi-line (`bynk_ts::TsExpr::Conditional`'s own
/// printer always renders `test ? consequent : alternate` on one line, no
/// multiline form). Both are real, checked gaps in the existing algebra —
/// not a default to opaque without checking — and closing either is
/// disproportionate to this one call site, so the whole `const validated =
/// …;` assignment stays hand-formatted text, the same "genuinely out of
/// scope" precedent `workers.rs`'s own `claim_predicate_to_js` call already
/// set (#1321). The `if (validated.tag === "Err") { … }
/// return Ok(validated.value as {name});` tail that follows has no such
/// gap, so it builds real nodes like everything else in this function.
fn emit_refined(
    name: &str,
    decl: &TypeDecl,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    qual: &Qual,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    let ty = qual_type(qual, name);
    let prov = qual_provenance(qual, name);
    let scalar = match wire_type(name, decl, types, prov) {
        Some(WireType {
            body: WireBody::Scalar(s),
            ..
        }) => s,
        _ => unreachable!(
            "emit_refined is only ever called for a non-generic Refined/Opaque declaration"
        ),
    };

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
        params: vec![TsParam {
            name: "value".to_string(),
            ty: Some(TsType::named(ty.clone())),
            optional: false,
        }],
        return_type: Some(TsType::named("__JsonValue")),
        body: vec![return_(as_expr(
            as_expr(ident("value"), TsType::named("unknown")),
            TsType::named(prim),
        ))],
        is_async: false,
        inline: false,
    }));

    let mut body = vec![if_(
        strict_neq(typeof_expr(ident("json")), str_lit(typeof_str)),
        block(vec![return_(err_structural_mismatch_top(
            typeof_str,
            typeof_expr(ident("json")),
        ))]),
    )];
    match scalar.revalidation {
        Revalidation::StructuralOnly => {
            // Decision C: structural cast only — never reach for the owner's
            // `.of`, which would resurrect the value import this increment
            // removes and leak the opaque predicate into the consumer.
            body.push(return_(call(
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
            body.push(return_(call(
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
            // P7.2: `name`'s declared type doesn't statically know about a
            // conditional `.of` constructor (that's the whole reason this probes
            // for it at runtime) — a marker type stating exactly the shape this
            // code actually depends on, not a blanket escape. `.error`'s real
            // type is `ValidationError` (a real, exported runtime interface,
            // `runtime/src/errors.ts`, already referenced by name elsewhere in
            // this file) — a first attempt typed it `unknown` and broke real
            // `tsc --strict` fixtures against this function's own declared
            // `violation: ValidationError` return shape. The `else` branch is
            // annotated to match the same `Result<{name}, ValidationError>`
            // union so both ternary arms unify to one type instead of one
            // arm's `Ok(...)` inferring a narrower `Result<{name}, never>`.
            //
            // This whole statement stays one opaque `TsStmt::raw` block — see
            // this function's own doc for why (a real, checked gap in both
            // `TsType::Fn`'s named-parameter support and
            // `TsExpr::Conditional`'s single-line-only printer, not a default
            // to opaque). Baked-in indent matches this body's own one-level
            // depth exactly, the same contract `print_stmt`'s own doc holds
            // every `Raw` statement to.
            body.push(TsStmt::raw(
                format!(
                    "  const validated = (typeof ({name} as unknown as {{ of?: (json: unknown) => Result<unknown, ValidationError> }}).of === \"function\")\n    ? ({name} as unknown as {{ of: (json: unknown) => Result<unknown, ValidationError> }}).of(json)\n    : (Ok(json as unknown as {name}) as Result<unknown, ValidationError>);\n"
                ),
                None,
            ));
            body.push(if_(
                strict_eq(member(ident("validated"), "tag"), str_lit("Err")),
                block(vec![return_(call(
                    ident("Err"),
                    vec![TsExpr::object_entries(vec![
                        TsObjectEntry::Prop("kind".to_string(), str_lit("RefinementViolation")),
                        TsObjectEntry::Shorthand("path".to_string()),
                        TsObjectEntry::Prop(
                            "violation".to_string(),
                            member(ident("validated"), "error"),
                        ),
                    ])],
                ))]),
            ));
            body.push(return_(call(
                ident("Ok"),
                vec![as_expr(
                    member(ident("validated"), "value"),
                    TsType::named(name.to_string()),
                )],
            )));
        }
        Revalidation::Base64Decode => unreachable!("handled by the early return above"),
    }

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{name}"),
        generics: Vec::new(),
        params: vec![
            TsParam {
                name: "json".to_string(),
                ty: Some(TsType::named("__JsonValue")),
                optional: false,
            },
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
        return_(call(
            ident("Err"),
            vec![TsExpr::object_entries(vec![
                TsObjectEntry::Prop("kind".to_string(), str_lit("RefinementViolation")),
                TsObjectEntry::Shorthand("path".to_string()),
                TsObjectEntry::Prop(
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
/// the field's own path (`path_expr`, a template literal).
fn emit_field_refinement_checks(
    fname: &str,
    predicates: &[PredKind],
    path_expr: &str,
) -> Vec<TsStmt> {
    let value = format!("__{fname}");
    let violation = |msg: &str| -> TsStmt {
        return_(call(
            ident("Err"),
            vec![TsExpr::object(vec![
                ("kind".to_string(), str_lit("RefinementViolation")),
                ("path".to_string(), ident(path_expr)),
                (
                    "violation".to_string(),
                    TsExpr::object(vec![
                        ("field".to_string(), str_lit(fname)),
                        (
                            "message".to_string(),
                            TsExpr::Lit(TsLit::Raw(format!("\"{msg}\""))),
                        ),
                        ("value".to_string(), ident(value.clone())),
                    ]),
                ),
            ])],
        ))
    };
    let mut stmts = Vec::new();
    for pred in bynk_syntax::ast::in_check_order(predicates, |p| p) {
        let (cond, msg) = super::pred_condition_and_message(pred, &value);
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
/// `body.fields` inline. `lower_field_default_wire` (this file) stays the
/// one place a default's *rendered* wire-JSON literal is produced, called
/// from `emit_record_codec` at the same point it always was (Part 1's
/// seam: the IR carries the raw default, the emitter renders it).
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
    let ts_type = qual_type(qual, name);
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
/// and its default as a raw `(Expr, TypeRef)`, lowered to its wire-JSON
/// literal right here via `lower_field_default_wire` — the same point it was
/// always called from, per Part 1's seam (`bynk-check` carries the boundary
/// fact, `bynk-emit` renders it). Events slice 3a (#972): a generic-record
/// instantiation's fields never carry a default (events are never generic),
/// so `default` is always `None` on that path; only `deserialise_<fn_suffix>`
/// consults it, `serialise_<fn_suffix>` is untouched (Decision B, #972).
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
    ts_type: &str,
    fields: &[WireField],
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    codecs: &DepthCodecs,
    ru: &RuntimeUse,
) -> Vec<TsDecl> {
    // #2656: a record on a type cycle threads the nesting depth.
    let depth = depth_for(codecs, fn_suffix, true);
    let mut serialise_params = vec![TsParam {
        name: "value".to_string(),
        ty: Some(TsType::named(ts_type)),
        optional: false,
    }];
    let mut serialise_body = Vec::new();
    if depth.is_some() {
        serialise_params.push(depth_param());
        serialise_body.push(depth_guard_encode());
    }
    serialise_body.push(return_(TsExpr::multiline_object(
        fields
            .iter()
            .map(|field| {
                (
                    field.name.clone(),
                    serialise_field_expr_wire(
                        &field.shape,
                        &format!("value.{}", field.name),
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
            block(vec![return_(err_structural_mismatch_top(
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
            .and_then(|(e, t)| lower_field_default_wire(e, t, types).ok());
        let fname = &field.name;
        let access = if let Some(d) = &default {
            // #1443 (Arc E slice 5): `"<field>" in obj ? obj["<field>"] :
            // <default>` — `d` is already-rendered wire-JSON literal *TS
            // source text* (`lower_field_default_wire`'s own contract, the
            // same "opaque, pre-rendered TS text" shape
            // `emit_inline_pred_check`'s own `cond` splice already
            // established in this file), spliced via `ident` rather than
            // re-derived as a real node — it is never re-parenthesised: as
            // a `Conditional`'s own `alternate`, it sits in an
            // `AssignmentExpression` grammar position, which (per
            // `bynk-ts/src/printer.rs`'s own `Conditional` render arm)
            // never adds parens around either branch regardless of that
            // branch's own shape.
            body.push(TsStmt::const_stmt(
                TsBindingName::Ident(format!("__d_{fname}")),
                Some(TsType::named("__JsonValue")),
                TsExpr::Conditional {
                    test: Box::new(in_expr(str_lit(fname.clone()), ident("obj"))),
                    consequent: Box::new(index(ident("obj"), str_lit(fname.clone()))),
                    alternate: Box::new(ident(d.clone())),
                },
                None,
            ));
            format!("__d_{fname}")
        } else {
            format!("obj[\"{fname}\"]")
        };
        let sub_path = format!("`${{path}}.{}`", field.path_segment);
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
    body.push(return_(call(
        ident("Ok"),
        vec![as_expr(
            TsExpr::object(
                fields
                    .iter()
                    .map(|field| (field.name.clone(), ident(format!("__{}", field.name))))
                    .collect(),
            ),
            TsType::named(ts_type),
        )],
    )));

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{fn_suffix}"),
        generics: Vec::new(),
        params: deserialise_params(depth),
        return_type: Some(TsType::named(format!("Result<{ts_type}, __BoundaryError>"))),
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
    let ty = qual_type(qual, name);
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
    ts_type: &str,
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
                    vec![return_(TsExpr::object(vec![(
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
                            &format!("value.{}", payload_prop(&field.name)),
                            "",
                            depth,
                            ru,
                        ),
                    ));
                }
                case_(
                    str_lit(vname.clone()),
                    vec![return_(TsExpr::object(entries))],
                    true,
                )
            }
        })
        .collect();
    let mut serialise_params = vec![TsParam {
        name: "value".to_string(),
        ty: Some(TsType::named(ts_type)),
        optional: false,
    }];
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
            block(vec![return_(err_structural_mismatch_top(
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
                    vec![return_(call(
                        ident("Ok"),
                        vec![as_expr(
                            TsExpr::object(vec![(tag.to_string(), str_lit(vname.clone()))]),
                            TsType::named(ts_type),
                        )],
                    ))],
                    false,
                )
            } else {
                let mut stmts = Vec::new();
                for field in &variant.payload {
                    let access = format!("obj[\"{}\"]", field.name);
                    let sub_path = format!("`${{path}}.{}`", field.path_segment);
                    // `raw_stmts_at_depth_one` (this file, above): preserves
                    // the pre-existing depth-1 splice quirk byte-for-byte —
                    // see its own doc for why this is a real, currently-
                    // shipped shape, not something this slice restructures.
                    stmts.push(raw_stmts_at_depth_one(emit_field_deserialise_wire(
                        &field.name,
                        &field.shape,
                        &access,
                        &sub_path,
                        depth,
                        ru,
                    )));
                }
                let mut entries = vec![(tag.to_string(), str_lit(vname.clone()))];
                for field in &variant.payload {
                    entries.push((
                        payload_prop(&field.name),
                        ident(format!("__{}", field.name)),
                    ));
                }
                stmts.push(return_(call(
                    ident("Ok"),
                    vec![as_expr(TsExpr::object(entries), TsType::named(ts_type))],
                )));
                case_(str_lit(vname.clone()), stmts, true)
            }
        })
        .collect();
    deserialise_cases.push(default_case(vec![return_(err_structural_mismatch_top(
        "sum variant kind",
        call(ident("String"), vec![ident(kind)]),
    ))]));
    deserialise_body.push(switch_stmt(ident(kind), deserialise_cases));

    let deserialise = TsDecl::Export(Box::new(TsDecl::Function {
        name: format!("__deserialise_{fn_suffix}"),
        generics: Vec::new(),
        params: deserialise_params(depth),
        return_type: Some(TsType::named(format!("Result<{ts_type}, __BoundaryError>"))),
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
    /// (`bynkc/tests/fixtures/positive/474_or_patterns`'s own
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
        let decls = emit_sum_codec("Parcel", "Parcel", &sum, &DepthCodecs::new(), &ru);
        assert_eq!(
            render_decls(decls),
            "export function __serialise_Parcel(value: Parcel): __JsonValue {\n  switch (value.tag) {\n    case \"Pending\":\n      return { kind: \"Pending\" };\n    case \"Shipped\": {\n      return { kind: \"Shipped\", tracking: value.tracking as __JsonValue, weight: ((v: number) => { if (!globalThis.Number.isSafeInteger(v)) throw new globalThis.Error(\"Int outside the safe-integer range at boundary\"); return v as __JsonValue; })(value.weight) };\n    }\n  }\n}\n\nexport function __deserialise_Parcel(json: __JsonValue, path: string = \"$\"): Result<Parcel, __BoundaryError> {\n  if (typeof json !== \"object\" || json === null || globalThis.Array.isArray(json)) {\n    return Err({ kind: \"StructuralMismatch\", path, expected: \"object\", actual: typeof json });\n  }\n  const obj = json as { [k: string]: __JsonValue };\n  const kind = obj[\"kind\"];\n  switch (kind) {\n    case \"Pending\":\n      return Ok({ tag: \"Pending\" } as Parcel);\n    case \"Shipped\": {\n  if (typeof obj[\"tracking\"] !== \"string\") {\n    return Err({ kind: \"StructuralMismatch\", path: `${path}.tracking`, expected: \"string\", actual: typeof obj[\"tracking\"] });\n  }\n  const __tracking = obj[\"tracking\"];\n  if (typeof obj[\"weight\"] !== \"number\") {\n    return Err({ kind: \"StructuralMismatch\", path: `${path}.weight`, expected: \"number\", actual: typeof obj[\"weight\"] });\n  }\n  if (!globalThis.Number.isSafeInteger(obj[\"weight\"])) {\n    return Err({ kind: \"StructuralMismatch\", path: `${path}.weight`, expected: \"safe integer\", actual: String(obj[\"weight\"]) });\n  }\n  const __weight = obj[\"weight\"];\n      return Ok({ tag: \"Shipped\", tracking: __tracking, weight: __weight } as Parcel);\n    }\n    default:\n      return Err({ kind: \"StructuralMismatch\", path, expected: \"sum variant kind\", actual: String(kind) });\n  }\n}\n"
        );
    }
}

/// Emit a let binding `__<field>` after destructuring & validating a
/// nested field.
///
/// #855 (Phase 2 step 6): dispatches on [`WireRef`] (via [`wire_ref_of`])
/// rather than matching `TypeRef` directly — the same resolution
/// `wire_ref` documents itself as mirroring one-for-one, **including** its
/// `Effect`/`HttpResult`/etc. field-position arm folding into the same
/// unchecked cast as `ValidationError`/`JsonError`/`QueueResult`. This is
/// the deserialise-side function `wire_ref`'s own doc names as the one it
/// agrees with exactly; contrast [`serialise_field_expr_via`], which does
/// not.
///
/// #855 (Phase 2 step 7): a thin `TypeRef` → [`WireRef`] wrapper over
/// [`emit_field_deserialise_wire`] — a record/sum field built from the IR
/// already carries its [`WireRef`] shape (`WireField::shape`) and calls that
/// directly, with no `TypeRef` to convert back from.
///
/// #1439 (Arc E slice 3): returns the same `Vec<bynk_ts::TsStmt>`
/// [`emit_field_deserialise_wire`] now does, rather than writing into an
/// `out: &mut String` — this function's only job is resolving `t` to a
/// [`WireRef`] before delegating, so it carries the signature change with
/// zero body change beyond the return.
fn emit_field_deserialise(
    name: &str,
    t: &TypeRef,
    json: &str,
    path_expr: &str,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> Vec<TsStmt> {
    emit_field_deserialise_wire(name, &wire_ref_of(t), json, path_expr, depth, ru)
}

/// The [`WireRef`]-driven body [`emit_field_deserialise`] delegates to,
/// exposed directly for a caller that already holds a [`WireRef`] (a
/// [`WireField`]'s `shape`) rather than the `TypeRef` it was resolved from.
///
/// #1439 (Arc E slice 3): returns a real `Vec<bynk_ts::TsStmt>` — a
/// *sequence* of statements (one or more guard `if`s, then a `const`
/// binding), not one expression, unlike slices 1/2's own `-> TsExpr`
/// conversions (#1436/#1438) — hence `Vec`, not a single node. Of this
/// function's 8 real call sites, 7 (`emit_sum_codec`,
/// `emit_generic_helpers_qualified`) are still `write!`-based, so each
/// splices the result back in via [`splice_stmts`] at the same one-level
/// indent the original `writeln!` calls always used, confirmed
/// byte-identical against the fixture corpus. The 8th, `emit_record_codec`,
/// became tree-native itself (#1443, Arc E slice 5) and now `extend`s the
/// returned statements directly into its own `Vec<TsStmt>` body — no print,
/// no splice, since both sides of that call are real nodes.
fn emit_field_deserialise_wire(
    name: &str,
    wire: &WireRef,
    json: &str,
    path_expr: &str,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> Vec<TsStmt> {
    match wire {
        // v0.110 (ADR 0142 D5): a bare `Bytes` field is a base64 JSON string —
        // require a string, then decode (rejecting invalid base64), binding the
        // decoded `Uint8Array`. This is the one base type whose wire value is
        // not a direct cast of its erased representation.
        WireRef::Bytes => {
            ru.note_bytes();
            vec![
                if_(
                    strict_neq(typeof_expr(ident(json)), str_lit("string")),
                    block(vec![return_(err_structural_mismatch(
                        path_expr,
                        "base64 string",
                        typeof_expr(ident(json)),
                    ))]),
                ),
                const_(
                    format!("__b_{name}"),
                    call(ident("__bynkBytesFromBase64"), vec![ident(json)]),
                ),
                if_(
                    strict_eq(member(ident(format!("__b_{name}")), "tag"), str_lit("None")),
                    block(vec![return_(err_structural_mismatch(
                        path_expr,
                        "base64 string",
                        str_lit("invalid base64"),
                    ))]),
                ),
                const_(
                    format!("__{name}"),
                    member(ident(format!("__b_{name}")), "value"),
                ),
            ]
        }
        WireRef::Base {
            json: kind, guards, ..
        } => {
            let typeof_str = json_kind_ts(*kind);
            let mut stmts = vec![if_(
                strict_neq(typeof_expr(ident(json)), str_lit(typeof_str)),
                block(vec![return_(err_structural_mismatch(
                    path_expr,
                    typeof_str,
                    typeof_expr(ident(json)),
                ))]),
            )];
            // v0.22b: bare `Int` fields validate integrality (ADR 0049) —
            // with `Float` in the language there is no excuse for a
            // fractional `Int` from the wire. v0.90 (ADR 0114 D7): an `Instant`
            // is whole epoch milliseconds, so it validates integrality too.
            // v0.21: boundary `Float` values are finite (ADR 0040) —
            // `JSON.parse("1e999")` yields `Infinity`, which must not be
            // admitted from the wire.
            for guard in guards {
                let (method, expected) = match guard {
                    // #1657: whole *and* within ±(2^53 − 1).
                    BaseGuard::Integral => ("isSafeInteger", "safe integer"),
                    BaseGuard::Finite => ("isFinite", "finite number"),
                };
                stmts.push(if_(
                    not_expr(call(
                        member(ident("globalThis.Number"), method),
                        vec![ident(json)],
                    )),
                    block(vec![return_(err_structural_mismatch(
                        path_expr,
                        expected,
                        call(ident("String"), vec![ident(json)]),
                    ))]),
                ));
            }
            stmts.push(const_(format!("__{name}"), ident(json)));
            stmts
        }
        // Named type (own module or generic instantiation, both keyed the
        // same way — `Named` for a declared type, `Inst` for `Result` /
        // `Option` / `List` / `Map` / a generic `App`): defer to its own
        // `deserialise_<key>`. Assumes it exists in scope (imported or
        // declared locally).
        // Review of #1440: `Named`/`Inst` differ only in which callee they
        // name (`type_name`/`key`, both a bare `String` — `bynk-check/src/
        // wire.rs`'s own `WireRef` fields) — collapsed via an or-pattern
        // rather than kept as two ~identical arms, the same "one shape, one
        // arm" discipline this tree's own exhaustiveness rules already
        // enforce for the printer side.
        WireRef::Named { name: type_name } | WireRef::Inst { key: type_name } => vec![
            const_(
                format!("__r_{name}"),
                call(
                    ident(format!("__deserialise_{type_name}")),
                    [ident(json), ident(path_expr)]
                        .into_iter()
                        .chain(depth_arg(depth, type_name))
                        .collect(),
                ),
            ),
            if_(
                strict_eq(member(ident(format!("__r_{name}")), "tag"), str_lit("Err")),
                return_(ident(format!("__r_{name}"))),
            ),
            const_(
                format!("__{name}"),
                member(ident(format!("__r_{name}")), "value"),
            ),
        ],
        WireRef::Unit => vec![const_(format!("__{name}"), ident("undefined"))],
        // The runtime-owned error family, plus a stray field-position
        // `Effect` (see this function's doc): no generated codec to name, so
        // the value is cast through unchecked.
        //
        // #1319: closes the `ts_any` residual for the four runtime-owned
        // error types — each now casts through its own real, exported
        // runtime type (`bynk-emit/runtime/src/errors.ts`/`queue.ts`/
        // `http.ts`), gated into the module's own import list the same way
        // `JsonError`/`HttpResult` already were (`write_header`'s
        // `file_mentions_json_error`/`file_mentions_http_result`, now joined
        // by `file_mentions_queue_result`; `ValidationError` is imported
        // unconditionally). `Effect` stays `any`: a stray field-position
        // `Effect[T]` genuinely has no shape to derive one from (this
        // function's own doc explains why it reaches here at all), and
        // narrowing it is not part of this residual — `unknown` would
        // compile but silently drop the fact that it's still open, which is
        // worse than leaving `any` visible.
        //
        // #1439 (Arc E slice 3): the cast is now a real `TsExpr::As` node
        // (nested `As`-under-`As` for the four bridged-through-`unknown`
        // reasons, mirroring `serialise_field_expr_wire`'s own
        // `WireRef::Unchecked` arm's identical shape) rather than a raw
        // `"as any"`/`"as unknown as X"` string glued onto `{json} {cast}`.
        // Per the accepted proposal's own explicit note: this moves the
        // *construction* of the `Effect` arm's `any` from a string to a
        // real node, but does NOT close the `ts_any` residual — `xtask`'s
        // probe was deliberately widened (#1322's review) to also match
        // `named("any"` in Rust source, so the count is expected to stay at
        // 31, not drop to 30.
        WireRef::Unchecked { reason } => {
            let value = match reason {
                UncheckedReason::Effect => as_expr(ident(json), TsType::named("any")),
                UncheckedReason::ValidationError => as_expr(
                    as_expr(ident(json), TsType::named("unknown")),
                    TsType::named("ValidationError"),
                ),
                UncheckedReason::JsonError => as_expr(
                    as_expr(ident(json), TsType::named("unknown")),
                    TsType::named("JsonError"),
                ),
                UncheckedReason::HttpResult => as_expr(
                    as_expr(ident(json), TsType::named("unknown")),
                    TsType::named_with_args("HttpResult", vec![TsType::named("unknown")]),
                ),
                UncheckedReason::QueueResult => as_expr(
                    as_expr(ident(json), TsType::named("unknown")),
                    TsType::named("QueueResult"),
                ),
                // #1883, slice 1a: unreachable in a checked program
                // (`bynk.types.asset_at_boundary`); total for the model.
                UncheckedReason::Asset => as_expr(
                    as_expr(ident(json), TsType::named("unknown")),
                    crate::emitter::asset_ts_type(),
                ),
            };
            vec![const_(format!("__{name}"), value)]
        }
    }
}

/// #1439 (Arc E slice 3): direct unit coverage for
/// [`emit_field_deserialise_wire`], byte-checked against the exact strings
/// its pre-conversion `writeln!` body used to produce. The project-form
/// fixture corpus reaches most of these arms already (`bless_positive_
/// fixtures`/`positive_fixtures` passing with zero fixture diff is the
/// stronger, end-to-end proof for those), but a real gap check (per this
/// issue's own "confirm this at implementation time" instruction) found
/// `WireRef::Unchecked { reason: UncheckedReason::Effect }` is NOT reached
/// by any real fixture — `1319_runtime_error_type_fields/expected/
/// errortypes.ts` exercises the other four `Unchecked` reasons (via a
/// record with one field of each runtime-owned error type), but a
/// field-position `Effect` never appears in that fixture or any other in
/// the corpus (a record/sum field can never legally *be* `Effect` per
/// `wire_ref`'s own doc — this arm is reached only via the `TypeRef` path
/// this file's callers don't exercise for that specific shape). This
/// module closes that gap directly rather than leaving the arm asserted
/// only by inspection.
#[cfg(test)]
mod emit_field_deserialise_wire_tests {
    use super::*;
    use bynk_check::wire::Expected;

    fn render(stmts: Vec<TsStmt>) -> String {
        stmts
            .iter()
            .map(|s| bynk_ts::print_stmt(s, 1))
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn unchecked_effect_casts_through_any() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Unchecked {
                reason: UncheckedReason::Effect,
            },
            "json",
            "path",
            None,
            &ru,
        );
        assert_eq!(render(stmts), "  const __name = json as any;\n");
    }

    #[test]
    fn unchecked_validation_error_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Unchecked {
                reason: UncheckedReason::ValidationError,
            },
            "json",
            "path",
            None,
            &ru,
        );
        assert_eq!(
            render(stmts),
            "  const __name = json as unknown as ValidationError;\n"
        );
    }

    #[test]
    fn unchecked_http_result_names_unknown_element_type() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Unchecked {
                reason: UncheckedReason::HttpResult,
            },
            "json",
            "path",
            None,
            &ru,
        );
        assert_eq!(
            render(stmts),
            "  const __name = json as unknown as HttpResult<unknown>;\n"
        );
    }

    #[test]
    fn unit_arm_binds_undefined() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire("name", &WireRef::Unit, "json", "path", None, &ru);
        assert_eq!(render(stmts), "  const __name = undefined;\n");
    }

    #[test]
    fn named_arm_propagates_the_error_branch() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire(
            "name",
            &WireRef::Named {
                name: "Foo".to_string(),
            },
            "json",
            "path",
            None,
            &ru,
        );
        assert_eq!(
            render(stmts),
            "  const __r_name = __deserialise_Foo(json, path);\n  if (__r_name.tag === \"Err\") return __r_name;\n  const __name = __r_name.value;\n"
        );
    }

    #[test]
    fn bytes_arm_decodes_base64_with_two_guards() {
        let ru = RuntimeUse::default();
        let stmts = emit_field_deserialise_wire("name", &WireRef::Bytes, "json", "path", None, &ru);
        assert_eq!(
            render(stmts),
            "  if (typeof json !== \"string\") {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"base64 string\", actual: typeof json });\n  }\n  const __b_name = __bynkBytesFromBase64(json);\n  if (__b_name.tag === \"None\") {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"base64 string\", actual: \"invalid base64\" });\n  }\n  const __name = __b_name.value;\n"
        );
    }

    #[test]
    fn base_arm_with_integral_and_finite_guards() {
        let ru = RuntimeUse::default();
        let wire = WireRef::Base {
            base: BaseType::Float,
            json: JsonKind::Number,
            guards: vec![BaseGuard::Integral, BaseGuard::Finite],
            expected: Expected::Json(JsonKind::Number),
        };
        let stmts = emit_field_deserialise_wire("name", &wire, "json", "path", None, &ru);
        assert_eq!(
            render(stmts),
            "  if (typeof json !== \"number\") {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"number\", actual: typeof json });\n  }\n  if (!globalThis.Number.isSafeInteger(json)) {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"safe integer\", actual: String(json) });\n  }\n  if (!globalThis.Number.isFinite(json)) {\n    return Err({ kind: \"StructuralMismatch\", path: path, expected: \"finite number\", actual: String(json) });\n  }\n  const __name = json;\n"
        );
    }
}

fn serialise_field_expr(
    t: &TypeRef,
    value: &str,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> TsExpr {
    if let TypeRef::Effect(inner, _) = t {
        return serialise_field_expr(inner, value, depth, ru);
    }
    serialise_field_expr_wire(&wire_ref_of(t), value, "", depth, ru)
}

/// The same dispatch, reaching its helpers through `ns` — `""` for a
/// module-local call, `"handlers."` from a Worker entry point that imports the
/// context's handlers as a namespace. Threading the prefix (rather than each
/// caller owning a parallel dispatch) is what keeps the boundary to **one**
/// codec path.
///
/// #855 (Phase 2 step 6): dispatches on [`WireRef`] (via [`wire_ref_of`]),
/// **except** `Effect`, which this function peels itself before consulting
/// the IR. `wire_ref`'s own doc names this exact asymmetry: a field-position
/// `Effect` is `Unchecked` under `wire_ref` (matching
/// [`emit_field_deserialise`], the resolver's one faithful consumer), but
/// this function has always *recursed* into the wrapped type instead — an
/// `Effect`-typed field serialises as its payload's codec, not as an opaque
/// cast. Routing `Effect` through `wire_ref` here would silently change that
/// to an unchecked cast, so it stays a manual peel rather than becoming a
/// second `WireRef` arm with no second consumer.
fn serialise_field_expr_via(t: &TypeRef, value: &str, ns: &str, ru: &RuntimeUse) -> TsExpr {
    if let TypeRef::Effect(inner, _) = t {
        return serialise_field_expr_via(inner, value, ns, ru);
    }
    serialise_field_expr_wire(&wire_ref_of(t), value, ns, None, ru)
}

/// A number crossing the boundary, checked with `Number.<check>` first: a
/// self-contained IIFE that throws `message` on failure, so the module needs no
/// extra runtime import.
///
/// #1435 (Arc E slice 1): the first real [`TsArrowBody::Block`] site in this
/// tree — a statement body (`if`/`throw`, then `return`), not reducible to one
/// expression the way every other arm here is. See that type's own doc
/// (`bynk-ts/src/program.rs`) for why this widens the existing `Arrow.body`
/// field rather than adding a new `TsExpr` variant.
fn guarded_number(value: &str, check: &str, message: &str) -> TsExpr {
    call(
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "v".to_string(),
                ty: Some(TsType::named("number")),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Block(vec![
                TsStmt::if_stmt(
                    TsExpr::Unary {
                        op: TsUnaryOp::Not,
                        expr: Box::new(call(
                            member(ident("globalThis.Number"), check),
                            vec![ident("v")],
                        )),
                    },
                    TsStmt::throw_stmt(
                        TsExpr::New {
                            callee: Box::new(ident("globalThis.Error")),
                            args: vec![str_lit(message)],
                        },
                        None,
                    ),
                    None,
                ),
                TsStmt::return_stmt(
                    Some(as_expr(ident("v"), TsType::named("__JsonValue"))),
                    None,
                ),
            ])),
        },
        vec![ident(value)],
    )
}

/// The [`WireRef`]-driven body [`serialise_field_expr_via`] delegates to
/// (after its manual `Effect` peel — see that function's doc), exposed
/// directly for a caller that already holds a [`WireRef`] (a [`WireField`]'s
/// `shape`) rather than the `TypeRef` it was resolved from. #855 (Phase 2
/// step 7): a record/sum field's shape can never legally be a field-position
/// `Effect` (non-storable, non-boundary — rejected before it could reach a
/// declared field), so calling this directly for such a field, skipping the
/// `Effect` peel above, is not a second disagreement with `wire_ref` — it is
/// the same dispatch on a shape the peel could never have matched anyway.
fn serialise_field_expr_wire(
    wire: &WireRef,
    value: &str,
    ns: &str,
    depth: Option<Depth<'_>>,
    ru: &RuntimeUse,
) -> TsExpr {
    match wire {
        // Named type or generic instantiation (`Result`/`Option`/`List`/`Map`/
        // a generic `App` all key the same way — see `wire_ref`'s doc):
        // serialise through its own `serialise_<key>`. #2656: a codec on a
        // type cycle passes the nesting depth on to a callee on one.
        WireRef::Named { name: key } | WireRef::Inst { key } => call(
            ident(format!("{ns}__serialise_{key}")),
            std::iter::once(ident(value))
                .chain(depth_arg(depth, key))
                .collect(),
        ),
        // v0.21: serialising a non-finite `Float` is a contract violation
        // (`JSON.stringify(NaN)` would silently produce `null`); the guard is
        // a self-contained IIFE (`guarded_number`).
        WireRef::Base {
            base: BaseType::Float,
            ..
        } => guarded_number(value, "isFinite", "non-finite Float at boundary"),
        // #1657 (Decision C): an `Int` outside the safe-integer range (`Infinity`
        // from `5 / 0` before the division trap, or a value past 2^53 from
        // arithmetic) would write `null` or an imprecise number, a silent
        // wire-contract violation the consumer's decoder then rejects. Fault
        // here instead, as a non-finite `Float` does.
        WireRef::Base {
            base: BaseType::Int,
            ..
        } => guarded_number(
            value,
            "isSafeInteger",
            "Int outside the safe-integer range at boundary",
        ),
        // #1969: a `Duration` or `Instant` is whole milliseconds in the `Int`
        // domain, and the receiver rejects anything else (`field_base_guards`),
        // so a fractional or unsafe one faults here, as an `Int` does, rather
        // than reaching the other side as a 400.
        WireRef::Base {
            base: b @ (BaseType::Duration | BaseType::Instant),
            ..
        } => guarded_number(
            value,
            "isSafeInteger",
            &format!("{} outside the safe-integer range at boundary", b.name()),
        ),
        // v0.110 (ADR 0142 D5): a `Bytes` is base64-encoded on the wire — the
        // one base type whose serialise is an encode, not a bare cast.
        WireRef::Bytes => {
            ru.note_bytes();
            as_expr(
                call(ident("__bynkBytesToBase64"), vec![ident(value)]),
                TsType::named("__JsonValue"),
            )
        }
        WireRef::Base { .. } => as_expr(ident(value), TsType::named("__JsonValue")),
        // The runtime-owned error types have no *generated* codec — they are
        // declared by the runtime, not by a `TypeDecl` this emitter can walk, so
        // there is no `serialise_ValidationError` to name. They keep the
        // pass-through the whole boundary used before this increment; unifying
        // the user-type paths does not reach them. Their JSON shape is fixed by
        // the runtime (`errors.ts`), so the cast is not *wrong* — it is simply
        // unchecked, and it is the one remaining unchecked arm at the boundary.
        // (`Effect` never lands here via `serialise_field_expr_via` — it is
        // peeled above; a record/sum field can never legally be `Effect` in
        // the first place — see this function's doc.)
        //
        // Review of #1319/#1320, finding 1's own tsc run (a real fixture with
        // one of these types finally reached this arm — none had before):
        // `value as JsonValue` fails `tsc --strict` (TS2352) for
        // `ValidationError`/`JsonError` specifically — real interfaces, not
        // structurally close enough to `JsonValue`'s own union for a direct
        // cast; `HttpResult<T>`/`QueueResult`'s own union shapes happen to
        // pass today, but that is a heuristic, not a guarantee this residual
        // should depend on. Bridging through `unknown` (the fix `tsc` itself
        // names) is safe and uniform for all five reasons, `Effect` included.
        //
        // A nested `As`-under-`As` (not one opaque double-cast string): safe
        // here because this function's own result is never rendered through
        // `render_operand` (nothing downstream uses it as a further Member/
        // Unary/Binary/Index operand) — every real caller either splices it
        // into a `Call`/`Object`/`Const` position via `render_expr`'s own
        // plain recursion, or stringifies the whole tree at a print boundary
        // (`bynk_ts::print_expr`), and `render_expr`'s own `As` arm only
        // parenthesises a `Binary`/`Arrow`/`Conditional` inner expression —
        // never a nested `As` — so this renders as the intended, parenless
        // `value as unknown as JsonValue`.
        WireRef::Unchecked { .. } => as_expr(
            as_expr(ident(value), TsType::named("unknown")),
            TsType::named("__JsonValue"),
        ),
        WireRef::Unit => TsExpr::Lit(TsLit::Null),
    }
}

// #855 (Phase 1): `inner_ts_name` moved to `bynk_check::wire` as
// `codec_suffix`; `collect_codec_closure` moved verbatim (pure, AST-only —
// see that module's doc for the seam). Re-exported under their original
// names so call sites here and elsewhere in `bynk-emit` keep compiling
// unchanged.
pub(crate) use bynk_check::wire::codec_suffix as inner_ts_name;
pub(crate) use bynk_check::wire::collect_codec_closure;

/// v0.22b: an expression-form serialise for a codec target — the same
/// dispatch as a record field's serialisation.
pub(crate) fn serialise_expr(t: &TypeRef, value: &str, ru: &RuntimeUse) -> TsExpr {
    serialise_field_expr(t, value, None, ru)
}

/// v0.176 (#642): the one serialise dispatch for the workers cross-context
/// boundary, reaching helpers through `ns`. Replaces the two parallel
/// dispatches this boundary used to carry — `emit.rs`'s `workers_serialise_expr`
/// (which dropped `List`/`Map` to a bare `as JsonValue` cast) and
/// `workers_entry.rs`'s `serialise_call` (which did the same to `Bytes`, the
/// asymmetry that forced `bynk.types.bytes_at_workers_boundary`).
pub(crate) fn serialise_expr_via(t: &TypeRef, value: &str, ns: &str, ru: &RuntimeUse) -> TsExpr {
    serialise_field_expr_via(t, value, ns, ru)
}

/// v0.176 (#642): a deserialise **reference** for `ns`, shaped to
/// `callService`'s `deserialiseResult` parameter. The inline arms become a
/// lambda rather than the unvalidated `((j: any) => ({ tag: "Ok", value: j }))`
/// identity the caller path used to fall back to.
pub(crate) fn deserialise_ref_via(t: &TypeRef, ns: &str, ru: &RuntimeUse) -> TsExpr {
    match strip_effect(t) {
        TypeRef::Named(id) => ident(format!("{ns}__deserialise_{}", id.name)),
        t @ (TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::List(..)
        | TypeRef::Set(..)
        | TypeRef::Map(..)
        | TypeRef::App { .. }) => ident(format!("{ns}__deserialise_{}", inner_ts_name(t))),
        other => TsExpr::Arrow {
            params: vec![TsParam {
                name: "__j".to_string(),
                ty: Some(TsType::named("__JsonValue")),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(deserialise_expr_via(
                other, "__j", "$", ns, ru,
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
        TypeRef::Named(id) => ident(format!("{ns}__serialise_{}", id.name)),
        t @ (TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::List(..)
        | TypeRef::Set(..)
        | TypeRef::Map(..)
        | TypeRef::App { .. }) => ident(format!("{ns}__serialise_{}", inner_ts_name(t))),
        other => TsExpr::Arrow {
            params: vec![TsParam {
                name: "__v".to_string(),
                ty: Some(TsType::named(crate::emitter::ts_type_ref(other))),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(serialise_field_expr_via(
                other, "__v", ns, ru,
            )))),
        },
    }
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
        | TypeRef::HttpResult(inner, _) => agent_wire_passes_through(inner),
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            agent_wire_passes_through(a) || agent_wire_passes_through(b)
        }
        TypeRef::App { args, .. } => args.iter().any(agent_wire_passes_through),
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => false,
    }
}

/// An `Effect[T]` in a handler signature wraps the *handler*, not the wire
/// payload — the caller awaits the Promise, so the codec is `T`'s.
fn strip_effect(t: &TypeRef) -> &TypeRef {
    match t {
        TypeRef::Effect(inner, _) => strip_effect(inner),
        other => other,
    }
}

/// v0.22b: an expression-form deserialise call for a codec target. Named
/// types and generic instantiations go through their (module-local)
/// helpers; bases inline the structural check.
pub(crate) fn deserialise_expr(t: &TypeRef, json: &str, path: &str, ru: &RuntimeUse) -> TsExpr {
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
    json: &str,
    path: &str,
    ns: &str,
    ru: &RuntimeUse,
) -> TsExpr {
    // Every arm except the delegating ones — which call a `deserialise_<T>` in the
    // module's own namespace — builds `Ok(…)` / `Err(… as BoundaryError)` inline.
    // Recorded once here rather than per-arm: the delegating set is short and
    // closed, the inlining set is long, and erring the other way emits a module
    // that references an unimported name (#914). `Effect` recurses, so it lets the
    // inner type decide.
    if !matches!(
        t,
        TypeRef::Named(_)
            | TypeRef::Result(..)
            | TypeRef::Option(..)
            | TypeRef::List(..)
            | TypeRef::Set(..)
            | TypeRef::Map(..)
            | TypeRef::App { .. }
            | TypeRef::Effect(..)
    ) {
        ru.note_boundary_codec();
    }
    // The runtime-owned-error-type arms below (`ValidationError`/`JsonError`/
    // `QueueResult`/`HttpResult`) all share this one shape: `Ok(json as
    // unknown as {ty}) as Result<{ty}, BoundaryError>` — the deserialise-side
    // mirror of `serialise_field_expr_wire`'s own `WireRef::Unchecked` arm
    // (see that arm's own doc for why a nested `As`-under-`As` is safe here:
    // this function's result is never rendered through `render_operand`).
    let ok_unknown_cast = |ty: &str| -> TsExpr {
        as_expr(
            call(
                ident("Ok"),
                vec![as_expr(
                    as_expr(ident(json), TsType::named("unknown")),
                    TsType::named(ty.to_string()),
                )],
            ),
            TsType::named(format!("Result<{ty}, __BoundaryError>")),
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
            ident(format!("{ns}__deserialise_{}", id.name)),
            vec![ident(json), str_lit(path)],
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
            vec![ident(json), str_lit(path)],
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
        TypeRef::ValidationError(_) => ok_unknown_cast("ValidationError"),
        TypeRef::JsonError(_) => ok_unknown_cast("JsonError"),
        // Review of #1319/#1320, finding 1: `HttpResult<T>` (`bynk-emit/
        // runtime/src/http.ts:6`) is generic with no default type argument
        // — a bare `HttpResult` in type position is `tsc`'s TS2314, not the
        // `any`-compatible cast the other three residual types get. This
        // arm already holds the element type (`TypeRef::HttpResult(Box<
        // TypeRef>, Span)`, `bynk-syntax/src/ast.rs:2196`), so it renders
        // through `ts_type_ref` — the same real-type-name printer every
        // other `HttpResult<...>` position in this file already uses —
        // instead of dropping the payload.
        TypeRef::HttpResult(inner, _) => {
            let ty = format!("HttpResult<{}>", crate::emitter::ts_type_ref(inner));
            ok_unknown_cast(&ty)
        }
        TypeRef::QueueResult(_) => ok_unknown_cast("QueueResult"),
        // #1883, slice 1a: unreachable in a checked program
        // (`bynk.types.asset_at_boundary`).
        TypeRef::Asset(_) => ok_unknown_cast("{ readonly url: string }"),
        // v0.110 (ADR 0142 D5): a `Bytes` wires as a base64 string; decode it
        // (rejecting a non-string or invalid base64) to a `Uint8Array`.
        TypeRef::Base(BaseType::Bytes, _) => {
            ru.note_bytes();
            call(
                TsExpr::Arrow {
                    params: vec![TsParam {
                        name: "__v".to_string(),
                        ty: None,
                        optional: false,
                    }],
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
                                params: vec![TsParam {
                                    name: "__b".to_string(),
                                    ty: None,
                                    optional: false,
                                }],
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
                                            "base64 string",
                                            str_lit("invalid base64"),
                                        )),
                                    },
                                ))),
                            },
                            vec![call(ident("__bynkBytesFromBase64"), vec![ident("__v")])],
                        )),
                        alternate: Box::new(structural_mismatch(
                            "base64 string",
                            TsExpr::Unary {
                                op: TsUnaryOp::Typeof,
                                expr: Box::new(ident("__v")),
                            },
                        )),
                    }))),
                },
                vec![ident(json)],
            )
        }
        TypeRef::Base(b, _) => {
            let typeof_str = match b {
                BaseType::Int => "number",
                BaseType::String => "string",
                BaseType::Bool => "boolean",
                BaseType::Float => "number",
                BaseType::Duration | BaseType::Instant => "number",
                // Unreachable: handled by the dedicated `Bytes` arm above.
                BaseType::Bytes => "string",
            };
            // `Some(predicate)`: the arm needs a second, narrower runtime check
            // beyond `typeof` — a real `TsExpr`, not text, now that `extra`'s
            // only past job (interpolating into a hand-built `format!`) is
            // gone.
            let predicate: Option<TsExpr> = match b {
                BaseType::Float => Some(call(member(ident("globalThis.Number"), "isFinite"), vec![ident("__v")])),
                // v0.86 (ADR 0112 D6): a `Duration` is whole milliseconds —
                // reject a non-integer from the wire, as a refined `Int` does.
                // #1657: whole *and* within ±(2^53 − 1), the `Int` domain.
                BaseType::Int | BaseType::Duration | BaseType::Instant => Some(call(
                    member(ident("globalThis.Number"), "isSafeInteger"),
                    vec![ident("__v")],
                )),
                _ => None,
            };
            // v0.176 (#642): report what was *required*, not just the `typeof`
            // that was tested. For the arms carrying a `predicate` the two
            // differ, and reporting the bare `typeof` makes the error useless in
            // exactly the case the predicate exists to catch: a `3.5` for an `Int`
            // would read `expected: "number", actual: "number"`.
            let expected = match b {
                BaseType::Int | BaseType::Duration | BaseType::Instant => "safe integer",
                BaseType::Float => "finite number",
                _ => typeof_str,
            };
            let typeof_v = || TsExpr::Unary {
                op: TsUnaryOp::Typeof,
                expr: Box::new(ident("__v")),
            };
            let Some(predicate) = predicate else {
                return call(
                    TsExpr::Arrow {
                        params: vec![TsParam {
                            name: "__v".to_string(),
                            ty: None,
                            optional: false,
                        }],
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
                    vec![ident(json)],
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
                    params: vec![TsParam {
                        name: "__v".to_string(),
                        ty: None,
                        optional: false,
                    }],
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
                vec![ident(json)],
            )
        }
        // Everything else is rejected by the checker's codec-domain rule (the
        // `Json` path) or by the boundary rules (the workers path). Shared by
        // three callers, so the message names the type rather than one caller.
        other => unreachable!("non-codable type reached a codec lowering: {other:?}"),
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
            bynk_ts::print_expr(&deserialise_expr(&t, "json", "path", &ru)),
            "Ok(json as unknown as ValidationError) as Result<ValidationError, __BoundaryError>"
        );
    }

    #[test]
    fn json_error_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let t = TypeRef::JsonError(sp());
        assert_eq!(
            bynk_ts::print_expr(&deserialise_expr(&t, "json", "path", &ru)),
            "Ok(json as unknown as JsonError) as Result<JsonError, __BoundaryError>"
        );
    }

    #[test]
    fn http_result_names_its_real_element_type_and_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let t = TypeRef::HttpResult(Box::new(TypeRef::Base(BaseType::Int, sp())), sp());
        assert_eq!(
            bynk_ts::print_expr(&deserialise_expr(&t, "json", "path", &ru)),
            "Ok(json as unknown as HttpResult<number>) as Result<HttpResult<number>, __BoundaryError>"
        );
    }

    #[test]
    fn queue_result_casts_through_unknown() {
        let ru = RuntimeUse::default();
        let t = TypeRef::QueueResult(sp());
        assert_eq!(
            bynk_ts::print_expr(&deserialise_expr(&t, "json", "path", &ru)),
            "Ok(json as unknown as QueueResult) as Result<QueueResult, __BoundaryError>"
        );
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

/// `if (typeof json !== "object" || json === null || Array.isArray(json)) {
/// return Err(...); } const obj = json as { [k: string]: JsonValue };` — the
/// exact 2-statement object-shape guard [`emit_record_codec`] already builds
/// inline (#1443), needed verbatim by [`emit_generic_helpers_qualified`]'s
/// own `ResultInst` arm (Arc E slice 7, #1447; the `OptionInst` arm used it
/// too until #1887 made `Option` decoding lenient, which must accept `null`
/// and non-object bare values this guard rejects). A local helper
/// here rather than a shared export, or a call into `emit_record_codec`
/// itself: matches this file's own "private builder set, not promoted
/// cross-function" precedent (#1435's own doc) — two real call sites inside
/// this one function is exactly the shape that precedent already covers,
/// and `emit_record_codec` has no reason to know this function exists.
fn object_shape_guard() -> Vec<TsStmt> {
    vec![
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
            block(vec![return_(err_structural_mismatch_top(
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
    ]
}

/// `if (!Array.isArray(json)) { return Err(...); }` — `ListInst`/`MapInst`'s
/// own shared array-shape guard (Arc E slice 7, #1447), the array-typed
/// sibling of [`object_shape_guard`] just above.
fn array_shape_guard() -> TsStmt {
    if_(
        not_expr(call(
            member(ident("globalThis.Array"), "isArray"),
            vec![ident("json")],
        )),
        block(vec![return_(err_structural_mismatch_top(
            "array",
            typeof_expr(ident("json")),
        ))]),
    )
}

/// `if (obj["kind"] === "<key_a>") { <guard_a><return_a> } else if
/// (obj["kind"] === "<key_b>") { <guard_b><return_b> }` — `ResultInst`'s
/// (`"Ok"`/`"Err"`) own two-armed wire-kind dispatch (Arc E slice 7, #1447).
/// `OptionInst` used it for `"Some"`/`"None"` until #1887, whose lenient
/// decoder ([`lenient_option_decode_body`]) needs no else-if cascade and is
/// built from real `if` nodes instead.
///
/// `bynk_ts::TsStmt` has no real "else if" continuation shape: the only way
/// to nest a second `If` as this shape's own `else_branch` is
/// [`TsStmt::if_else_same_line_stmt`], whose `} else <branch>` rendering
/// falls to `render_inline_stmt`'s own fallback for a bare, non-`Block`
/// `If` — which always renders at depth 0 (correct only for the genuinely
/// brace-free, single-line bodies that fallback was built for; nothing in
/// `events_fanout.rs`'s own grounding ever nested a multi-line block that
/// way). The real call site here needs `body_a`/`body_b` at depth 2
/// (4-space, correctly nested one level inside this cascade's own `if`),
/// which that depth-0 fallback cannot produce without mis-indenting them two
/// spaces shallow. Building the whole two-branch cascade as one
/// `TsStmt::Raw` sidesteps the gap entirely — the same "real,
/// already-tree-native content the algebra can't yet nest correctly at this
/// exact spot; print it and splice" move [`raw_stmts_at_depth_one`] already
/// established for `emit_sum_codec`'s own switch-case content (#1445).
/// `guard_a`/`guard_b`/`return_a`/`return_b` are still real
/// `Vec<TsStmt>`/`TsStmt` nodes built by this function's own real callers
/// (`emit_field_deserialise` and this arm's own `return_`/`call`/`as_expr`
/// composition) — only the surrounding "if (...) { } else if (...) { }"
/// skeleton itself is hand-assembled text, and even that skeleton's own two
/// conditions are real `TsExpr` nodes (`strict_eq`/[`index`]), rendered via
/// `bynk_ts::print_expr` rather than spelled out by hand.
///
/// `guard_a`/`guard_b` render at depth 1 (2-space) — one level shallower
/// than their true nesting, the same pre-existing indentation quirk
/// [`raw_stmts_at_depth_one`]'s own doc names for `emit_sum_codec`'s payload
/// guards (first confirmed byte-for-byte against the pre-#1887
/// `deserialise_Option_Int`, whose `if (obj["kind"] === "Some") {` was
/// immediately followed by a 2-space-indented `if (typeof obj["value"] !==
/// "number")`, not the 4-space a correctly-nested `if`-inside-an-`if` would
/// get).
/// `return_a`/`return_b` render at depth 2 (4-space) — the correctly-nested
/// depth for content one level inside the cascade, itself one level inside
/// the enclosing function's own top-level body (depth 1).
fn wire_kind_dispatch_raw(
    key_a: &str,
    guard_a: Vec<TsStmt>,
    return_a: TsStmt,
    key_b: &str,
    guard_b: Vec<TsStmt>,
    return_b: TsStmt,
) -> TsStmt {
    let wire_kind_eq = |key: &str| strict_eq(index(ident("obj"), str_lit("kind")), str_lit(key));

    let mut text = String::new();
    text.push_str("  if (");
    text.push_str(&bynk_ts::print_expr(&wire_kind_eq(key_a)));
    text.push_str(") {\n");
    splice_stmts(&mut text, guard_a);
    text.push_str(&bynk_ts::print_stmt(&return_a, 2));
    text.push_str("  } else if (");
    text.push_str(&bynk_ts::print_expr(&wire_kind_eq(key_b)));
    text.push_str(") {\n");
    splice_stmts(&mut text, guard_b);
    text.push_str(&bynk_ts::print_stmt(&return_b, 2));
    text.push_str("  }\n");
    TsStmt::raw(text, None)
}

/// The text appended to a bare value's `StructuralMismatch.expected` when it
/// fails to decode as an `Option[U]`'s `U` (#1887), so the error names every
/// form the `Option` decoder would have taken, not only `U`'s.
const OPTION_FORMS_SUFFIX: &str =
    " | null | {\"kind\": \"None\"} | {\"kind\": \"Some\", \"value\": ...}";

/// `left && right`, for the `Option` decoder's tagged-form recognition
/// (#1887). Left-folding prints flat, as [`or_expr`] does.
fn and_expr(left: TsExpr, right: TsExpr) -> TsExpr {
    binary(TsBinaryOp::And, left, right)
}

/// `Ok(<value> as Option<U>)`: the `Option` decoder's one success shape.
fn ok_option(value: TsExpr, option_ty: &TsType) -> TsStmt {
    return_(call(ident("Ok"), vec![as_expr(value, option_ty.clone())]))
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
                in_expr(str_lit("value"), ident("obj")),
            ),
            block(vec![
                const_(
                    "__t",
                    call(
                        ident(value_fn),
                        value_args(
                            index(ident("obj"), str_lit("value")),
                            ident("`${path}.value`"),
                        ),
                    ),
                ),
                if_(
                    strict_eq(member(ident("__t"), "tag"), str_lit("Err")),
                    block(vec![return_(ident("__t"))]),
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
            block(vec![return_(call(
                ident("Err"),
                vec![TsExpr::object_entries(vec![
                    TsObjectEntry::Spread(bare_error.clone()),
                    TsObjectEntry::Prop(
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
        return_(ident("__b")),
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
    let mut body = emit_field_deserialise("v", inner, "raw", "at", depth, ru);
    body.push(return_(call(ident("Ok"), vec![ident("__v")])));
    let mut params = vec![
        TsParam {
            name: "raw".to_string(),
            ty: Some(TsType::named("__JsonValue")),
            optional: false,
        },
        TsParam {
            name: "at".to_string(),
            ty: Some(TsType::named("string")),
            optional: false,
        },
    ];
    // #2656: on a type cycle, the `Option` decoder hands its depth through.
    if depth.is_some() {
        params.push(TsParam {
            name: "depth".to_string(),
            ty: Some(TsType::named("number")),
            optional: false,
        });
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

/// #661: as [`emit_generic_helpers`], but the value-type positions of each
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
pub(crate) fn emit_generic_helpers_qualified(
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
                let ts_type = format!(
                    "{}<{}>",
                    qual_type(qual, name),
                    args.iter()
                        .map(|a| bynk_ts::print_type(&qualified_ts_type(a, qual)))
                        .collect::<Vec<_>>()
                        .join(", ")
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
                // Events slice 3a (#972): a generic record instantiation
                // never carries a field default — events are never generic
                // (`parse_event_decl` always builds zero type params), so
                // this path can never reach one. #855 (Phase 2 step 9): a
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
                let declared: std::collections::HashMap<&str, Vec<bynk_syntax::ast::PredKind>> =
                    match types.get(name.as_str()).map(|d| &d.body) {
                        Some(bynk_syntax::ast::TypeBody::Record(r)) => r
                            .fields
                            .iter()
                            .map(|f| (f.name.name.as_str(), bynk_check::wire::field_predicates(f)))
                            .collect(),
                        _ => unreachable!("RecordInst `{name}` is not a resolved generic record"),
                    };
                let fields: Vec<WireField> = fields
                    .into_iter()
                    .map(|(n, t)| WireField {
                        shape: wire_ref_of(&t),
                        predicates: declared.get(n.as_str()).cloned().unwrap_or_else(|| {
                            unreachable!("`{name}` has no declared field `{n}`")
                        }),
                        path_segment: n.clone(),
                        name: n,
                        default: None,
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
                let ts_type = format!(
                    "{}<{}>",
                    qual_type(qual, name),
                    args.iter()
                        .map(|a| bynk_ts::print_type(&qualified_ts_type(a, qual)))
                        .collect::<Vec<_>>()
                        .join(", ")
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
                let ok_inner = bynk_ts::print_type(&qualified_ts_type(ok, qual));
                let err_inner = bynk_ts::print_type(&qualified_ts_type(err, qual));
                let result_ty = TsType::named_with_args(
                    "Result",
                    vec![TsType::named(ok_inner), TsType::named(err_inner)],
                );

                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_Result_{ok_ts}_{err_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(result_ty.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body: vec![
                        if_(
                            strict_eq(member(ident("value"), "tag"), str_lit("Ok")),
                            return_(TsExpr::object(vec![
                                ("kind".to_string(), str_lit("Ok")),
                                (
                                    "value".to_string(),
                                    serialise_field_expr(ok, "value.value", depth, ru),
                                ),
                            ])),
                        ),
                        return_(TsExpr::object(vec![
                            ("kind".to_string(), str_lit("Err")),
                            (
                                "error".to_string(),
                                serialise_field_expr(err, "value.error", depth, ru),
                            ),
                        ])),
                    ],
                    is_async: false,
                    inline: false,
                }));

                let mut body = object_shape_guard();
                body.push(wire_kind_dispatch_raw(
                    "Ok",
                    emit_field_deserialise(
                        "v",
                        ok,
                        "obj[\"value\"]",
                        "`${path}.value`",
                        depth,
                        ru,
                    ),
                    return_(call(
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
                        "obj[\"error\"]",
                        "`${path}.error`",
                        depth,
                        ru,
                    ),
                    return_(call(
                        ident("Ok"),
                        vec![as_expr(
                            call(ident("Err"), vec![ident("__e")]),
                            result_ty.clone(),
                        )],
                    )),
                ));
                body.push(return_(err_structural_mismatch_top(
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
                let inner_ty = bynk_ts::print_type(&qualified_ts_type(inner, qual));
                let inner_ty = TsType::named(inner_ty);
                let option_ty = TsType::named_with_args("Option", vec![inner_ty.clone()]);

                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_Option_{inner_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(option_ty.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body: vec![
                        if_(
                            strict_eq(member(ident("value"), "tag"), str_lit("Some")),
                            return_(TsExpr::object(vec![
                                ("kind".to_string(), str_lit("Some")),
                                (
                                    "value".to_string(),
                                    serialise_field_expr(inner, "value.value", depth, ru),
                                ),
                            ])),
                        ),
                        return_(TsExpr::object(vec![("kind".to_string(), str_lit("None"))])),
                    ],
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
                let elem_ty = TsType::named(bynk_ts::print_type(&qualified_ts_type(elem, qual)));
                let readonly_elem_array = TsType::readonly_array(elem_ty.clone());

                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_List_{elem_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(readonly_elem_array.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body: vec![return_(call(
                        member(ident("value"), "map"),
                        vec![TsExpr::Arrow {
                            params: vec![TsParam {
                                name: "v".to_string(),
                                ty: None,
                                optional: false,
                            }],
                            is_async: false,
                            generics: Vec::new(),
                            return_type: None,
                            body: Box::new(TsArrowBody::Expr(Box::new(serialise_field_expr(
                                elem, "v", depth, ru,
                            )))),
                        }],
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
                    "item",
                    "`${path}[${i}]`",
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
                            less_than(ident("i"), member(ident("json"), "length")),
                            block(vec![raw_stmts_at_depth_one(loop_body)]),
                            None,
                        ),
                        return_(call(ident("Ok"), vec![ident("out")])),
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
                let elem_ty = TsType::named(bynk_ts::print_type(&qualified_ts_type(elem, qual)));
                let set_ty = TsType::named_with_args("ReadonlySet", vec![elem_ty.clone()]);
                let set_ctor = bynk_ts::print_type(&TsType::named_with_args(
                    "globalThis.Set",
                    vec![elem_ty.clone()],
                ));

                let serialise = TsDecl::Export(Box::new(TsDecl::Function {
                    name: format!("__serialise_Set_{elem_ts}"),
                    generics: Vec::new(),
                    params: serialise_params(set_ty.clone(), depth),
                    return_type: Some(TsType::named("__JsonValue")),
                    body: vec![return_(call(
                        member(
                            call(ident("globalThis.Array.from"), vec![ident("value")]),
                            "map",
                        ),
                        vec![TsExpr::Arrow {
                            params: vec![TsParam {
                                name: "v".to_string(),
                                ty: None,
                                optional: false,
                            }],
                            is_async: false,
                            generics: Vec::new(),
                            return_type: None,
                            body: Box::new(TsArrowBody::Expr(Box::new(serialise_field_expr(
                                elem, "v", depth, ru,
                            )))),
                        }],
                    ))],
                    is_async: false,
                    inline: false,
                }));

                let mut loop_body = vec![const_("item", index(ident("json"), ident("i")))];
                loop_body.extend(emit_field_deserialise(
                    "el",
                    elem,
                    "item",
                    "`${path}[${i}]`",
                    depth,
                    ru,
                ));
                loop_body.push(if_(
                    call(
                        member(ident("out"), "has"),
                        vec![as_expr(ident("__el"), elem_ty.clone())],
                    ),
                    block(vec![return_(err_structural_mismatch(
                        "`${path}[${i}]`",
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
                                callee: Box::new(ident(set_ctor)),
                                args: vec![],
                            },
                        ),
                        TsStmt::for_stmt(
                            "i",
                            TsExpr::Lit(TsLit::Num("0".to_string())),
                            less_than(ident("i"), member(ident("json"), "length")),
                            block(vec![raw_stmts_at_depth_one(loop_body)]),
                            None,
                        ),
                        return_(call(ident("Ok"), vec![ident("out")])),
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
                let key_ty = TsType::named(bynk_ts::print_type(&qualified_ts_type(key, qual)));
                let val_ty = TsType::named(bynk_ts::print_type(&qualified_ts_type(val, qual)));
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
                let map_ctor = bynk_ts::print_type(&TsType::named_with_args(
                    "globalThis.Map",
                    vec![key_ty.clone(), val_ty.clone()],
                ));

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
                                        serialise_field_expr(key, "k", depth, ru),
                                        serialise_field_expr(val, "v", depth, ru),
                                    ])],
                                ),
                                None,
                            )]),
                            None,
                        ),
                        return_(ident("entries")),
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
                        block(vec![return_(err_structural_mismatch(
                            "`${path}[${i}]`",
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
                    "entryK",
                    "`${path}[${i}][0]`",
                    depth,
                    ru,
                ));
                loop_body.extend(emit_field_deserialise(
                    "v",
                    val,
                    "entryV",
                    "`${path}[${i}][1]`",
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
                                callee: Box::new(ident(map_ctor)),
                                args: vec![],
                            },
                            None,
                        ),
                        TsStmt::for_stmt(
                            "i",
                            TsExpr::Lit(TsLit::Num("0".to_string())),
                            less_than(ident("i"), member(ident("json"), "length")),
                            block(vec![raw_stmts_at_depth_one(loop_body)]),
                            None,
                        ),
                        return_(call(ident("Ok"), vec![ident("out")])),
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

#[cfg(test)]
mod default_lowering_tests {
    use super::*;
    use bynk_syntax::ast::{CommonsItem, Expr, RecordField};
    use std::collections::HashMap;

    /// Parses `src` as a single-file commons and returns its `types` table
    /// plus the field list of the `event`/`type` decl named `subject`, so
    /// each test can feed a real, parsed `(init, type_ref)` pair into
    /// [`lower_field_default_wire`] rather than hand-building AST nodes.
    fn parse_fields(
        src: &str,
        subject: &str,
    ) -> (HashMap<String, Arc<TypeDecl>>, Vec<RecordField>) {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("tokenize");
        let unit = bynk_syntax::parser::parse_unit(&tokens, src).expect("parse");
        let items: Vec<CommonsItem> = match unit {
            bynk_syntax::ast::SourceUnit::Context(ctx) => ctx.items,
            bynk_syntax::ast::SourceUnit::Commons(commons) => commons.items,
            _ => panic!("expected a context or commons unit"),
        };
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        let mut fields = None;
        for item in &items {
            match item {
                CommonsItem::Type(t) => {
                    types.insert(t.name.name.clone(), Arc::new(t.clone()));
                    if t.name.name == subject
                        && let TypeBody::Record(r) = &t.body
                    {
                        fields = Some(r.fields.clone());
                    }
                }
                CommonsItem::Event(e) => {
                    if e.name.name == subject {
                        fields = Some(e.body.fields.clone());
                    }
                    types.insert(e.name.name.clone(), Arc::new(e.as_type_decl()));
                }
                _ => {}
            }
        }
        (
            types,
            fields.unwrap_or_else(|| panic!("no decl named `{subject}`")),
        )
    }

    fn default_of<'a>(fields: &'a [RecordField], name: &str) -> (&'a Expr, &'a TypeRef) {
        let f = fields
            .iter()
            .find(|f| f.name.name == name)
            .unwrap_or_else(|| panic!("no field `{name}`"));
        (
            f.init
                .as_ref()
                .unwrap_or_else(|| panic!("field `{name}` has no default")),
            &f.type_ref,
        )
    }

    #[test]
    fn base_literals_lower_to_their_raw_wire_form() {
        let src = r#"
context test

event E = {
  a: Int = 5,
  b: Int = -5,
  c: String = "hi",
  d: Bool = true,
  e: Float = 1.5,
  f: Duration = 5.minutes,
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let cases = [
            ("a", "5"),
            ("b", "-5"),
            ("c", "\"hi\""),
            ("d", "true"),
            ("e", "1.5"),
            ("f", "300000"),
        ];
        for (name, expected) in cases {
            let (init, ty) = default_of(&fields, name);
            assert_eq!(
                lower_field_default_wire(init, ty, &types),
                Ok(expected.to_string()),
                "field `{name}`"
            );
        }
    }

    #[test]
    fn sum_variant_defaults_lower_to_a_bare_kind_object_not_a_qualified_reference() {
        let src = r#"
context test

type Region = enum { Domestic, International }

event E = {
  bare: Region = Domestic,
  qualified: Region = Region.Domestic,
}
"#;
        let (types, fields) = parse_fields(src, "E");
        for name in ["bare", "qualified"] {
            let (init, ty) = default_of(&fields, name);
            let got = lower_field_default_wire(init, ty, &types).unwrap();
            assert_eq!(got, "{ kind: \"Domestic\" }", "field `{name}`");
            assert!(
                !got.contains('.'),
                "field `{name}` must not contain a qualified reference: {got}"
            );
        }
    }

    #[test]
    fn payload_variant_default_recurses_into_declared_field_types() {
        let src = r#"
context test

type Outcome = | Won(prize: Int) | Lost

event E = {
  o: Outcome = Won(100),
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "o");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("{ kind: \"Won\", prize: 100 }".to_string())
        );
    }

    #[test]
    fn qualified_payload_variant_call_lowers_the_same_as_the_bare_call() {
        // Regression: `Outcome.Won(100)` parses to `ExprKind::MethodCall`
        // with an `Ident` receiver — a match that missed that shape silently
        // fell through to "expected a variant" for this qualified spelling.
        let src = r#"
context test

type Outcome = | Won(prize: Int) | Lost

event E = {
  o: Outcome = Outcome.Won(100),
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "o");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("{ kind: \"Won\", prize: 100 }".to_string())
        );
    }

    #[test]
    fn opaque_unsafe_default_lowers_to_the_raw_literal() {
        // Regression: `OrderId.unsafe("x")` also parses to `MethodCall`, the
        // same shape as the qualified-variant case above.
        let src = r#"
context test

type OrderId = opaque String where MinLength(1)

event E = {
  id: OrderId = OrderId.unsafe("abc"),
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "id");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("\"abc\"".to_string())
        );
    }

    #[test]
    fn option_and_result_defaults_use_the_wire_kind_discriminant() {
        let src = r#"
context test

event E = {
  a: Option[Int] = Some(1),
  b: Option[Int] = None,
  c: Result[Int, String] = Ok(1),
  d: Result[Int, String] = Err("nope"),
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let cases = [
            ("a", "{ kind: \"Some\", value: 1 }"),
            ("b", "{ kind: \"None\" }"),
            ("c", "{ kind: \"Ok\", value: 1 }"),
            ("d", "{ kind: \"Err\", error: \"nope\" }"),
        ];
        for (name, expected) in cases {
            let (init, ty) = default_of(&fields, name);
            assert_eq!(
                lower_field_default_wire(init, ty, &types),
                Ok(expected.to_string()),
                "field `{name}`"
            );
        }
    }

    #[test]
    fn record_literal_default_lowers_to_a_plain_object() {
        let src = r#"
context test

type Region = enum { Domestic, International }
type Meta = { region: Region, note: String }

event E = {
  m: Meta = Meta { region: Region.Domestic, note: "x" },
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "m");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("{ region: { kind: \"Domestic\" }, note: \"x\" }".to_string())
        );
    }

    #[test]
    fn list_literal_default_recurses_per_element() {
        let src = r#"
context test

event E = {
  xs: List[Int] = [1, 2, 3],
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "xs");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("[1, 2, 3]".to_string())
        );
    }

    #[test]
    fn mismatched_shapes_return_err_not_panic() {
        let src = r#"
context test

type Region = enum { Domestic, International }

event E = {
  a: Int = "wrong",
  b: Region = International,
}
"#;
        let (types, fields) = parse_fields(src, "E");
        let (init, ty) = default_of(&fields, "a");
        assert!(lower_field_default_wire(init, ty, &types).is_err());
        // Sanity: a *valid* shape for the same sum still succeeds, proving
        // the harness itself is sound.
        let (init, ty) = default_of(&fields, "b");
        assert_eq!(
            lower_field_default_wire(init, ty, &types),
            Ok("{ kind: \"International\" }".to_string())
        );
    }
}

/// #1887: the lenient `Option` decoder's emitted shape. The behaviour is
/// proved at runtime by the `1887_lenient_option_decode` fixture; these pin
/// the rule order the round-trip invariant depends on (absent or `null`, then
/// the exact tagged forms, then a bare value) and the nesting an
/// `Option[Option[Int]]` relies on.
#[cfg(test)]
mod lenient_option_decode_tests {
    use crate::testkit::emit_project;

    const SRC: &str = "commons d.opt\n\n\
        type N = { n: Option[Option[Int]] }\n\n\
        fn dec(s: String) -> Bool {\n  match Json.decode[N](s) {\n    Ok(_) => true\n    Err(_) => false\n  }\n}\n";

    /// The emitted function `name`, from its signature to its closing brace.
    fn function_body<'a>(text: &'a str, name: &str) -> &'a str {
        let start = text
            .find(&format!("function {name}("))
            .unwrap_or_else(|| panic!("`{name}` is emitted:\n{text}"));
        let len = text[start..].find("\n}\n").expect("the function closes");
        &text[start..start + len]
    }

    /// The byte offset of `needle` in `body`, which must contain it.
    fn at(body: &str, needle: &str) -> usize {
        body.find(needle)
            .unwrap_or_else(|| panic!("`{needle}` is in:\n{body}"))
    }

    fn emitted() -> String {
        let out = emit_project(&[("d/opt.bynk", SRC)], crate::project::BuildTarget::Bundle);
        out.text("d/opt.ts").to_string()
    }

    #[test]
    fn the_tagged_forms_are_tried_before_a_bare_value() {
        let text = emitted();
        let outer = function_body(&text, "__deserialise_Option_Option_Int");
        let absent = at(outer, "if (json === undefined || json === null) {");
        let none = at(
            outer,
            "if (obj[\"kind\"] === \"None\" && __keys.length === 1) {",
        );
        let some = at(
            outer,
            "if (obj[\"kind\"] === \"Some\" && __keys.length === 2 && \"value\" in obj) {",
        );
        let tagged = at(
            outer,
            "__option_value_Option_Int(obj[\"value\"], `${path}.value`)",
        );
        let bare = at(outer, "const __b = __option_value_Option_Int(json, path);");
        assert!(
            absent < none && none < some && some < tagged && tagged < bare,
            "{outer}"
        );
    }

    #[test]
    fn the_value_decoder_delegates_to_the_inner_option_codec() {
        let text = emitted();
        // Module-local: only this module's `Option` decoder calls it.
        assert!(!text.contains("export function __option_value_"), "{text}");
        let value = function_body(&text, "__option_value_Option_Int");
        at(value, "__deserialise_Option_Int(raw, at)");
        // The inner `Option[Int]` decoder is lenient in turn, so a bare `1`
        // nests to `Some(Some(1))`.
        let inner = function_body(&text, "__deserialise_Option_Int");
        at(inner, "const __b = __option_value_Int(json, path);");
    }

    #[test]
    fn a_failed_bare_value_names_every_option_form_only_at_its_own_path() {
        let text = emitted();
        let inner = function_body(&text, "__deserialise_Option_Int");
        at(
            inner,
            "if (__b.error.kind === \"StructuralMismatch\" && __b.error.path === path && !__b.error.expected.endsWith(",
        );
        at(inner, &super::OPTION_FORMS_SUFFIX.replace('"', "\\\""));
    }
}
