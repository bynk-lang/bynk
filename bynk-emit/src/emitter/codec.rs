//! Codec witnesses for `Codable` (constrained-generics slice 3, #2715; the model
//! is DECISIONS B and C of the proposal and the track's Q4/Q7).
//!
//! Bynk generics are erased, so a `Codable`-bounded type parameter gets a hidden
//! trailing codec argument (a *witness*): `{ encode, decode }`, built at the call
//! site from the concrete type's generated `__serialise_X` / `__deserialise_X`
//! pair, so a generic call crosses the boundary exactly as a monomorphic one
//! does. This module builds that expression, in TypeScript, from a type:
//!
//! - a type with no type variable is the generated pair for it;
//! - a rigid type parameter is the enclosing function's own witness, forwarded;
//! - `List`, `Option`, `Set`, `Map` and `Result` over a type variable are the
//!   runtime combinators (`__bynkCodecList`, …) over the witnesses of their
//!   parts.
//!
//! A user generic type over a type variable has no codec here: the checker
//! rejects it (`bynk.generics.codec_generic_at_rigid`).

use std::collections::HashMap;

use bynk_check::checker::{Ty, TyId, TypedCommons};
use bynk_syntax::ast::TypeRef;

use super::runtime_use::RuntimeUse;
use super::serialisation;

/// The hidden witness parameter's name for type parameter `param`.
pub(crate) fn witness_ident(param: &str) -> String {
    format!("__codec_{param}")
}

/// Whether `ty` mentions a type variable.
pub(crate) fn has_var(ty: TyId, commons: &TypedCommons) -> bool {
    let tys = commons.tys();
    match &*tys.get(ty) {
        Ty::Var(_) => true,
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => has_var(*a, commons),
        Ty::Result(a, b) | Ty::Map(a, b) => has_var(*a, commons) || has_var(*b, commons),
        Ty::Named { args, .. } => args.iter().any(|a| has_var(*a, commons)),
        _ => false,
    }
}

/// The variable-free parts of `ty` that a witness for it is built from: `ty`
/// itself when it has no type variable, else the ground parts of its containers.
/// These are the types whose generated codecs the module must emit.
pub(crate) fn ground_parts(ty: TyId, commons: &TypedCommons, out: &mut Vec<TyId>) {
    if !has_var(ty, commons) {
        out.push(ty);
        return;
    }
    let tys = commons.tys();
    match &*tys.get(ty) {
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => ground_parts(*a, commons, out),
        Ty::Result(a, b) | Ty::Map(a, b) => {
            ground_parts(*a, commons, out);
            ground_parts(*b, commons, out);
        }
        _ => {}
    }
}

/// Builds witness expressions. `scope` maps a rigid type parameter to the
/// identifier of its in-scope witness.
pub(crate) struct CodecGen<'a> {
    commons: &'a TypedCommons,
    runtime_use: &'a RuntimeUse,
    scope: &'a HashMap<String, String>,
}

impl<'a> CodecGen<'a> {
    pub(crate) fn new(
        commons: &'a TypedCommons,
        runtime_use: &'a RuntimeUse,
        scope: &'a HashMap<String, String>,
    ) -> Self {
        CodecGen {
            commons,
            runtime_use,
            scope,
        }
    }

    fn wrap(&self, helper: &'static str, parts: Vec<String>) -> String {
        self.runtime_use.note_cmp(helper);
        format!("{helper}({})", parts.join(", "))
    }

    /// The `{ encode, decode }` witness for `ty`.
    pub(crate) fn for_ty(&self, ty: TyId) -> String {
        let tys = self.commons.tys();
        if !has_var(ty, self.commons) {
            return self.ground(ty);
        }
        match &*tys.get(ty) {
            Ty::Var(name) => self
                .scope
                .get(name)
                .cloned()
                .unwrap_or_else(|| witness_ident(name)),
            Ty::Option(a) => self.wrap("__bynkCodecOption", vec![self.for_ty(*a)]),
            Ty::List(a) => self.wrap("__bynkCodecList", vec![self.for_ty(*a)]),
            Ty::Set(a) => self.wrap("__bynkCodecSet", vec![self.for_ty(*a)]),
            Ty::Map(k, v) => self.wrap("__bynkCodecMap", vec![self.for_ty(*k), self.for_ty(*v)]),
            Ty::Result(a, b) => {
                self.wrap("__bynkCodecResult", vec![self.for_ty(*a), self.for_ty(*b)])
            }
            other => {
                unreachable!("bynk internal error: no codec over a type variable in {other:?}")
            }
        }
    }

    /// The generated pair for a variable-free type.
    fn ground(&self, ty: TyId) -> String {
        let tys = self.commons.tys();
        let tref: TypeRef = super::ty_to_type_ref(ty, tys)
            .unwrap_or_else(|| unreachable!("bynk internal error: no TypeRef for a codec target"));
        bynk_ts::print_expr(&serialisation::witness_codec_expr(&tref, self.runtime_use))
    }
}
