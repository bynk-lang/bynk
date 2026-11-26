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
mod factory;
mod field;
mod generics;
mod record_sum;
mod refined;
mod support;

use support::DepthCodecs;

pub(crate) use expr::{
    agent_wire_passes_through, deserialise_expr, deserialise_expr_via, peel_list_option,
    serialise_expr, serialise_ref_via, witness_codec_expr,
};
pub use expr::{deserialise_ref_via, serialise_expr_via};
pub(crate) use factory::{
    codec_factory_ground_roots, codec_factory_ident, codec_factory_name, codec_factory_root,
    emit_codec_factory,
};
pub use generics::emit_generic_helpers_qualified;
pub(crate) use generics::{emit_generic_helpers, is_string_key_ref};

use std::sync::Arc;

use bynk_syntax::ast::{TypeBody, TypeDecl, TypeRef};

use crate::emitter::RuntimeUse;
use bynk_ts::{TsDecl, TsStmt};

use record_sum::{emit_record, emit_sum};
use refined::emit_refined;

// -- Helpers over the shared builders (`crate::ts_build`) ----------------------

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
use bynk_check::wire::{Provenance, WireRef, recursive_codec_suffixes, wire_ref};

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

// #855 (Phase 1): `inner_ts_name` moved to `bynk_check::wire` as
// `codec_suffix`; `collect_codec_closure` moved verbatim (pure, AST-only —
// see that module's doc for the seam). Re-exported under their original
// names so call sites here and elsewhere in `bynk-emit` keep compiling
// unchanged.
pub(crate) use bynk_check::wire::codec_suffix as inner_ts_name;
pub use bynk_check::wire::collect_codec_closure;

// #855 (Phase 1): `collect_generic_instantiations` and `GenericInst` (now
// `WireInst` — see that type's doc for why its variant names are kept as
// `ResultInst`/`OptionInst`/… rather than the plan's bare spelling, deferred
// to Phase 2) moved to `bynk_check::wire`; `walk_generic_inst` moved with
// them (no callers outside the functions that moved). Re-exported under
// their original names so call sites here and elsewhere in `bynk-emit` keep
// compiling unchanged.
pub(crate) use bynk_check::wire::WireInst as GenericInst;
pub(crate) use bynk_check::wire::collect_generic_instantiations;

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
