//! Which types are `Ord`, and the checks that need it. Constrained generics
//! slice 1 (#2660; the model is the track's Q2/Q5 in
//! `design/tracks/constrained-generics.md`).
//!
//! `Ord` is a declared, structural property. A type is `Ord` when it is one of:
//!
//! - `Int`, `Float`, `String`, `Duration`, `Instant` or `Bytes`, or a refined type
//!   over one of them (a refinement is visible everywhere, so it needs no
//!   declaration);
//! - `Option[T]` or `List[T]` with `T` `Ord`;
//! - a record, sum, enum or opaque type that declares `derives Ord`, and, when it
//!   is generic, whose type arguments are `Ord`;
//! - a type parameter declared `T: Ord` (checked separately: see
//!   [`super::equality::require_ord_vars`]).
//!
//! `Bool`, `Result`, `Map`, `Set`, functions and held resources are not `Ord`.
//!
//! `derives Ord` makes declaration order meaningful: a record orders by its fields
//! in declaration order, a sum by variant declaration order and then by payload.
//! That is why `Ord` is opt-in (Q2) while `Eq` is automatic.
//!
//! `Query`'s `sortBy`/`min`/`max` keep the five-base-type rule on their own
//! predicate (`kernels::is_orderable`): their ordering runs on the storage path,
//! where a user-type comparator has no SQL lowering (DECISION H).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{BaseType, FnDecl, FnName, Ident, TypeBody, TypeDecl, TypeRef};
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

use super::equality::{has_ord, require_ord_vars};
use super::{Ctx, NamedKind, Ty, TyId};

/// Why a type is not `Ord`: the first offending part found.
pub(crate) struct NotOrd {
    /// The offending part, as the author would write it.
    pub part: String,
    /// The user-declared type that lacks `derives Ord`, when that is the cause.
    pub decl: Option<Arc<TypeDecl>>,
}

/// Whether a base type is `Ord`.
fn base_is_ord(b: BaseType) -> bool {
    !matches!(b, BaseType::Bool)
}

/// The first part of `ty` that is not `Ord`, or `None` when `ty` is `Ord` all the
/// way down. A type variable counts as `Ord` here: whether the enclosing
/// function declared the bound is [`require_ord_vars`]'s question.
pub(crate) fn not_orderable(ty: TyId, ctx: &Ctx) -> Option<NotOrd> {
    let tys = ctx.tys;
    match &*tys.get(ty) {
        Ty::Base(b) if base_is_ord(*b) => None,
        Ty::Var(_) => None,
        Ty::Option(t) | Ty::List(t) => not_orderable(*t, ctx),
        Ty::Named {
            name, kind, args, ..
        } => {
            match kind {
                NamedKind::Refined(b) => {
                    if !base_is_ord(*b) {
                        return Some(NotOrd {
                            part: tys.display(ty),
                            decl: None,
                        });
                    }
                }
                NamedKind::Record | NamedKind::Sum | NamedKind::Opaque(_) => {
                    let decl = ctx.input.types.get(name);
                    if !decl.is_some_and(|d| has_ord(&d.derives)) {
                        return Some(NotOrd {
                            part: tys.display(ty),
                            decl: decl.cloned(),
                        });
                    }
                }
            }
            args.iter().find_map(|a| not_orderable(*a, ctx))
        }
        _ => Some(NotOrd {
            part: tys.display(ty),
            decl: None,
        }),
    }
}

