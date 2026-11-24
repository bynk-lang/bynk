//! `Val[T]` mocks and refined defaults (back-half paydown 1A.2, 2.2b, #4087): `Val[T]` and
//! `Val[T](lit)`, the mock value of a type, and the default a refined type's predicates admit.
//! Moved out of `lower.rs` byte for byte.

use bynk_check::checker::{NamedKind, Ty, TyId};
use bynk_syntax::ast::{
    BaseType, Expr, PredBound, PredKind, Refinement, TypeBody, TypeDecl, TypeRef,
};

use super::super::internal_fault::undefined_node;
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::unchecked_construct_test_node;
use super::{Lowered, Pre, decimal_lit, lower_const_literal_node, member_node, one};
use crate::ts_build::{num_lit, str_lit};

/// A default base-type literal (as TypeScript source) that satisfies a refined
/// type's predicates, for bare `Val[T]`. `None` when no default can be derived
/// (a `Matches` refinement — the checker rejects bare `Val` for those).
fn refined_default(decl: &TypeDecl) -> Option<bynk_ts::TsExpr> {
    match &decl.body {
        TypeBody::Refined {
            base, refinement, ..
        } => refinement_default(*base, refinement.as_ref()),
        TypeBody::Record(..) | TypeBody::Sum(..) | TypeBody::Opaque { .. } => None,
    }
}

