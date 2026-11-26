//! A checker `Ty` rendered back to a `TypeRef` (moved from `bynk-emit`'s boundary
//! module, #3879, so the project-level queue check can name a send's type).

use bynk_syntax::ast::{Ident, TypeRef};

use crate::checker::{Ty, TyId, Types};

/// v0.22b: a checker `Ty` rendered back to a `TypeRef` for the codec
/// machinery (which is `TypeRef`-driven). `None` for types the codec
/// rejects anyway (functions, effects, type variables).
pub fn ty_to_type_ref(t: TyId, tys: &Types) -> Option<TypeRef> {
    let sp = bynk_syntax::span::Span::new(0, 0);
    Some(match &*tys.get(t) {
        Ty::Base(b) => TypeRef::Base(*b, sp),
        // v0.174 (#592): a generic-record instantiation (`Paginated[User]`,
        // `args` non-empty) round-trips as a `TypeRef::App` so the codec closure
        // reaches its monomorphised helper; a non-generic named type stays a
        // bare `Named`.
        Ty::Named { name, args, .. } if !args.is_empty() => TypeRef::App {
            name: Ident {
                name: name.clone(),
                span: sp,
            },
            args: args
                .iter()
                .map(|a| ty_to_type_ref(*a, tys))
                .collect::<Option<Vec<_>>>()?,
            span: sp,
        },
        Ty::Named { name, .. } => TypeRef::Named(Ident {
            name: name.clone(),
            span: sp,
        }),
        Ty::Result(a, b) => TypeRef::Result(
            Box::new(ty_to_type_ref(*a, tys)?),
            Box::new(ty_to_type_ref(*b, tys)?),
            sp,
        ),
        Ty::Option(a) => TypeRef::Option(Box::new(ty_to_type_ref(*a, tys)?), sp),
        Ty::List(a) => TypeRef::List(Box::new(ty_to_type_ref(*a, tys)?), sp),
        Ty::Set(a) => TypeRef::Set(Box::new(ty_to_type_ref(*a, tys)?), sp),
        Ty::Map(k, v) => TypeRef::Map(
            Box::new(ty_to_type_ref(*k, tys)?),
            Box::new(ty_to_type_ref(*v, tys)?),
            sp,
        ),
        Ty::Unit => TypeRef::Unit(sp),
        Ty::ValidationError => TypeRef::ValidationError(sp),
        Ty::DecodeError => TypeRef::DecodeError(sp),
        // R4.3: `Ty::Error` has no codec — same as the other non-boundary
        // types below, but for the additional reason that a checked program
        // should never contain one at a codec-generation site.
        Ty::Error
        | Ty::Never
        | Ty::Effect(_)
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::TestConnection(_)
        | Ty::Connection(_)
        | Ty::HttpResult(_, _)
        | Ty::HttpOutcome(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::RequestQuery
        | Ty::RequestCookies
        | Ty::TokenClaims
        | Ty::Fn { .. }
        | Ty::Var(_)
        | Ty::Actor(_)
        | Ty::ActorSum(_) => {
            return None;
        }
    })
}
