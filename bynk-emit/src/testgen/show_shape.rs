//! #4229: where a `Duration` or an `Instant` sits inside a value's type, for the test runtime's
//! printer. At run time both are plain numbers, indistinguishable from an `Int`, so a failing
//! `for all d: Duration` and a failed `expect … is …` print a bare millisecond count unless the
//! printer is told the static type. The shape is a small TypeScript literal the printer walks
//! beside the value:
//!
//! - `"D"` and `"I"`: a `Duration` and an `Instant`;
//! - `["l", shape]`: a list; `["o", shape]`: an `Option`; `["res", ok, err]`: a `Result`
//!   (a `null` slot has nothing to tell);
//! - `["r", { field: shape }]`: a record; `["s", { Variant: { field: shape } }]`: a sum.
//!
//! A type that holds neither has no shape, and keeps the type-blind printer.

use std::collections::HashMap;
use std::sync::Arc;

use bynk_check::checker::{self, TyId, Types};
use bynk_syntax::ast::{BaseType, TypeBody, TypeDecl};

use crate::ts_build::str_lit;
use bynk_ts::TsExpr;

/// How deep a shape follows named types, which also bounds a recursive type.
const SHAPE_DEPTH: u32 = 6;

/// The shape of values of `ty`, or `None` when no `Duration` or `Instant` can be inside one.
pub(crate) fn show_shape(
    ty: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> Option<TsExpr> {
    shape_of(ty, types, tys, SHAPE_DEPTH)
}

fn shape_of(
    ty: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
    depth: u32,
) -> Option<TsExpr> {
    if depth == 0 {
        return None;
    }
    let one = |t: TyId| shape_of(t, types, tys, depth - 1);
    let tagged = |tag: &str, inner: Vec<TsExpr>| {
        let mut parts = vec![str_lit(tag)];
        parts.extend(inner);
        TsExpr::array(parts)
    };
    // A slot with no shape of its own is `null`, so a later slot keeps its position.
    let slot = |s: Option<TsExpr>| s.unwrap_or_else(crate::ts_build::null_lit);
    match &*tys.get(ty) {
        checker::Ty::Base(BaseType::Duration) => Some(str_lit("D")),
        checker::Ty::Base(BaseType::Instant) => Some(str_lit("I")),
        checker::Ty::List(t) => one(*t).map(|s| tagged("l", vec![s])),
        checker::Ty::Option(t) => one(*t).map(|s| tagged("o", vec![s])),
        checker::Ty::Result(a, b) => {
            let (ok, err) = (one(*a), one(*b));
            if ok.is_none() && err.is_none() {
                None
            } else {
                Some(tagged("res", vec![slot(ok), slot(err)]))
            }
        }
        checker::Ty::Named { name, .. } => {
            let decl = types.get(name)?;
            let fields = |fs: &[(String, TyId)]| -> Vec<(String, TsExpr)> {
                fs.iter()
                    .filter_map(|(n, t)| one(*t).map(|s| (n.clone(), s)))
                    .collect()
            };
            match &decl.body {
                TypeBody::Record(r) => {
                    let resolved: Vec<(String, TyId)> = r
                        .fields
                        .iter()
                        .filter_map(|f| {
                            checker::resolve_type_ref(&f.type_ref, types, tys)
                                .map(|t| (f.name.name.clone(), t))
                        })
                        .collect();
                    let found = fields(&resolved);
                    (!found.is_empty()).then(|| tagged("r", vec![TsExpr::object(found)]))
                }
                TypeBody::Sum(s) => {
                    let variants: Vec<(String, TsExpr)> = s
                        .variants
                        .iter()
                        .filter_map(|v| {
                            let resolved: Vec<(String, TyId)> = v
                                .payload
                                .iter()
                                .filter_map(|f| {
                                    checker::resolve_type_ref(&f.type_ref, types, tys)
                                        .map(|t| (crate::emitter::payload_prop(&f.name.name), t))
                                })
                                .collect();
                            let found = fields(&resolved);
                            (!found.is_empty())
                                .then(|| (v.name.name.clone(), TsExpr::object(found)))
                        })
                        .collect();
                    (!variants.is_empty()).then(|| tagged("s", vec![TsExpr::object(variants)]))
                }
                TypeBody::Refined { .. } | TypeBody::Opaque { .. } => None,
            }
        }
        checker::Ty::Error
        | checker::Ty::Never
        | checker::Ty::Base(_)
        | checker::Ty::Effect(_)
        | checker::Ty::HttpResult(_, _)
        | checker::Ty::QueueResult
        | checker::Ty::HttpOutcome(_)
        | checker::Ty::BoundaryRejection
        | checker::Ty::Asset
        | checker::Ty::RequestHeaders
        | checker::Ty::RequestQuery
        | checker::Ty::RequestCookies
        | checker::Ty::TokenClaims
        | checker::Ty::Map(..)
        | checker::Ty::Set(_)
        | checker::Ty::Query(_)
        | checker::Ty::Stream(_)
        | checker::Ty::Connection(_)
        | checker::Ty::TestConnection(_)
        | checker::Ty::ValidationError
        | checker::Ty::DecodeError
        | checker::Ty::Unit
        | checker::Ty::Actor(_)
        | checker::Ty::ActorSum(_)
        | checker::Ty::Fn { .. }
        | checker::Ty::Var(_) => None,
    }
}