/// [`refined_default`] over a base type and its refinement, wherever it is
/// written: a refined type's declaration, or a record field's inline `where`
/// (#2658: `qty: Int where Positive` must fabricate `1`, not `0`).
fn refinement_default(base: BaseType, refinement: Option<&Refinement>) -> Option<bynk_ts::TsExpr> {
    // #2686: an `@example` is the value for a refinement `Val` cannot generate
    // (`Matches`, `IsoDate`); the checker proved it satisfies the refinement.
    if let Some(lit) = refinement
        .and_then(bynk_check::checker::refinement_example)
        .and_then(lower_const_literal_node)
    {
        return Some(lit);
    }
    match base {
        BaseType::Int => {
            let mut lo: i64 = 0;
            let mut hi = i64::MAX;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1),
                        PredKind::NonNegative => lo = lo.max(0),
                        PredKind::InRange(a, b) => {
                            lo = lo.max(a.value);
                            hi = hi.min(b.value);
                        }
                        PredKind::AtLeast(PredBound::Int(a)) => lo = lo.max(a.value),
                        PredKind::AtMost(PredBound::Int(b)) => hi = hi.min(b.value),
                        PredKind::Matches(..)
                        | PredKind::InRangeF(..)
                        | PredKind::InRangeD(..)
                        | PredKind::InRangeDur(..)
                        | PredKind::AtLeast(_)
                        | PredKind::AtMost(_)
                        | PredKind::MinLength(..)
                        | PredKind::MaxLength(..)
                        | PredKind::Length(..)
                        | PredKind::NonEmpty
                        | PredKind::IsoDate
                        | PredKind::Cron => {}
                    }
                }
            }
            // The floor of 0 can overshoot a wholly negative range
            // (`InRange(-5, -1)`); fall back to the upper bound. An empty
            // range never reaches here: the checker rejects it.
            if lo > hi {
                lo = hi;
            }
            Some(num_lit(lo.to_string()))
        }
        BaseType::String => {
            let mut len: i64 = 0;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match p.kind {
                        PredKind::NonEmpty => len = len.max(1),
                        PredKind::MinLength(k) | PredKind::Length(k) => len = len.max(k),
                        PredKind::Matches(_) | PredKind::IsoDate | PredKind::Cron => return None,
                        PredKind::InRange(..)
                        | PredKind::InRangeF(..)
                        | PredKind::InRangeD(..)
                        | PredKind::InRangeDur(..)
                        | PredKind::AtLeast(_)
                        | PredKind::AtMost(_)
                        | PredKind::MaxLength(..)
                        | PredKind::NonNegative
                        | PredKind::Positive => {}
                    }
                }
            }
            if len < 1 {
                len = 1;
            }
            Some(str_lit("x".repeat(len as usize)))
        }
        BaseType::Bool => Some(bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true))),
        BaseType::Float => {
            let mut lo: f64 = 0.0;
            let mut hi = f64::INFINITY;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1.0),
                        PredKind::NonNegative => lo = lo.max(0.0),
                        PredKind::InRangeF(a, b) => {
                            lo = lo.max(a.value);
                            hi = hi.min(b.value);
                        }
                        PredKind::AtLeast(PredBound::Float(a)) => lo = lo.max(a.value),
                        PredKind::AtMost(PredBound::Float(b)) => hi = hi.min(b.value),
                        PredKind::Matches(..)
                        | PredKind::InRange(..)
                        | PredKind::InRangeD(..)
                        | PredKind::InRangeDur(..)
                        | PredKind::AtLeast(_)
                        | PredKind::AtMost(_)
                        | PredKind::MinLength(..)
                        | PredKind::MaxLength(..)
                        | PredKind::Length(..)
                        | PredKind::NonEmpty
                        | PredKind::IsoDate
                        | PredKind::Cron => {}
                    }
                }
            }
            // The `Positive` floor of 1.0 can overshoot a tight fractional
            // range (`InRange(0.0, 0.5)`); fall back to the upper bound.
            if lo > hi {
                lo = hi;
            }
            Some(num_lit(lo.to_string()))
        }
        // #3948: the smallest `Duration` the refinement admits, in milliseconds
        // (`0` when it is unrefined).
        BaseType::Duration => {
            let mut lo: i64 = 0;
            let mut hi = i64::MAX;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1),
                        PredKind::InRangeDur(a, b) => {
                            lo = lo.max(a.millis);
                            hi = hi.min(b.millis);
                        }
                        PredKind::AtLeast(PredBound::Duration(a)) => lo = lo.max(a.millis),
                        PredKind::AtMost(PredBound::Duration(b)) => hi = hi.min(b.millis),
                        PredKind::NonNegative
                        | PredKind::InRangeD(..)
                        | PredKind::Matches(..)
                        | PredKind::InRange(..)
                        | PredKind::InRangeF(..)
                        | PredKind::AtLeast(_)
                        | PredKind::AtMost(_)
                        | PredKind::MinLength(..)
                        | PredKind::MaxLength(..)
                        | PredKind::Length(..)
                        | PredKind::NonEmpty
                        | PredKind::IsoDate
                        | PredKind::Cron => {}
                    }
                }
            }
            if lo > hi {
                lo = hi;
            }
            Some(num_lit(lo.to_string()))
        }
        // v0.86: `Instant` carries no refinement; `0` millis is its default.
        BaseType::Instant => Some(num_lit("0".to_string())),
        // v0.110: `Bytes` carries no refinement; the empty octet sequence is
        // its default.
        BaseType::Bytes => Some(empty_bytes_node()),
        // #2622: the smallest value the refinement admits, at scale 2.
        BaseType::Decimal => {
            let mut lo: i128 = 0;
            let mut hi: Option<i128> = None;
            let mut scale = 2u32;
            if let Some(r) = refinement {
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::InRangeD(a, b) => scale = scale.max(a.scale).max(b.scale),
                        PredKind::AtLeast(PredBound::Decimal(a))
                        | PredKind::AtMost(PredBound::Decimal(a)) => scale = scale.max(a.scale),
                        PredKind::InRangeDur(..)
                        | PredKind::NonNegative
                        | PredKind::Positive
                        | PredKind::Matches(..)
                        | PredKind::InRange(..)
                        | PredKind::InRangeF(..)
                        | PredKind::AtLeast(_)
                        | PredKind::AtMost(_)
                        | PredKind::MinLength(..)
                        | PredKind::MaxLength(..)
                        | PredKind::Length(..)
                        | PredKind::NonEmpty
                        | PredKind::IsoDate
                        | PredKind::Cron => {}
                    }
                }
                for p in &r.predicates {
                    match &p.kind {
                        PredKind::Positive => lo = lo.max(1),
                        PredKind::InRangeD(a, b) => {
                            match scaled_coef(&a.coef, a.scale, scale) {
                                Some(v) => lo = lo.max(v),
                                // The bound does not fit once lifted to the
                                // common scale: the bound itself, at its own
                                // scale, is the smallest value in range.
                                None => {
                                    return Some(decimal_lit(&a.coef, a.scale));
                                }
                            }
                            if let Some(v) = scaled_coef(&b.coef, b.scale, scale) {
                                hi = Some(hi.map_or(v, |h| h.min(v)));
                            }
                        }
                        PredKind::AtLeast(PredBound::Decimal(a)) => {
                            match scaled_coef(&a.coef, a.scale, scale) {
                                Some(v) => lo = lo.max(v),
                                None => {
                                    return Some(decimal_lit(&a.coef, a.scale));
                                }
                            }
                        }
                        PredKind::AtMost(PredBound::Decimal(b)) => {
                            if let Some(v) = scaled_coef(&b.coef, b.scale, scale) {
                                hi = Some(hi.map_or(v, |h| h.min(v)));
                            }
                        }
                        PredKind::Matches(..)
                        | PredKind::InRange(..)
                        | PredKind::InRangeF(..)
                        | PredKind::InRangeDur(..)
                        | PredKind::AtLeast(_)
                        | PredKind::AtMost(_)
                        | PredKind::MinLength(..)
                        | PredKind::MaxLength(..)
                        | PredKind::Length(..)
                        | PredKind::NonNegative
                        | PredKind::NonEmpty
                        | PredKind::IsoDate
                        | PredKind::Cron => {}
                    }
                }
            }
            // As for `Float`: a floor above the ceiling falls back to the ceiling.
            if let Some(h) = hi
                && lo > h
            {
                lo = h;
            }
            Some(decimal_lit(&lo.to_string(), scale))
        }
    }
}

