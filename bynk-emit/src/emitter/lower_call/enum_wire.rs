//! #4560: `e.wireName()` and `E.fromWireName(s)`, lowered from the wire names the codecs use.
//!
//! The names are read off the enum's declaration with `Variant::wire_name`, the same call that
//! builds the codec's tables (`bynk_check::wire`), so the text this writes and reads is the text
//! `Json.encodePlain`, `Form` and the path/query/header/cookie readers use, `@name` included.
//! Both forms are inline, with no runtime helper: an object literal read by the value's `tag`,
//! and a chain of comparisons that answers the variant or `None`.

use bynk_check::checker::{Callee, Ty};
use bynk_syntax::ast::{Expr, TypeBody, TypeDecl};
use bynk_ts::{TsArrowBody, TsBinaryOp, TsExpr, TsObjectEntry, TsParam, TsType};

use super::super::lower::Pre;
use super::super::lower_ctx::LowerCtx;
use super::super::ts_types::ts_ty_to_ts_type;
use crate::ts_build::{call, ident, member, str_lit};

/// A variant's wire name: its `@name`, else its own name.
fn wire_names(decl: &TypeDecl) -> Vec<(String, String)> {
    match &decl.body {
        TypeBody::Sum(s) => s
            .variants
            .iter()
            .map(|v| {
                (
                    v.name.name.clone(),
                    v.wire_name().unwrap_or_else(|| v.name.name.clone()),
                )
            })
            .collect(),
        TypeBody::Refined { .. } | TypeBody::Record(_) | TypeBody::Opaque { .. } => Vec::new(),
    }
}

/// `e.wireName()`: `({ Open: "open", … })[e.tag]`.
pub(in crate::emitter) fn lower_wire_name(
    recv_ty: bynk_check::checker::TyId,
    receiver: &Expr,
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<TsExpr> {
    let Ty::Named { name, .. } = (*cx.ty_render().get(recv_ty)).clone() else {
        return None;
    };
    let decl = cx.commons().types.get(&name)?.clone();
    let table = TsExpr::object_entries(
        wire_names(&decl)
            .into_iter()
            .map(|(variant, wire)| TsObjectEntry::prop(variant, str_lit(wire)))
            .collect(),
    );
    let value = pre.lower_node(receiver, cx);
    Some(TsExpr::Index {
        object: Box::new(TsExpr::Paren(Box::new(table))),
        index: Box::new(member(value, "tag")),
    })
}

/// `E.fromWireName(s)`: an arrow over the text that answers `Some(variant)` for the variant
/// whose wire name equals it exactly, else `None`.
pub(in crate::emitter) fn lower_from_wire_name(
    e: &Expr,
    args: &[Expr],
    cx: &mut LowerCtx,
    pre: &mut Pre,
) -> Option<TsExpr> {
    let Some(Callee::EnumFromWireName(decl)) = cx.commons().callee(e.id).cloned() else {
        return None;
    };
    let [arg] = args else {
        return None;
    };
    let result = cx.commons().expr_types.get(&e.id)?.ty;
    let Ty::Option(inner) = (*cx.ty_render().get(result)).clone() else {
        return None;
    };
    let ty: TsType = ts_ty_to_ts_type(inner, cx.ty_render());
    let mut chain = ident("None");
    for (variant, wire) in wire_names(&decl).into_iter().rev() {
        let value = TsExpr::Paren(Box::new(TsExpr::As {
            expr: Box::new(TsExpr::object_entries(vec![TsObjectEntry::prop(
                "tag".to_string(),
                str_lit(variant),
            )])),
            ty: ty.clone(),
        }));
        chain = TsExpr::Conditional {
            test: Box::new(TsExpr::Binary {
                op: TsBinaryOp::StrictEq,
                left: Box::new(ident("__s")),
                right: Box::new(str_lit(wire)),
            }),
            consequent: Box::new(call(ident("Some"), vec![value])),
            alternate: Box::new(chain),
        };
    }
    let text = pre.lower_node(arg, cx);
    let arrow = TsExpr::Arrow {
        params: vec![TsParam::new(
            "__s".to_string(),
            Some(TsType::named("string")),
        )],
        is_async: false,
        generics: vec![],
        return_type: None,
        body: Box::new(TsArrowBody::Expr(Box::new(chain))),
    };
    Some(TsExpr::Call {
        callee: Box::new(TsExpr::Paren(Box::new(arrow))),
        args: vec![text],
    })
}
