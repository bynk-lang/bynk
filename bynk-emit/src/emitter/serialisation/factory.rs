//! Codec factories for user generic types (back-half paydown 2.3b, #4088, #2792): `codec_factory_*` and
//! `emit_codec_factory`. Moved out of `serialisation.rs` byte for byte.

use std::sync::Arc;

use bynk_syntax::ast::{TypeDecl, TypeRef};
use bynk_ts::{TsArrowBody, TsDecl, TsExpr, TsParam, TsStmt, TsType};

use crate::emitter::RuntimeUse;
use crate::ts_build::{call, const_, ident, member, ret};

use super::generics::emit_generic_helpers;
use super::support::deserialise_path_param;
use super::{GenericInst, app_ts_name, record_inst_fields, sum_inst_variants};

/// Prefix of the stand-in name a codec factory gives a generic declaration's
/// type parameter. The prefix keeps it from colliding with a declared type of
/// the same name; the helpers nested in the factory name it in their suffixes.
const FACTORY_PARAM_PREFIX: &str = "__P_";

/// The TypeScript name of the codec factory for generic declaration `name`.
/// Keyed on the bare declaration name, as every other codec name is: a module's
/// type table (`TypedCommons::types`) holds one declaration per name, so two
/// same-named generic declarations from different units cannot both be in scope.
pub(crate) fn codec_factory_ident(name: &str) -> String {
    format!("__codecOf_{name}")
}

