//! Typed HTML (#3927, slice 1; typed-HTML §8): each `template` declaration as
//! TypeScript.
//!
//! A template is compiled, not parsed at run time. The checker's plan
//! (`bynk_check::templates::check_template`) gives its literal pieces and, for
//! each placeholder, the field and its escape. A template with fields becomes
//! `export function shell(f: PageFields): Html`, returning a template literal of
//! the pieces with each field escaped in place; one with no fields becomes an
//! exported `Html` constant. `Html` is `bynk.html`'s branded string, so the
//! result costs nothing at run time.

use bynk_check::checker::TypedCommons;
use bynk_check::templates::{Scope, Treatment, check_template};
use bynk_syntax::ast::{BaseType, EmbedDecl};
use bynk_ts::{TsDecl, TsExpr, TsParam, TsStmt, TsType};

use super::lower::escape_ts_template;
use super::runtime_use::RuntimeUse;
use super::ts_ident;
use super::ts_types::ts_type_of;
use crate::emit_model::EmitProjectCtx;
use crate::ts_build::{call, ident};

/// The parameter a template's function takes its fields record as.
const FIELDS: &str = "__fields";

/// A scalar field's text, as an interpolation hole writes it.
fn text_of(value: TsExpr, base: BaseType, runtime_use: &RuntimeUse) -> TsExpr {
    match base {
        BaseType::Decimal => {
            runtime_use.note_decimal("__bynkDecimalToString");
            call(ident("__bynkDecimalToString"), vec![value])
        }
        BaseType::Instant => call(ident("__bynkInstantToIso"), vec![value]),
        BaseType::Duration => call(ident("__bynkDurationToString"), vec![value]),
        BaseType::String => value,
        BaseType::Int | BaseType::Bool | BaseType::Float | BaseType::Bytes => {
            call(ident("String"), vec![value])
        }
    }
}

/// The exported declaration of embed `e` (#2387, slice 2): an exported `string` constant holding
/// its file's text, or, for a template (#3927), [`emit_template`]'s function or constant.
pub(super) fn emit_embed(e: &EmbedDecl, commons: &TypedCommons, ctx: &EmitProjectCtx) -> TsStmt {
    let key = bynk_project::assets::normalise_asset_path(&e.path);
    let text = ctx.embed_text.get(&key).cloned().unwrap_or_else(|| {
        panic!(
            "bynk internal error: embed `{}` reached emission without its text",
            e.name.name
        )
    });
    if e.is_template() {
        return emit_template(e, &text, commons, &ctx.runtime_use);
    }
    TsStmt::decl(
        TsDecl::Export(Box::new(TsDecl::ConstDecl {
            name: ts_ident(&e.name.name),
            ty: Some(TsType::named("string")),
            init: TsExpr::Lit(bynk_ts::TsLit::Str(text)),
        })),
        Some(e.span),
    )
}

/// The exported declaration of template `e`, from its file's `text`.
fn emit_template(
    e: &EmbedDecl,
    text: &str,
    commons: &TypedCommons,
    runtime_use: &RuntimeUse,
) -> TsStmt {
    let assets = Default::default();
    let scope = Scope {
        types: &commons.types,
        assets: &assets,
    };
    let (plan, _) = check_template(e, text, &scope);
    let plan = plan.unwrap_or_else(|| {
        panic!(
            "bynk internal error: template `{}` reached emission without passing its check",
            e.name.name
        )
    });
    let html = TsType::named("Html");
    let parts: Vec<String> = plan.pieces.iter().map(|p| escape_ts_template(p)).collect();
    let exprs: Vec<TsExpr> = plan
        .holes
        .iter()
        .map(|h| {
            let value = TsExpr::Member {
                object: Box::new(TsExpr::Ident(FIELDS.to_string())),
                property: ts_ident(&h.field),
            };
            match &h.treatment {
                Treatment::Html => value,
                Treatment::Text(b) => {
                    call(ident("escapeText"), vec![text_of(value, *b, runtime_use)])
                }
                Treatment::Attr(b) => {
                    call(ident("escapeAttr"), vec![text_of(value, *b, runtime_use)])
                }
            }
        })
        .collect();
    let body = TsExpr::As {
        expr: Box::new(TsExpr::template_lit(parts, exprs)),
        ty: html.clone(),
    };
    let fields = e.template.as_ref().and_then(|t| t.fields.as_ref());
    let decl = match fields {
        Some(fields) => TsDecl::Function {
            name: ts_ident(&e.name.name),
            generics: vec![],
            params: vec![TsParam::new(FIELDS.to_string(), Some(ts_type_of(fields)))],
            return_type: Some(html),
            body: vec![TsStmt::return_stmt(Some(body), None)],
            is_async: false,
            inline: false,
        },
        None => TsDecl::ConstDecl {
            name: ts_ident(&e.name.name),
            ty: Some(html),
            init: body,
        },
    };
    TsStmt::decl(TsDecl::Export(Box::new(decl)), Some(e.span))
}
