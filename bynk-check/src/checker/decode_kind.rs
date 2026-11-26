//! #3976: `e.kind` on a `DecodeError`, the `DecodeErrorKind` of `bynk.decode` that
//! names why a decode failed.
//!
//! `DecodeError` is compiler-known and has no declaration, so its `kind` field cannot
//! be typed from one. The sum it answers is a prelude type ([`bynk_std::PRELUDE_TYPES`]),
//! found the way `e.rule`'s [`super::validation_rule::validation_rule_ty`] finds
//! `ValidationRule`: only the declaration from that unit counts, so a type of the user's
//! own called `DecodeErrorKind` is not it.

use super::{Ctx, Ty, TyId, named_ty};
use bynk_syntax::ast::{BaseType, Expr, ExprKind, Ident};
use bynk_syntax::error::{Applicability, CompileError};

/// The `DecodeErrorKind` of `bynk.decode`, when it is in scope.
pub(crate) fn decode_error_kind_ty(ctx: &Ctx) -> Option<TyId> {
    let decl = ctx
        .input
        .types
        .get("DecodeErrorKind")
        .filter(|d| crate::resolver::is_prelude_type(d))?;
    Some(named_ty(decl, ctx.tys))
}

/// The type of `e.kind`, or an error when `bynk.decode` is not reachable (a type of the unit's
/// own named `DecodeErrorKind` hides it).
fn kind_field_ty(field: &Ident, ctx: &mut Ctx) -> Option<TyId> {
    if let Some(ty) = decode_error_kind_ty(ctx) {
        return Some(ty);
    }
    ctx.errors.push(
        CompileError::new(
            "bynk.types.unknown_field",
            field.span,
            "`e.kind` is a `DecodeErrorKind` from `bynk.decode`, which is not in scope here",
        )
        .with_note(
            "a type of your own named `DecodeErrorKind` hides it, and `bynk.decode` is then not the `DecodeErrorKind` in scope",
        ),
    );
    None
}

/// A field of the compiler-known `DecodeError` (ADR 0047, #3976): `path` and `message` are
/// `String`s and `kind` is the `DecodeErrorKind` that failed.
pub(crate) fn decode_error_field(field: &Ident, ctx: &mut Ctx) -> Option<TyId> {
    match field.name.as_str() {
        "path" | "message" => Some(ctx.tys.intern(Ty::Base(BaseType::String))),
        "kind" => kind_field_ty(field, ctx),
        other => {
            ctx.errors.push(CompileError::new(
                "bynk.types.unknown_field",
                field.span,
                format!(
                    "`DecodeError` has no field `{other}` — its fields are `kind`, `path`, `message`"
                ),
            ));
            None
        }
    }
}

/// Every `DecodeErrorKind` variant, in declaration order.
const KINDS: [&str; 12] = [
    "Malformed",
    "StructuralMismatch",
    "RefinementViolation",
    "UnsupportedAnchor",
    "UnsupportedAlias",
    "UnsupportedMergeKey",
    "UnsupportedTag",
    "MultipleDocuments",
    "UnsupportedSyntax",
    "UnsupportedValue",
    "DuplicateKey",
    "LimitExceeded",
];

/// #3976 (Decision B): `e.kind == "Malformed"` compared a `String` before `kind` was a
/// `DecodeErrorKind`. When one operand is a `DecodeErrorKind` and the other a string
/// literal, say so, and when the literal names a kind offer the machine-applicable fix to
/// the variant (`e.kind == Malformed`). A literal that names no kind gets no fix: the
/// error says so, which is what catches the typo.
pub(crate) fn migrate_string_comparison(
    err: CompileError,
    lhs: (&Expr, TyId),
    rhs: (&Expr, TyId),
    ctx: &Ctx,
) -> CompileError {
    let Some(kind) = decode_error_kind_ty(ctx) else {
        return err;
    };
    let literal = match (lhs, rhs) {
        ((_, t), (lit, _)) if t == kind => lit,
        ((lit, _), (_, t)) if t == kind => lit,
        _ => return err,
    };
    let ExprKind::StrLit(text) = &literal.kind else {
        return err;
    };
    if KINDS.contains(&text.as_str()) {
        err.with_note(
            "`e.kind` is a `DecodeErrorKind` (a closed sum), not a `String`; compare it with a variant",
        )
        .with_suggestion(
            format!("compare with the variant `{text}`"),
            vec![(literal.span, text.clone())],
            Applicability::MachineApplicable,
        )
    } else {
        err.with_note(format!(
            "`e.kind` is a `DecodeErrorKind`, not a `String`, and no kind is named \"{text}\"; the kinds are {}",
            KINDS.join(", ")
        ))
    }
}