/// #2792: the root a codec factory is requested with: the generic declaration
/// `name` applied to its own type parameters, each as a stand-in name. `None`
/// when `name` is not a generic record or sum.
pub(crate) fn codec_factory_root(
    name: &str,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Option<TypeRef> {
    let decl = types.get(name)?;
    if decl.type_params.is_empty()
        || !matches!(
            decl.body,
            bynk_syntax::ast::TypeBody::Record(_) | bynk_syntax::ast::TypeBody::Sum(_)
        )
    {
        return None;
    }
    let sp = bynk_syntax::span::Span::new(0, 0);
    Some(TypeRef::App {
        name: bynk_syntax::ast::Ident {
            name: name.to_string(),
            span: sp,
        },
        args: decl
            .type_params
            .iter()
            .map(|p| {
                TypeRef::Named(bynk_syntax::ast::Ident {
                    name: format!("{FACTORY_PARAM_PREFIX}{}", p.name.name),
                    span: sp,
                })
            })
            .collect(),
        span: sp,
    })
}

/// Whether `t` is a codec factory root ([`codec_factory_root`]), and for which
/// declaration.
pub(crate) fn codec_factory_name(t: &TypeRef) -> Option<&str> {
    match t {
        TypeRef::App { name, args, .. }
            if args.iter().all(
                |a| matches!(a, TypeRef::Named(id) if id.name.starts_with(FACTORY_PARAM_PREFIX)),
            ) && !args.is_empty() =>
        {
            Some(&name.name)
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::Result(..)
        | TypeRef::Option(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::List(..)
        | TypeRef::Map(..)
        | TypeRef::Set(..)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..)
        | TypeRef::App { .. } => None,
    }
}

fn mentions_factory_param(t: &TypeRef) -> bool {
    match t {
        TypeRef::Named(id) => id.name.starts_with(FACTORY_PARAM_PREFIX),
        TypeRef::App { args, .. } => args.iter().any(mentions_factory_param),
        TypeRef::Option(a, _) | TypeRef::List(a, _) | TypeRef::Set(a, _) => {
            mentions_factory_param(a)
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            mentions_factory_param(a) || mentions_factory_param(b)
        }
        TypeRef::Base(..)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..) => false,
    }
}

/// The type-parameter-free parts of the declaration's fields: the types whose
/// ordinary codecs the factory calls, which the module must emit beside it.
fn factory_ground_parts(t: &TypeRef, out: &mut Vec<TypeRef>) {
    if !mentions_factory_param(t) {
        out.push(t.clone());
        return;
    }
    match t {
        TypeRef::App { args, .. } => args.iter().for_each(|a| factory_ground_parts(a, out)),
        TypeRef::Option(a, _) | TypeRef::List(a, _) | TypeRef::Set(a, _) => {
            factory_ground_parts(a, out)
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            factory_ground_parts(a, out);
            factory_ground_parts(b, out);
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::Effect(..)
        | TypeRef::HttpResult(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::Query(..)
        | TypeRef::Stream(..)
        | TypeRef::Connection(..)
        | TypeRef::History(..)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_)
        | TypeRef::Fn(..) => {}
    }
}

/// The field and payload types of the factory's declaration, instantiated at
/// its stand-in parameter names.
fn factory_field_types(
    root: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Vec<TypeRef> {
    let TypeRef::App { name, args, .. } = root else {
        return Vec::new();
    };
    if let Some(fields) = record_inst_fields(&name.name, args, types) {
        return fields.into_iter().map(|(_, t)| t).collect();
    }
    sum_inst_variants(&name.name, args, types)
        .map(|vs| {
            vs.into_iter()
                .flat_map(|(_, fields)| fields.into_iter().map(|(_, t)| t))
                .collect()
        })
        .unwrap_or_default()
}

/// #2792: the roots the module's ordinary codec closure needs for the
/// type-parameter-free parts of a factory's declaration.
pub(crate) fn codec_factory_ground_roots(
    root: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
) -> Vec<TypeRef> {
    let mut out = Vec::new();
    for t in factory_field_types(root, types) {
        factory_ground_parts(&t, &mut out);
    }
    out
}

/// #2792: the codec factory for a generic record or sum, `root` being its
/// [`codec_factory_root`]. A function from the witnesses of the declaration's
/// type parameters to the `{ encode, decode }` codec of the instantiation.
///
/// The body is the monomorphised codec for the declaration applied to its
/// stand-in parameter names, nested so that the calls it makes for a parameter
/// (`__serialise___P_T`, `__deserialise___P_T`) reach the witness. Every other
/// codec it calls is the module's ordinary one. So the JSON, the error paths
/// and the revalidation are the monomorphised codec's, by construction.
pub(crate) fn emit_codec_factory(
    root: &TypeRef,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    ru: &RuntimeUse,
) -> TsDecl {
    let TypeRef::App { name, args, .. } = root else {
        unreachable!("bynk internal error: a codec factory root is a generic application");
    };
    let params: Vec<String> = args
        .iter()
        .map(|a| match a {
            TypeRef::Named(id) => id.name.clone(),
            TypeRef::Base(..)
            | TypeRef::Result(..)
            | TypeRef::Option(..)
            | TypeRef::Effect(..)
            | TypeRef::HttpResult(..)
            | TypeRef::QueueResult(_)
            | TypeRef::Asset(_)
            | TypeRef::RequestHeaders(_)
            | TypeRef::RequestQuery(_)
            | TypeRef::RequestCookies(_)
            | TypeRef::TokenClaims(_)
            | TypeRef::BoundaryRejection(_)
            | TypeRef::List(..)
            | TypeRef::Map(..)
            | TypeRef::Set(..)
            | TypeRef::Query(..)
            | TypeRef::Stream(..)
            | TypeRef::Connection(..)
            | TypeRef::History(..)
            | TypeRef::ValidationError(_)
            | TypeRef::DecodeError(_)
            | TypeRef::Unit(_)
            | TypeRef::Fn(..)
            | TypeRef::App { .. } => {
                unreachable!("bynk internal error: a codec factory root names its parameters")
            }
        })
        .collect();
    let (_, insts) = bynk_check::wire::collect_codec_closure(std::slice::from_ref(root), types);
    // The helpers that mention a parameter nest in the factory; the rest are
    // the module's own, emitted from `codec_factory_ground_roots`.
    let nested: Vec<GenericInst> = insts
        .into_iter()
        .filter(|i| i.ts_name().contains(FACTORY_PARAM_PREFIX))
        .collect();
    let mut body: Vec<TsStmt> = Vec::new();
    for p in &params {
        let witness = |method: &str, args: Vec<TsExpr>| {
            call(member(ident(format!("__codec_{p}")), method), args)
        };
        let arrow = |params: Vec<TsParam>, body: TsExpr| TsExpr::Arrow {
            params,
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(body))),
        };
        body.push(const_(
            format!("__serialise_{p}"),
            arrow(
                vec![TsParam::new("v", Some(TsType::named(p.clone())))],
                witness("encode", vec![ident("v")]),
            ),
        ));
        body.push(const_(
            format!("__deserialise_{p}"),
            arrow(
                vec![
                    TsParam::new("j", Some(TsType::named("__JsonValue"))),
                    deserialise_path_param(),
                ],
                witness("decode", vec![ident("j"), ident("path")]),
            ),
        ));
    }
    for d in emit_generic_helpers(&nested, types, ru) {
        let d = if let TsDecl::Export(inner) = d {
            *inner
        } else {
            d
        };
        body.push(TsStmt::decl(d, None));
    }
    let suffix = app_ts_name(&name.name, args);
    let ts_type = format!("{}<{}>", name.name, params.join(", "));
    body.push(ret(TsExpr::object(vec![
        ("encode".to_string(), ident(format!("__serialise_{suffix}"))),
        (
            "decode".to_string(),
            ident(format!("__deserialise_{suffix}")),
        ),
    ])));
    let witness_ty = |p: &str| {
        format!(
            "{{ encode: (v: {p}) => __JsonValue; decode: (j: __JsonValue, path: string) => Result<{p}, __BoundaryError> }}"
        )
    };
    TsDecl::Function {
        name: codec_factory_ident(&name.name),
        generics: params.clone(),
        params: params
            .iter()
            .map(|p| TsParam::new(format!("__codec_{p}"), Some(TsType::named(witness_ty(p)))))
            .collect(),
        return_type: Some(TsType::named(format!(
            "{{ encode: (v: {ts_type}) => __JsonValue; decode: (j: __JsonValue, path?: string) => Result<{ts_type}, __BoundaryError> }}"
        ))),
        body,
        is_async: false,
        inline: false,
    }
}
