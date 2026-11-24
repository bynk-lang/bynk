//! #3978: the `String` methods the kernel lacked (`isEmpty`, `trimStart`, `trimEnd`,
//! `padStart`, `padEnd`, `repeat`, `lines`, `lastIndexOf`, `replaceFirst`) and
//! `join` on a `List[String]`. Kept apart from `kernels.rs` so that file does not grow.

use super::calls::check_arg;
use super::*;
use crate::kernel_op::StringOp;

/// `(parameter types, return type)` of one of the new `String` methods.
pub(super) fn signature(op: StringOp, tys: &Types) -> (Vec<TyId>, Ty) {
    let s = Ty::Base(BaseType::String);
    let int = Ty::Base(BaseType::Int);
    let string = |tys: &Types| tys.intern(s.clone());
    let int_id = |tys: &Types| tys.intern(int.clone());
    match op {
        StringOp::IsEmpty => (vec![], Ty::Base(BaseType::Bool)),
        StringOp::TrimStart | StringOp::TrimEnd => (vec![], s.clone()),
        StringOp::PadStart | StringOp::PadEnd => (vec![int_id(tys), string(tys)], s.clone()),
        StringOp::Repeat => (vec![int_id(tys)], s.clone()),
        StringOp::Lines => (vec![], Ty::List(string(tys))),
        StringOp::LastIndexOf => (vec![string(tys)], Ty::Option(int_id(tys))),
        StringOp::ReplaceFirst => (vec![string(tys), string(tys)], s.clone()),
        _ => unreachable!("only the #3978 methods are routed here"),
    }
}

/// `padStart`/`padEnd` repeat `fill` to make up the width, so an empty `fill` has
/// nothing to repeat. A literal one is refused here; a computed one faults the call.
pub(super) fn check_pad_fill(method: &Ident, args: &[Expr], ctx: &mut Ctx) {
    if let Some(fill) = args.get(1)
        && matches!(&fill.kind, ExprKind::StrLit(s) if s.is_empty())
    {
        ctx.errors.push(
            CompileError::new(
                "bynk.string.empty_pad_fill",
                fill.span,
                format!(
                    "the `fill` of `String.{}` is empty, so there is nothing to pad with",
                    method.name
                ),
            )
            .with_note("pass a non-empty `fill`, such as `\" \"` or `\"0\"`"),
        );
    }
}

/// `xs.join(sep)` on a `List[String]`: the elements joined with `sep`, `""` for an
/// empty list. A refined `String` (`List[Email]`) widens to its base, as it does for the
/// free `bynk.string.join`, so the deprecation's fix keeps compiling; an opaque one does
/// not. Any other element type is `bynk.types.method_not_found` with a note, since there is
/// no implicit `toString`; an element that already failed to type is reported once, there.
pub(super) fn check_list_join(
    method: &Ident,
    args: &[Expr],
    elem: TyId,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let string = tys.intern(Ty::Base(BaseType::String));
    if elem.is_error(tys) {
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    if elem.base(tys) != Some(BaseType::String) {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "`List[{}]` has no one-argument `join`: it joins only a `List[String]`",
                    elem.display(tys)
                ),
            )
            .with_note("map the elements to `String` first, as in `xs.map((x) => x.toString()).join(\", \")`"),
        );
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    check_arg(&args[0], string, "the `List.join` separator", ctx);
    Some(string)
}