/// What a use of `Ord` is for, which decides the diagnostic's wording and code.
pub(crate) enum OrdSite<'a> {
    /// A comparison operator (`<`, `<=`, `>`, `>=`).
    Operator(&'a str),
    /// A `List` ordering kernel's key (`List.sortBy`, `List.min`, `List.max`).
    Key(&'a str),
    /// A call to an `Ord`-bounded callee: `callee` requires `param: Ord`.
    Call { callee: &'a str, param: &'a str },
}

/// Check that `ty` can be ordered at `span`: every concrete part is `Ord`
/// ([`not_orderable`]) and every type parameter in it is declared `Ord`
/// ([`require_ord_vars`]). Reports and returns `false` otherwise.
pub(crate) fn check_ord(ctx: &mut Ctx, ty: TyId, span: Span, site: &OrdSite) -> bool {
    if let Some(blocker) = not_orderable(ty, ctx) {
        report_not_orderable(ctx, ty, span, site, blocker);
        return false;
    }
    let clause = match site {
        OrdSite::Operator(op) => format!("operator `{op}` orders values of this type"),
        OrdSite::Key(method) => format!("`{method}` orders by this key"),
        OrdSite::Call { callee, param } => format!("`{callee}` requires `{param}: Ord`"),
    };
    require_ord_vars(ctx, ty, span, &clause)
}

fn report_not_orderable(ctx: &mut Ctx, ty: TyId, span: Span, site: &OrdSite, blocker: NotOrd) {
    let tys = ctx.tys;
    let shown = ty.display(tys);
    let (code, message) = match site {
        OrdSite::Operator(op) => (
            "bynk.types.not_orderable",
            format!("operator `{op}` needs an orderable type, but `{shown}` is not"),
        ),
        OrdSite::Key(method) => (
            "bynk.types.key_not_orderable",
            format!("`{method}` needs an orderable key, but the key function returns `{shown}`"),
        ),
        OrdSite::Call { callee, param } => (
            "bynk.types.not_orderable",
            format!("`{callee}` requires `{param}: Ord`, but this call makes `{param}` `{shown}`"),
        ),
    };
    let mut err = CompileError::new(code, span, message);
    match &blocker.decl {
        Some(decl) => {
            err = err.with_note(format!(
                "`{}` does not declare `derives Ord`; declaring it orders the type by declaration order (fields, then variants), so reordering the declaration changes the order",
                decl.name.name
            ));
            // The edit lands in the declaring file; a suggestion for a type
            // declared elsewhere would render against the wrong source.
            if decl.span.file == span.file {
                let end = decl.span.end;
                err = err.with_suggestion(
                    format!("add `derives Ord` to `{}`", decl.name.name),
                    vec![(Span::new_in(decl.span.file, end, end), " derives Ord".to_string())],
                    Applicability::MaybeIncorrect,
                );
            }
        }
        None => {
            err = err.with_note(format!(
                "`{}` is not orderable; orderable types are `Int`, `Float`, `String`, `Duration`, `Instant`, `Bytes`, `Option` and `List` of an orderable type, refined types over them, and types that declare `derives Ord`",
                blocker.part
            ));
        }
    }
    ctx.errors.push(err);
}

// -- `derives` validation (declaration side) --

/// Whether a declared field type is `Ord`, with the declaring type's own
/// parameters `params` taken as `Ord`. `visited` keeps recursive types finite.
/// Returns the first offending part, rendered as written.
pub(crate) fn typeref_not_ord(
    t: &TypeRef,
    params: &[&str],
    types: &HashMap<String, Arc<TypeDecl>>,
    visited: &mut HashSet<String>,
) -> Option<String> {
    match t {
        TypeRef::Base(b, _) => (!base_is_ord(*b)).then(|| super::equality::render(t)),
        TypeRef::Option(a, _) | TypeRef::List(a, _) => typeref_not_ord(a, params, types, visited),
        TypeRef::Named(id) if params.contains(&id.name.as_str()) => None,
        TypeRef::Named(id) => named_not_ord(&id.name, &[], t, params, types, visited),
        TypeRef::App { name, args, .. } => named_not_ord(&name.name, args, t, params, types, visited),
        _ => Some(super::equality::render(t)),
    }
}

fn named_not_ord(
    name: &str,
    args: &[TypeRef],
    whole: &TypeRef,
    params: &[&str],
    types: &HashMap<String, Arc<TypeDecl>>,
    visited: &mut HashSet<String>,
) -> Option<String> {
    let Some(decl) = types.get(name) else {
        // An unresolved name is reported by the resolver, not here.
        return None;
    };
    match &decl.body {
        TypeBody::Refined { base, .. } => {
            if !base_is_ord(*base) {
                return Some(super::equality::render(whole));
            }
        }
        _ => {
            if !has_ord(&decl.derives) {
                return Some(super::equality::render(whole));
            }
        }
    }
    let _ = visited;
    args.iter()
        .find_map(|a| typeref_not_ord(a, params, types, visited))
}

/// `derives` validation for one type declaration: unknown names, the automatic
/// ones, and, for `Ord`, that every field and payload is `Ord`.
pub(crate) fn check_derives(
    decl: &TypeDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
) {
    for (i, d) in decl.derives.iter().enumerate() {
        match d.name.as_str() {
            "Ord" => {}
            "Eq" | "Key" | "Codable" => errors.push(derives_automatic(decl, i, d)),
            other => errors.push(
                CompileError::new(
                    "bynk.generics.unknown_constraint",
                    d.span,
                    format!(
                        "`{other}` is not a constraint a type can derive (type `{}`)",
                        decl.name.name
                    ),
                )
                .with_note("a type can derive `Ord`; `Eq`, `Key` and `Codable` hold automatically"),
            ),
        }
    }
    if !has_ord(&decl.derives) {
        return;
    }
    let params: Vec<&str> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.as_str())
        .collect();
    let mut visited = HashSet::new();
    let mut fail = |what: String, part: String, span: Span| {
        errors.push(
            CompileError::new(
                "bynk.types.derives_field_not_orderable",
                span,
                format!(
                    "`{}` derives `Ord`, but {what} has type `{part}`, which is not orderable",
                    decl.name.name
                ),
            )
            .with_note("every field and payload of a type that derives `Ord` must itself be `Ord`"),
        );
    };
    match &decl.body {
        TypeBody::Record(r) => {
            for f in &r.fields {
                if let Some(part) = typeref_not_ord(&f.type_ref, &params, types, &mut visited) {
                    fail(format!("field `{}`", f.name.name), part, f.type_ref.span());
                }
            }
        }
        TypeBody::Sum(s) => {
            for v in &s.variants {
                for p in &v.payload {
                    if let Some(part) = typeref_not_ord(&p.type_ref, &params, types, &mut visited)
                    {
                        fail(
                            format!("`{}.{}`'s field `{}`", decl.name.name, v.name.name, p.name.name),
                            part,
                            p.type_ref.span(),
                        );
                    }
                }
            }
        }
        TypeBody::Opaque { base, base_span, .. } => {
            if !base_is_ord(*base) {
                fail("its representation".to_string(), base.name().to_string(), *base_span);
            }
        }
        // A refined type is `Ord` over an orderable base without declaring it.
        TypeBody::Refined { .. } => {}
    }
}