/// `coef` (a signed digit string at `from` scale) raised to scale `to`, as an
/// `i128`, or `None` when it does not fit.
fn scaled_coef(coef: &str, from: u32, to: u32) -> Option<i128> {
    let v: i128 = coef.parse().ok()?;
    v.checked_mul(10i128.checked_pow(to.checked_sub(from)?)?)
}

/// The v0.9.4 slice 2 recursion cap for bare `Val` generation (mirrors the
/// checker's `MOCK_DEPTH`).
const MOCK_DEPTH: u32 = 12;

/// `new globalThis.Uint8Array()`: the empty octet sequence, `Bytes`'s default.
fn empty_bytes_node() -> bynk_ts::TsExpr {
    bynk_ts::TsExpr::New {
        callee: Box::new(member_node("globalThis", "Uint8Array")),
        args: Vec::new(),
    }
}

/// A base-literal default for an opaque type's underlying base, as a node. Not
/// distinct per call in this increment — per-call distinctness via a runtime
/// counter is a follow-up.
fn base_default_ts(base: BaseType) -> bynk_ts::TsExpr {
    match base {
        BaseType::Int => num_lit("0".to_string()),
        BaseType::String => str_lit("mock"),
        BaseType::Bool => bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
        BaseType::Float => num_lit("0".to_string()),
        BaseType::Duration | BaseType::Instant => num_lit("0".to_string()),
        BaseType::Bytes => empty_bytes_node(),
        BaseType::Decimal => decimal_lit("0", 0),
    }
}

/// Generate a TypeScript expression for a bare `Val` of `ty` (v0.9.4 Part B,
/// slice 2), as a node (1A.17, Decision D). Recurses through sum payloads and
/// record fields; refined types use `refined_default`, opaque types wrap a base
/// default, bare bases use 0/""/true.
///
/// A shape with no mock value renders as the `undefined` identifier here and
/// nowhere else (1A.34): [`mock_value_of`] says `None` for it, and this is the
/// one place that renders it. `at` is the `Val[T]` call, where a checker fact
/// the recursion needs and does not find is reported.
fn mock_value(ty: TyId, cx: &LowerCtx, depth: u32, at: bynk_syntax::span::Span) -> bynk_ts::TsExpr {
    mock_value_of(ty, cx, depth, at).unwrap_or_else(undefined_node)
}

