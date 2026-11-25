//! #3958: a type's declared bounds are checked at every application.
//!
//! `type Sorted[T: Ord]` promises that every `Sorted` holds orderable items. A
//! written argument that is a concrete type is checked by the resolver
//! (`not_orderable`, `not_keyable`, `not_comparable`); this module checks the
//! other written case, a type *parameter* as the argument (`Sorted[U]`, in a
//! signature, a field, an annotation or a constant), which must carry the same
//! bounds. It reports with the codes and the machine-applicable fix the function
//! bound rules already use (`missing_{eq,ord,key,codable}_bound`).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{TypeDecl, TypeRef};
use bynk_syntax::error::{Applicability, CompileError};

use super::equality::EqScope;

/// The constraints a type parameter can be bound by, in the order they report.
const CONSTRAINTS: [&str; 4] = ["Eq", "Ord", "Key", "Codable"];

/// What `constraint` allows the bounded position to do, for the diagnostic's
/// note (the wording the function-level rules use).
fn why(constraint: &str) -> &'static str {
    match constraint {
        "Eq" => "a type parameter that is compared must say so",
        "Ord" => "a type parameter that is ordered must say so",
        "Key" => "a type parameter that keys a `Map` or `Set` must say so",
        _ => "a type parameter that crosses the boundary must say so",
    }
}

/// Every application of a bounded type in `r` whose argument is one of
/// `params` that lacks a bound the type declares, as `missing_*_bound` errors.
/// `scope` is where `params` are declared (a function's, or a type's own).
pub(crate) fn applied_bound_errors(
    r: &TypeRef,
    params: &HashSet<String>,
    scope: &EqScope,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<CompileError> {
    let mut out = Vec::new();
    walk(r, params, scope, types, &mut out);
    out
}

fn walk(
    r: &TypeRef,
    params: &HashSet<String>,
    scope: &EqScope,
    types: &HashMap<String, Arc<TypeDecl>>,
    out: &mut Vec<CompileError>,
) {
    match r {
        TypeRef::App { name, args, .. } => {
            if let Some(decl) = types.get(&name.name) {
                for (tp, arg) in decl.type_params.iter().zip(args) {
                    let TypeRef::Named(id) = arg else { continue };
                    if !params.contains(&id.name) {
                        continue;
                    }
                    for constraint in CONSTRAINTS {
                        if !tp.bounds.iter().any(|b| b.name == constraint)
                            || scope.carries(&id.name, constraint)
                            || !scope.first_report(&id.name, constraint)
                        {
                            continue;
                        }
                        out.push(missing_bound(name, tp, &id.name, id.span, constraint, scope));
                    }
                }
            }
            for a in args {
                walk(a, params, scope, types, out);
            }
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            walk(a, params, scope, types, out);
            walk(b, params, scope, types, out);
        }
        TypeRef::Option(a, _)
        | TypeRef::Effect(a, _)
        | TypeRef::HttpResult(a, _)
        | TypeRef::List(a, _)
        | TypeRef::Set(a, _)
        | TypeRef::Query(a, _)
        | TypeRef::Stream(a, _)
        | TypeRef::Connection(a, _)
        | TypeRef::History(a, _) => walk(a, params, scope, types, out),
        TypeRef::Fn(ps, ret, _) => {
            for p in ps {
                walk(p, params, scope, types, out);
            }
            walk(ret, params, scope, types, out);
        }
        TypeRef::Base(..)
        | TypeRef::Named(_)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::RequestQuery(_)
        | TypeRef::RequestCookies(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::DecodeError(_)
        | TypeRef::Unit(_) => {}
    }
}

fn missing_bound(
    applied: &bynk_syntax::ast::Ident,
    declared_on: &bynk_syntax::ast::TypeParam,
    param: &str,
    span: bynk_syntax::span::Span,
    constraint: &str,
    scope: &EqScope,
) -> CompileError {
    let code = match constraint {
        "Eq" => "bynk.generics.missing_eq_bound",
        "Ord" => "bynk.generics.missing_ord_bound",
        "Key" => "bynk.generics.missing_key_bound",
        _ => "bynk.generics.missing_codable_bound",
    };
    let mut err = CompileError::new(
        code,
        span,
        format!(
            "`{}` requires `{}: {constraint}`, but type parameter `{param}` has no `{constraint}` bound",
            applied.name, declared_on.name.name,
        ),
    )
    .with_note(format!("{}: declare `{param}: {constraint}`", why(constraint)));
    if let Some((msg, edits)) = scope.fix_for(param, constraint) {
        err = err.with_suggestion(msg, edits, Applicability::MachineApplicable);
    }
    err
}
