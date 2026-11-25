//! #4364: a generic type that holds its parameter directly under an `Effect` cannot take an
//! `Effect` for it. `type Wrap[T] = { v: Effect[T] }` is legal, and so is `Wrap[Int]`, but
//! `Wrap[Effect[Int]]` has a field `v: Effect[Effect[Int]]`, which cannot exist at runtime (an
//! effect is a promise, and promises flatten): the same shape #4324 refuses written out directly,
//! and refused here under the same code, `bynk.types.nested_effect`.
//!
//! Only a parameter the declaration puts *directly* under an `Effect` counts. `type Box[T] =
//! { v: T }` takes `Box[Effect[Int]]`, whose field is an effect held in a record, like a
//! `List[Effect[T]]`, which is legal.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{Ident, TypeBody, TypeDecl, TypeRef};
use bynk_syntax::error::CompileError;

use super::nested_effect::nested_effect_error;
use super::{Ty, TyId, Types};

type Decls = HashMap<String, Arc<TypeDecl>>;

/// The positions of `decl`'s type parameters that its body holds directly under an `Effect`,
/// through other generic types it applies (`type Outer[U] = { w: Wrap[U] }` wraps `U` too).
pub(crate) fn wrapped_params(
    decl: &TypeDecl,
    types: &Decls,
    visiting: &mut HashSet<String>,
) -> HashSet<usize> {
    let names: HashMap<&str, usize> = decl
        .type_params
        .iter()
        .enumerate()
        .map(|(i, tp)| (tp.name.name.as_str(), i))
        .collect();
    let mut out = HashSet::new();
    if names.is_empty() {
        return out;
    }
    let refs: Vec<&TypeRef> = match &decl.body {
        TypeBody::Record(r) => r.fields.iter().map(|f| &f.type_ref).collect(),
        TypeBody::Sum(s) => s
            .variants
            .iter()
            .flat_map(|v| v.payload.iter().map(|p| &p.type_ref))
            .collect(),
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => Vec::new(),
    };
    for t in refs {
        scan(t, &names, types, visiting, &mut out);
    }
    out
}

/// A parameter of the declaration that `t` names, as its position.
fn param_of(t: &TypeRef, names: &HashMap<&str, usize>) -> Option<usize> {
    match t {
        TypeRef::Named(id) => names.get(id.name.as_str()).copied(),
        _ => None,
    }
}

fn scan(
    t: &TypeRef,
    names: &HashMap<&str, usize>,
    types: &Decls,
    visiting: &mut HashSet<String>,
    out: &mut HashSet<usize>,
) {
    match t {
        TypeRef::Effect(inner, _) => {
            out.extend(param_of(inner, names));
            scan(inner, names, types, visiting, out);
        }
        TypeRef::App { name, args, .. } => {
            if let Some(decl) = crate::prelude_record::lookup(&name.name, types)
                && visiting.insert(name.name.clone())
            {
                let inner = wrapped_params(&decl, types, visiting);
                visiting.remove(&name.name);
                for i in inner {
                    out.extend(args.get(i).and_then(|a| param_of(a, names)));
                }
            }
            for a in args {
                scan(a, names, types, visiting, out);
            }
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            scan(a, names, types, visiting, out);
            scan(b, names, types, visiting, out);
        }
        TypeRef::Option(a, _)
        | TypeRef::HttpResult(a, _)
        | TypeRef::List(a, _)
        | TypeRef::Set(a, _)
        | TypeRef::Query(a, _)
        | TypeRef::Stream(a, _)
        | TypeRef::Connection(a, _)
        | TypeRef::History(a, _) => scan(a, names, types, visiting, out),
        TypeRef::Fn(params, ret, _) => {
            for p in params {
                scan(p, names, types, visiting, out);
            }
            scan(ret, names, types, visiting, out);
        }
        _ => {}
    }
}

/// A written `Name[A, …]`: an error for each argument that is an `Effect` at a position the
/// declaration holds under an `Effect`.
pub(crate) fn check_application(
    name: &Ident,
    decl: &TypeDecl,
    args: &[TypeRef],
    types: &Decls,
) -> Vec<CompileError> {
    let mut visiting = HashSet::from([name.name.clone()]);
    let wrapped = wrapped_params(decl, types, &mut visiting);
    let mut errors = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if wrapped.contains(&i) && matches!(arg, TypeRef::Effect(..)) {
            let param = &decl.type_params[i].name.name;
            errors.push(
                nested_effect_error(arg.span(), &format!("{}[Effect[_]]", name.name)).with_note(
                    format!(
                        "`{}` holds its parameter `{param}` in an `Effect`, so an `Effect` for `{param}` would be an effect of an effect; use a parameter that is not an effect, or hold the effect in a `List` or `Option`",
                        name.name
                    ),
                ),
            );
        }
    }
    errors
}

/// The component types of `t`, for the walk below.
fn children(t: TyId, tys: &Types) -> Vec<TyId> {
    match &*tys.get(t) {
        Ty::Result(a, b) | Ty::Map(a, b) => vec![*a, *b],
        Ty::Effect(a)
        | Ty::Option(a)
        | Ty::HttpResult(a)
        | Ty::HttpOutcome(a)
        | Ty::List(a)
        | Ty::Set(a)
        | Ty::Query(a)
        | Ty::Stream(a)
        | Ty::TestConnection(a)
        | Ty::Connection(a) => vec![*a],
        Ty::Fn { params, ret } => params.iter().copied().chain([*ret]).collect(),
        Ty::Named { args, .. } => args.clone(),
        _ => Vec::new(),
    }
}

/// Does `t` apply a generic type to an `Effect` where that type holds the parameter under an
/// `Effect`, at any depth? The inferred counterpart of [`check_application`], for the type a
/// generic call returns.
pub(crate) fn nests_through_wrapper(t: TyId, tys: &Types, types: &Decls) -> bool {
    if let Ty::Named { name, args, .. } = &*tys.get(t)
        && let Some(decl) = crate::prelude_record::lookup(name, types)
    {
        let mut visiting = HashSet::from([name.clone()]);
        let wrapped = wrapped_params(&decl, types, &mut visiting);
        if wrapped.iter().any(|i| {
            args.get(*i)
                .is_some_and(|a| matches!(&*tys.get(*a), Ty::Effect(_)))
        }) {
            return true;
        }
    }
    children(t, tys)
        .into_iter()
        .any(|c| nests_through_wrapper(c, tys, types))
}
