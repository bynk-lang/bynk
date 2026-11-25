//! #4120: generators for `List`, `Set`, `Map` and `Option`, and the types containing them.
//!
//! A collection is drawn from its element generators (`rng.list` draws up to eight elements,
//! sharing a size budget with any nested collection); an `Option` is `None` or `Some` of an
//! element. The boundary values are the empty collection, `None`, and a one-element collection of
//! the element's canonical value. A collection shrinks by dropping elements and then by shrinking
//! the remaining ones; an `Option` shrinks to `None` and then shrinks its payload.

use super::*;
use crate::emitter::internal_fault::FaultSite;
use bynk_check::checker::Types;
use bynk_syntax::ast::{BaseType, TypeBody, TypeDecl};
use bynk_ts::{TsArrowBody, TsExpr, TsParam};
use std::sync::Arc;

/// How many levels of a type an element of a collection is drawn through.
const COLLECTION_ELEM_DEPTH: u32 = 3;

/// `(<params>) => <body>`.
fn arrow(params: &[&str], body: TsExpr) -> TsExpr {
    TsExpr::Arrow {
        params: params
            .iter()
            .map(|p| TsParam::new((*p).to_string(), None))
            .collect(),
        is_async: false,
        generics: Vec::new(),
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(body))),
    }
}

/// `{ key: <k>, value: <v> }`, the entry shape `__bynkMapOf` reads.
fn entry(k: TsExpr, v: TsExpr) -> TsExpr {
    TsExpr::Paren(Box::new(TsExpr::object(vec![
        ("key".to_string(), k),
        ("value".to_string(), v),
    ])))
}

/// The element of a resolved list, set or option, or the key and value of a map.
enum Shape {
    List(checker::TyId),
    Set(checker::TyId),
    Option(checker::TyId),
    Map(checker::TyId, checker::TyId),
}

