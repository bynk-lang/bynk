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

/// The type references a declaration's body holds: its fields, or its variants' payloads.
fn body_refs(decl: &TypeDecl) -> Vec<&TypeRef> {
    match &decl.body {
        TypeBody::Record(r) => r.fields.iter().map(|f| &f.type_ref).collect(),
        TypeBody::Sum(s) => s
            .variants
            .iter()
            .flat_map(|v| v.payload.iter().map(|p| &p.type_ref))
            .collect(),
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => Vec::new(),
    }
}

/// The names of the generic types `t` applies, at any depth.
fn applied_names<'a>(t: &'a TypeRef, out: &mut Vec<&'a str>) {
    match t {
        TypeRef::App { name, args, .. } => {
            out.push(&name.name);
            args.iter().for_each(|a| applied_names(a, out));
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            applied_names(a, out);
            applied_names(b, out);
        }
        TypeRef::Effect(a, _)
        | TypeRef::Option(a, _)
        | TypeRef::HttpResult(a, _)
        | TypeRef::List(a, _)
        | TypeRef::Set(a, _)
        | TypeRef::Query(a, _)
        | TypeRef::Stream(a, _)
        | TypeRef::Connection(a, _)
        | TypeRef::History(a, _) => applied_names(a, out),
        TypeRef::Fn(params, ret, _) => {
            params.iter().for_each(|p| applied_names(p, out));
            applied_names(ret, out);
        }
        _ => {}
    }
}

/// The positions of each generic type's parameters that its body holds directly under an
/// `Effect`, for `root` and every generic type reachable from it, as a fixpoint. A position is
/// wrapped when the body puts the parameter directly under an `Effect`, or applies another
/// generic type to it at a position that type wraps (`type Outer[U] = { w: Wrap[U] }` wraps `U`
/// too). The iteration starts from nothing and only adds positions, so it ends, whatever cycles
/// the declarations form, and a recursive type that permutes its own parameters
/// (`next: Option[Swap[B, A]]`) is answered correctly.
fn wrapped_table(root: &str, types: &Decls) -> HashMap<String, HashSet<usize>> {
    let mut decls: Vec<(String, Arc<TypeDecl>)> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    let mut work = vec![root.to_string()];
    while let Some(name) = work.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(decl) = crate::prelude_record::lookup(&name, types) else {
            continue;
        };
        let mut applied = Vec::new();
        body_refs(&decl)
            .into_iter()
            .for_each(|t| applied_names(t, &mut applied));
        work.extend(applied.into_iter().map(str::to_string));
        decls.push((name, decl));
    }
    let mut table: HashMap<String, HashSet<usize>> = decls
        .iter()
        .map(|(n, _)| (n.clone(), HashSet::new()))
        .collect();
    loop {
        let mut changed = false;
        for (name, decl) in &decls {
            let names: HashMap<&str, usize> = decl
                .type_params
                .iter()
                .enumerate()
                .map(|(i, tp)| (tp.name.name.as_str(), i))
                .collect();
            if names.is_empty() {
                continue;
            }
            let mut found = HashSet::new();
            for t in body_refs(decl) {
                scan(t, &names, &table, &mut found);
            }
            let entry = table.entry(name.clone()).or_default();
            let before = entry.len();
            entry.extend(found);
            changed |= entry.len() != before;
        }
        if !changed {
            return table;
        }
    }
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
    table: &HashMap<String, HashSet<usize>>,
    out: &mut HashSet<usize>,
) {
    match t {
        TypeRef::Effect(inner, _) => {
            out.extend(param_of(inner, names));
            scan(inner, names, table, out);
        }
        TypeRef::App { name, args, .. } => {
            for i in table.get(&name.name).into_iter().flatten() {
                out.extend(args.get(*i).and_then(|a| param_of(a, names)));
            }
            for a in args {
                scan(a, names, table, out);
            }
        }
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            scan(a, names, table, out);
            scan(b, names, table, out);
        }
        TypeRef::Option(a, _)
        | TypeRef::HttpResult(a, _)
        | TypeRef::List(a, _)
        | TypeRef::Set(a, _)
        | TypeRef::Query(a, _)
        | TypeRef::Stream(a, _)
        | TypeRef::Connection(a, _)
        | TypeRef::History(a, _) => scan(a, names, table, out),
        TypeRef::Fn(params, ret, _) => {
            for p in params {
                scan(p, names, table, out);
            }
            scan(ret, names, table, out);
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
    let wrapped = wrapped_table(&name.name, types)
        .remove(&name.name)
        .unwrap_or_default();
    let mut errors = Vec::new();
    for (i, arg) in args.iter().enumerate() {
        if wrapped.contains(&i) && matches!(arg, TypeRef::Effect(..)) {
            let param = &decl.type_params[i].name.name;
            errors.push(
                nested_effect_error(arg.span(), &format!("{}[Effect[_]]", name.name))
                    .with_note(wrapper_note(&name.name, param)),
            );
        }
    }
    errors
}

/// Why a wrapper refuses an effect.
fn wrapper_note(wrapper: &str, param: &str) -> String {
    format!(
        "`{wrapper}` holds its parameter `{param}` in an `Effect`, so an `Effect` for `{param}` would be an effect of an effect; use a parameter that is not an effect, or hold the effect in a `List` or `Option`"
    )
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
/// generic call returns: the note explaining the first wrapper that does, or `None`.
pub(crate) fn nests_through_wrapper(t: TyId, tys: &Types, types: &Decls) -> Option<String> {
    if let Ty::Named { name, args, .. } = &*tys.get(t)
        && let Some(decl) = crate::prelude_record::lookup(name, types)
    {
        let wrapped = wrapped_table(name, types).remove(name).unwrap_or_default();
        for i in wrapped {
            if args
                .get(i)
                .is_some_and(|a| matches!(&*tys.get(*a), Ty::Effect(_)))
            {
                let param = decl
                    .type_params
                    .get(i)
                    .map_or("T", |tp| tp.name.name.as_str());
                return Some(wrapper_note(name, param));
            }
        }
    }
    children(t, tys)
        .into_iter()
        .find_map(|c| nests_through_wrapper(c, tys, types))
}