fn derives_automatic(decl: &TypeDecl, index: usize, d: &Ident) -> CompileError {
    let mut err = CompileError::new(
        "bynk.types.derives_automatic",
        d.span,
        format!(
            "`{}` holds for `{}` automatically; there is nothing to derive",
            d.name, decl.name.name
        ),
    )
    .with_note("only `Ord` is opted into with `derives`");
    // Remove the name; when it is the only one, the whole clause goes too.
    let names = &decl.derives;
    let edit = if names.len() == 1 {
        // `<body> derives Eq` -> `<body>`: the keyword through the name, and the
        // one separator before it.
        let kw = decl.derives_kw.unwrap_or(d.span);
        (
            Span::new_in(d.span.file, kw.start.saturating_sub(1), d.span.end),
            String::new(),
        )
    } else if index + 1 < names.len() {
        (Span::new_in(d.span.file, d.span.start, names[index + 1].span.start), String::new())
    } else {
        (Span::new_in(d.span.file, names[index - 1].span.end, d.span.end), String::new())
    };
    err = err.with_suggestion(
        format!("remove `{}`", d.name),
        vec![edit],
        Applicability::MachineApplicable,
    );
    err
}

// -- witnesses --

/// The type parameters of `f` that take a comparator witness, in witness order
/// (DECISION D): the receiver type's parameters first, in the type's declaration
/// order, then the function's own, in declaration order. A parameter is a witness
/// parameter when declared `Ord` inline (`T: Ord`), in a `where` entry, or, for
/// a receiver parameter, on the type (`type Sorted[T: Ord]`). The emitter adds
/// one trailing parameter per name, and every call site passes one argument per
/// name, so this order is the contract between them; it is not part of the
/// published ABI.
pub fn witness_params(f: &FnDecl, types: &HashMap<String, Arc<TypeDecl>>) -> Vec<String> {
    let where_ord = |name: &str| {
        f.where_bounds
            .iter()
            .any(|wb| wb.param.name == name && has_ord(&wb.bounds))
    };
    let mut out = Vec::new();
    if let FnName::Method { type_name, .. } = &f.name
        && let Some(decl) = types.get(&type_name.name)
    {
        for tp in &decl.type_params {
            if has_ord(&tp.bounds) || where_ord(&tp.name.name) {
                out.push(tp.name.name.clone());
            }
        }
    }
    for tp in &f.type_params {
        if has_ord(&tp.bounds) || where_ord(&tp.name.name) {
            out.push(tp.name.name.clone());
        }
    }
    out
}

/// At a call, check each `Ord`-bounded type parameter of the callee (`params`, in
/// witness order) against its instantiation (`subst`) and collect the
/// instantiations: the call's witnesses. `None` after reporting the first
/// violation (`not_orderable`, or `missing_ord_bound` when the argument is a
/// type parameter the caller has not declared `Ord`).
pub(crate) fn check_declared_ord_args(
    callee: &str,
    call_span: Span,
    params: &[String],
    subst: &HashMap<String, TyId>,
    ctx: &mut Ctx,
) -> Option<Vec<TyId>> {
    let mut out = Vec::new();
    for param in params {
        let &arg_ty = subst.get(param)?;
        if !check_ord(ctx, arg_ty, call_span, &OrdSite::Call { callee, param }) {
            return None;
        }
        out.push(arg_ty);
    }
    Some(out)
}