fn shape(ty: checker::TyId, tys: &Arc<Types>) -> Option<Shape> {
    match &*tys.get(ty) {
        checker::Ty::List(t) => Some(Shape::List(*t)),
        checker::Ty::Set(t) => Some(Shape::Set(*t)),
        checker::Ty::Option(t) => Some(Shape::Option(*t)),
        checker::Ty::Map(k, v) => Some(Shape::Map(*k, *v)),
        checker::Ty::Base(_)
        | checker::Ty::Named { .. }
        | checker::Ty::Error
        | checker::Ty::Never
        | checker::Ty::Result(..)
        | checker::Ty::Effect(_)
        | checker::Ty::HttpResult(_)
        | checker::Ty::QueueResult
        | checker::Ty::HttpOutcome(_)
        | checker::Ty::BoundaryRejection
        | checker::Ty::Asset
        | checker::Ty::RequestHeaders
        | checker::Ty::RequestQuery
        | checker::Ty::RequestCookies
        | checker::Ty::TokenClaims
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

/// The draw of a `List`, `Set`, `Map` or `Option`, or `None` for any other type.
pub(super) fn collection_gen_ts(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    depth: u32,
    tys: &Arc<Types>,
    site: FaultSite<'_>,
) -> Option<TsExpr> {
    let sh = shape(ty, tys)?;
    // A collection's elements are drawn a few levels deep at most: the empty collection ends a
    // type that recurses through one, and a generator inlined at every level of such a type
    // would otherwise grow with the branching to the power of the depth.
    let elem_depth = depth.saturating_sub(1).min(COLLECTION_ELEM_DEPTH);
    let elem = |t: checker::TyId| {
        coerce_int_field(
            t,
            types,
            tys,
            gen_ts_for_ty(t, types, elem_depth, tys, site),
        )
    };
    let list = |body: TsExpr| method_call(ident("rng"), "list", vec![arrow(&[], body)]);
    Some(match sh {
        Shape::List(t) => list(elem(t)),
        Shape::Set(t) => call(ident("__bynkSetOf"), vec![list(elem(t))]),
        Shape::Option(t) => method_call(
            ident("rng"),
            "pick",
            vec![TsExpr::array(vec![
                arrow(&[], ident("None")),
                arrow(&[], call(ident("Some"), vec![elem(t)])),
            ])],
        ),
        Shape::Map(k, v) => call(ident("__bynkMapOf"), vec![list(entry(elem(k), elem(v)))]),
    })
}

/// The canonical value of a collection or option: empty, or `None`.
pub(super) fn collection_canon_ts(ty: checker::TyId, tys: &Arc<Types>) -> Option<TsExpr> {
    Some(match shape(ty, tys)? {
        Shape::List(_) => TsExpr::array(vec![]),
        Shape::Set(_) => new(member(ident("globalThis"), "Set"), Vec::new()),
        Shape::Map(..) => new(member(ident("globalThis"), "Map"), Vec::new()),
        Shape::Option(_) => ident("None"),
    })
}

/// The boundary values and the shrinker of a collection or option binding.
pub(super) fn collection_boundaries_shrink(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
    site: FaultSite<'_>,
) -> Option<(Vec<TsExpr>, TsExpr)> {
    let sh = shape(ty, tys)?;
    let canon = |t: checker::TyId| {
        coerce_int_field(
            t,
            types,
            tys,
            canon_ts_for_ty(t, types, test_suites::PROP_GEN_DEPTH, tys, site),
        )
    };
    let empty = collection_canon_ts(ty, tys)?;
    let one = match &sh {
        Shape::List(t) => TsExpr::array(vec![canon(*t)]),
        Shape::Set(t) => call(ident("__bynkSetOf"), vec![TsExpr::array(vec![canon(*t)])]),
        Shape::Option(t) => call(ident("Some"), vec![canon(*t)]),
        Shape::Map(k, v) => call(
            ident("__bynkMapOf"),
            vec![TsExpr::array(vec![entry(canon(*k), canon(*v))])],
        ),
    };
    let shrink = match &sh {
        Shape::List(t) => call(
            ident("__bynkShrinkList"),
            vec![ident("v"), elem_shrink_ts(*t, types, tys)],
        ),
        Shape::Option(t) => call(
            ident("__bynkShrinkOption"),
            vec![ident("v"), elem_shrink_ts(*t, types, tys)],
        ),
        Shape::Set(t) => method_call(
            call(
                ident("__bynkShrinkList"),
                vec![
                    TsExpr::array(vec![TsExpr::Spread(Box::new(ident("v")))]),
                    elem_shrink_ts(*t, types, tys),
                ],
            ),
            "map",
            vec![arrow(
                &["__l"],
                call(ident("__bynkSetOf"), vec![ident("__l")]),
            )],
        ),
        Shape::Map(_, v) => method_call(
            call(
                ident("__bynkShrinkList"),
                vec![
                    method_call(
                        TsExpr::array(vec![TsExpr::Spread(Box::new(ident("v")))]),
                        "map",
                        vec![arrow(
                            &["__e"],
                            entry(member(ident("__e"), "0"), member(ident("__e"), "1")),
                        )],
                    ),
                    arrow(
                        &["__e"],
                        method_call(
                            call(
                                elem_shrink_ts(*v, types, tys),
                                vec![member(ident("__e"), "value")],
                            ),
                            "map",
                            vec![arrow(
                                &["__x"],
                                entry(member(ident("__e"), "key"), ident("__x")),
                            )],
                        ),
                    ),
                ],
            ),
            "map",
            vec![arrow(
                &["__l"],
                call(ident("__bynkMapOf"), vec![ident("__l")]),
            )],
        ),
    };
    Some((vec![empty, one], shrink))
}

/// A shrinker for one element: `(x) => <candidates>`. Plain scalars and nested collections shrink;
/// a record or a sum keeps its value (its collection shrinks by dropping it instead).
fn elem_shrink_ts(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> TsExpr {
    let none = || arrow(&["__x"], TsExpr::array(vec![]));
    let via = |f: &str, extra: Vec<TsExpr>| {
        let mut args = vec![ident("__x")];
        args.extend(extra);
        arrow(&["__x"], call(ident(f), args))
    };
    // A nested `Int` is a `number`; its shrinker works on `bigint`.
    let int_via = |floor: i64| {
        arrow(
            &["__x"],
            method_call(
                call(
                    ident("__bynkShrinkInt"),
                    vec![
                        call(ident("globalThis.BigInt"), vec![ident("__x")]),
                        TsExpr::bigint(floor),
                    ],
                ),
                "map",
                vec![ident("globalThis.Number")],
            ),
        )
    };
    match &*tys.get(ty) {
        checker::Ty::Base(BaseType::Int) => int_via(0),
        checker::Ty::Base(BaseType::String) => via("__bynkShrinkString", vec![num_lit("0")]),
        checker::Ty::Base(BaseType::Float) => via("__bynkShrinkNum", vec![num_lit("0")]),
        checker::Ty::Base(BaseType::Duration | BaseType::Instant) => {
            via("__bynkShrinkMillis", vec![])
        }
        checker::Ty::Base(BaseType::Decimal) => via("__bynkShrinkDecimal", vec![]),
        checker::Ty::Base(BaseType::Bytes) => via("__bynkShrinkBytes", vec![]),
        checker::Ty::Base(BaseType::Bool) => arrow(
            &["__x"],
            cond_expr(
                ident("__x"),
                TsExpr::array(vec![TsExpr::Lit(bynk_ts::TsLit::Bool(false))]),
                TsExpr::array(vec![]),
            ),
        ),
        checker::Ty::Named { name, .. } => match types.get(name).map(|d| &d.body) {
            Some(TypeBody::Refined {
                base, refinement, ..
            })
            | Some(TypeBody::Opaque {
                base, refinement, ..
            }) => match base {
                BaseType::Int => int_via(int_bounds(refinement.as_ref()).2),
                BaseType::String => match string_gen_spec(refinement.as_ref()) {
                    Some(spec) => arrow(
                        &["__x"],
                        call(
                            ident("__bynkShrinkMatch"),
                            vec![ident("__x"), str_lit(spec)],
                        ),
                    ),
                    None => via(
                        "__bynkShrinkString",
                        vec![num_lit(str_min(refinement.as_ref()).to_string())],
                    ),
                },
                BaseType::Float
                | BaseType::Bool
                | BaseType::Duration
                | BaseType::Instant
                | BaseType::Bytes
                | BaseType::Decimal => none(),
            },
            Some(TypeBody::Record(_) | TypeBody::Sum(_)) | None => none(),
        },
        checker::Ty::List(t) => arrow(
            &["__x"],
            call(
                ident("__bynkShrinkList"),
                vec![ident("__x"), elem_shrink_ts(*t, types, tys)],
            ),
        ),
        checker::Ty::Option(t) => arrow(
            &["__x"],
            call(
                ident("__bynkShrinkOption"),
                vec![ident("__x"), elem_shrink_ts(*t, types, tys)],
            ),
        ),
        // A nested set or map keeps its value: the outer collection shrinks around it.
        checker::Ty::Set(_) | checker::Ty::Map(..) => none(),
        checker::Ty::Error
        | checker::Ty::Never
        | checker::Ty::Result(..)
        | checker::Ty::Effect(_)
        | checker::Ty::HttpResult(_)
        | checker::Ty::QueueResult
        | checker::Ty::HttpOutcome(_)
        | checker::Ty::BoundaryRejection
        | checker::Ty::Asset
        | checker::Ty::RequestHeaders
        | checker::Ty::RequestQuery
        | checker::Ty::RequestCookies
        | checker::Ty::TokenClaims
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
        | checker::Ty::Var(_) => none(),
    }
}
