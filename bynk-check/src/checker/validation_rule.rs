//! #3952: `e.rule` on a `ValidationError`, the `ValidationRule` of
//! `bynk.validation` that names the predicate which failed.
//!
//! `ValidationError` is compiler-known and has no declaration, so its `rule`
//! field cannot be typed from one. The sum it answers is a prelude type
//! ([`bynk_std::PRELUDE_TYPES`]): the checker finds the unit's declaration of it
//! the way `compare` finds `Ordering` ([`super::ordering::ordering_ty`]), and only
//! the declaration from that unit counts, so a type of the user's own called
//! `ValidationRule` is not it.

use super::{Ctx, Ty, TyId, named_ty};
use bynk_syntax::ast::BaseType;
use bynk_syntax::ast::Ident;
use bynk_syntax::error::CompileError;

/// The `ValidationRule` of `bynk.validation`, when it is in scope.
pub(crate) fn validation_rule_ty(ctx: &Ctx) -> Option<TyId> {
    let decl = ctx
        .input
        .types
        .get("ValidationRule")
        .filter(|d| crate::resolver::is_prelude_type(d))?;
    Some(named_ty(decl, ctx.tys))
}

/// The type of `e.rule`, or an error when `bynk.validation` is not reachable (a type of the unit's
/// own named `ValidationRule` hides it).
pub(crate) fn rule_field_ty(field: &Ident, ctx: &mut Ctx) -> Option<TyId> {
    if let Some(ty) = validation_rule_ty(ctx) {
        return Some(ty);
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.types.unknown_field",
            field.span,
            "`e.rule` is a `ValidationRule` from `bynk.validation`, which is not in scope here",
        )
        .with_note(
            "a type of your own named `ValidationRule` hides it, and `bynk.validation` is then not the `ValidationRule` in scope",
        ),
    );
    None
}

/// #2866, #3952: a field of the compiler-known `ValidationError`: `field` and
/// `message` are `String`s and `rule` is the `ValidationRule` that failed. Its
/// runtime `value` stays unreachable from Bynk, and is never written to an HTTP
/// response.
pub(crate) fn validation_error_field(field: &Ident, ctx: &mut Ctx) -> Option<TyId> {
    match field.name.as_str() {
        "field" | "message" => Some(ctx.tys.intern(Ty::Base(BaseType::String))),
        "rule" => rule_field_ty(field, ctx),
        other => {
            ctx.errors.push(CompileError::new(
                "bynk.types.unknown_field",
                field.span,
                format!(
                    "`ValidationError` has no field `{other}` — its fields are `field`, `message`, `rule`"
                ),
            ));
            None
        }
    }
}