/// [`mock_value`], with the legitimate absence of a value as `None`: the
/// recursion bound, a type no mock builds (`Option`, `List`, `Unit`, ...), a
/// compiler-known type with no declaration, and a sum with no variant.
fn mock_value_of(
    ty: TyId,
    cx: &LowerCtx,
    depth: u32,
    at: bynk_syntax::span::Span,
) -> Option<bynk_ts::TsExpr> {
    let tys = cx.ty_render();
    if depth == 0 {
        return None;
    }
    // A declared field or payload type the checker resolved when it checked the
    // declaration: it not resolving here is a missing fact.
    let field_value = |type_ref: &TypeRef| match bynk_check::checker::resolve_type_ref(
        type_ref,
        &cx.commons().types,
        tys.types(),
    ) {
        Some(t) => mock_value(t, cx, depth - 1, at),
        None => cx.missing_fact(at, "resolved type for a mocked field"),
    };
    Some(match &*tys.get(ty) {
        Ty::Base(BaseType::Int) => num_lit("0".to_string()),
        Ty::Base(BaseType::String) => str_lit(""),
        Ty::Base(BaseType::Bool) => bynk_ts::TsExpr::Lit(bynk_ts::TsLit::Bool(true)),
        Ty::Base(BaseType::Float) => num_lit("0".to_string()),
        Ty::Base(BaseType::Decimal) => decimal_lit("0", 0),
        Ty::Named { name, .. } => {
            let decl = cx.commons().types.get(name.as_str())?;
            match &decl.body {
                TypeBody::Refined { .. } => {
                    let d = refined_default(decl).unwrap_or_else(|| num_lit("0".to_string()));
                    unchecked_construct_test_node(name, d, false)
                }
                TypeBody::Opaque { base, .. } => {
                    unchecked_construct_test_node(name, base_default_ts(*base), true)
                }
                TypeBody::Sum(s) => match s.variants.first()? {
                    v if v.payload.is_empty() => member_node(name, &v.name.name),
                    v => {
                        let args = v.payload.iter().map(|f| field_value(&f.type_ref)).collect();
                        bynk_ts::TsExpr::Call {
                            callee: Box::new(member_node(name, &v.name.name)),
                            args,
                        }
                    }
                },
                TypeBody::Record(r) => bynk_ts::TsExpr::object(
                    r.fields
                        .iter()
                        .map(|f| {
                            // An inline `where` on a base-typed field narrows
                            // the base's default, as a declared refined type's
                            // does; the field stays a plain base value.
                            let inline = match (&f.type_ref, &f.refinement) {
                                (TypeRef::Base(base, _), Some(r)) => {
                                    refinement_default(*base, Some(r))
                                }
                                _ => None,
                            };
                            let fv = inline.unwrap_or_else(|| field_value(&f.type_ref));
                            (f.name.name.clone(), fv)
                        })
                        .collect(),
                ),
            }
        }
        Ty::Base(BaseType::Bytes | BaseType::Duration | BaseType::Instant)
        | Ty::Error
        | Ty::Never
        | Ty::Result(..)
        | Ty::Option(..)
        | Ty::Effect(..)
        | Ty::HttpResult(..)
        | Ty::QueueResult
        | Ty::HttpOutcome(..)
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::List(..)
        | Ty::Map(..)
        | Ty::Set(..)
        | Ty::Query(..)
        | Ty::Stream(..)
        | Ty::Connection(..)
        | Ty::TestConnection(..)
        | Ty::ValidationError
        | Ty::DecodeError
        | Ty::Unit
        | Ty::Actor(..)
        | Ty::ActorSum(..)
        | Ty::Fn { .. }
        | Ty::Var(..) => return None,
    })
}

/// v0.9.4 Part B (slice 1): lower a refined-type `Val[T]` / `Val[T](lit)` to
/// the branded `unsafe` constructor. The checker has already validated this is a
/// refined type in a test body, and recorded the refined type at `span`.
pub(super) fn lower_val(type_ref: &TypeRef, args: &[Expr], cx: &mut LowerCtx) -> Lowered {
    let tys = cx.ty_render();
    let mut pre = Pre::new();
    // Resolve the mocked type straight from the AST node rather than the
    // checker's `expr_types` side-table — the static type table is always
    // populated, whereas a test body's per-expression types may not be visible
    // to the emitter.
    let ty = match bynk_check::checker::resolve_type_ref(type_ref, &cx.commons().types, tys.types())
    {
        Some(t) => t,
        // A `Val[T]` whose `T` the checker resolved when it checked the call:
        // it not resolving here is a missing fact (1A.34).
        None => {
            return pre.finish(cx.missing_fact(type_ref.span(), "resolved type for a `Val` call"));
        }
    };
    // Refined literal pin. This `Val[T](lit)` path is test-only scaffolding, where
    // the branded type is an `any` value binding, not a type (ADR 0182) — so brand
    // via `unchecked_construct_test_node` (refined → `(lit as any)`), not a `(lit as T)`
    // that would fail to resolve `T`. Opaque never reaches here (`NamedKind::Refined`).
    let ty_node = tys.get(ty);
    if let (
        Some(arg),
        Ty::Named {
            name,
            kind: NamedKind::Refined(_),
            ..
        },
    ) = (args.first(), &*ty_node)
    {
        let value = match lower_const_literal_node(arg) {
            Some(n) => n,
            None => pre.lower_node(arg, cx),
        };
        let built = {
            let c = vec![value];
            unchecked_construct_test_node(name, one(c), false)
        };
        return pre.finish(built);
    }
    // Bare mock (refined / opaque / sum / record).
    pre.finish(mock_value(ty, cx, MOCK_DEPTH, type_ref.span()))
}
