//! #4560: an enum's wire name as text, and its inverse.
//!
//! An enum (a sum whose variants carry no payload) travels as its variant's wire name: the
//! `@name("…")` of the variant, else the variant's own name. The codecs, the form and
//! path/query/header/cookie readers and OpenAPI all read that one name; this module gives the
//! language a way to read it too, so a caller that needs the text (a SQL parameter, a header, a
//! log field, a key) does not hand-write a `match` that drifts from `@name`.
//!
//! - `e.wireName()` answers the `String`.
//! - `E.fromWireName(s)` answers `Some(variant)` for the variant whose wire name is exactly `s`
//!   (case-sensitive, like the codecs), else `None`.
//!
//! Both read the wire name the way the codecs do (`Variant::wire_name`), and a type that declares
//! its own `wireName` method or `fromWireName` static keeps it: the built-in answers only where
//! the type has declared none, so no program that compiled before changes meaning.

use std::sync::Arc;

use bynk_syntax::ast::{BaseType, Expr, Ident, TypeBody, TypeDecl};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::kernel_op::{EnumOp, KernelOp, OrdOp};

use super::{Callee, Ctx, ExprId, Ty, TyId};

/// The name of the static.
pub const FROM_WIRE_NAME: &str = "fromWireName";

/// Whether `decl` is an enum: a non-generic sum with at least one variant, none carrying a payload.
pub fn is_enum(decl: &TypeDecl) -> bool {
    decl.type_params.is_empty()
        && matches!(&decl.body, TypeBody::Sum(s)
            if !s.variants.is_empty() && s.variants.iter().all(|v| v.payload.is_empty()))
}

/// `e.wireName()`: `None` when the call is not this method (the receiver is not an enum, or the
/// type declares its own `wireName`) and ordinary dispatch carries on; `Some(result)` is the
/// checked call, which the caller records as a [`crate::kernel_op::EnumOp`].
pub(crate) fn check_wire_name_method(
    recv_ty: TyId,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let Ty::Named { name, .. } = &*ctx.tys.get(recv_ty) else {
        return None;
    };
    let decl = ctx.input.types.get(name)?;
    if !is_enum(decl)
        || ctx
            .input
            .methods
            .get(name)
            .is_some_and(|t| t.instance.contains_key("wireName"))
    {
        return None;
    }
    if !args.is_empty() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`wireName` takes no arguments, got {}", args.len()),
        ));
        for a in args {
            let _ = super::type_of(a, None, ctx);
        }
        return Some(None);
    }
    Some(Some(ctx.tys.intern(Ty::Base(BaseType::String))))
}

/// The methods that belong to no one receiver type: `compare` on every `Ord` receiver (#3955) and
/// `wireName` on every enum. `None` when `method` is neither, or the call is not theirs (a type that
/// declares its own), so ordinary dispatch carries on; else the kernel op and the checked call.
pub(crate) fn check_universal_method(
    recv_ty: TyId,
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<(KernelOp, Option<TyId>)> {
    match method.name.as_str() {
        "compare" => super::ordering::check_compare_method(recv_ty, method, args, span, ctx)
            .map(|r| (KernelOp::Ord(OrdOp::Compare), r)),
        "wireName" => check_wire_name_method(recv_ty, args, span, ctx)
            .map(|r| (KernelOp::Enum(EnumOp::WireName), r)),
        _ => None,
    }
}

/// `E.fromWireName(s)`, where `decl` is the enum `E`: `None` when the call is not this static (the
/// type is not an enum), else the checked call, an `Option[E]`, recorded as
/// [`Callee::EnumFromWireName`]. The caller has already let a user-declared static of that name win.
pub(crate) fn check_from_wire_name(
    decl: &Arc<TypeDecl>,
    type_name: &Ident,
    args: &[Expr],
    span: Span,
    expr_id: ExprId,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    if !is_enum(decl) {
        return None;
    }
    ctx.callees
        .insert(expr_id, Callee::EnumFromWireName(Arc::clone(decl)));
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.constructor_arity",
            span,
            format!(
                "`{}.{FROM_WIRE_NAME}` expects 1 argument, but {} were given",
                type_name.name,
                args.len()
            ),
        ));
        for a in args {
            let _ = super::type_of(a, None, ctx);
        }
        return Some(None);
    }
    let string = ctx.tys.intern(Ty::Base(BaseType::String));
    super::calls::check_arg(
        &args[0],
        string,
        &format!("the `{}.{FROM_WIRE_NAME}` argument", type_name.name),
        ctx,
    );
    let me = super::named_ty(decl, ctx.tys);
    Some(Some(ctx.tys.intern(Ty::Option(me))))
}
