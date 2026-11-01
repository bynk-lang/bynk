//! Which types are `Key`, and the checks that need it. Constrained generics
//! slice 2 (#2684; the model is the track's Q6 in
//! `design/tracks/constrained-generics.md`).
//!
//! `Key` admits a type as a value `Map` key or `Set` member (and as a `distinct`,
//! `groupBy` or join key). It is automatic, never derived: a type is `Key` when it
//! is one of:
//!
//! - `String`, `Int`, `Bool`, `Instant`, `Duration` or `Bytes`, or a refined or
//!   opaque type over one of them;
//! - `Option[T]` or `List[T]` with `T` `Key`;
//! - a record, sum or enum whose every field and payload is `Key`, and, when it is
//!   generic, whose type arguments are `Key`;
//! - a type parameter declared `K: Key` (checked separately: see
//!   [`super::equality::require_key_vars`]).
//!
//! `Float` is not `Key` at any depth (`NaN` is not equal to itself), nor are
//! `Map` and `Set` (an order-independent encoding is possible but nothing needs
//! one yet), functions, `Effect`, `Query`, streams, held resources, actors and
//! the boundary-only types.
//!
//! The runtime holds a composite key under a canonical encoding
//! (`__bynkKeyEncode`); `Key` needs no witness, so it adds no hidden argument.
//!
//! The store rule (`store m: Map[K, V]`, `Cache`, `@indexed`) and `Query`'s
//! `distinct`/`groupBy`/join keys keep their own `String`/`Int` predicates: both
//! persist or hash keys through `String(k)`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{BaseType, TypeBody, TypeDecl, TypeRef};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use super::equality::{EqScope, require_key_vars, require_key_vars_in};
use super::{Ctx, NamedKind, Ty, TyId, Types};

/// Why a type is not `Key`: the first offending part found.
pub(crate) struct NotKey {
    /// The offending part, as the author would write it.
    pub part: String,
    /// Whether the part is (or contains) a `Float`.
    pub float: bool,
}

fn base_is_key(b: BaseType) -> bool {
    !matches!(b, BaseType::Float)
}

/// The first part of `ty` that is not `Key`, or `None` when `ty` is `Key` all the
/// way down. A type variable counts as `Key` here: whether the enclosing function
/// declared the bound is `require_key_vars`'s question.
pub(crate) fn not_keyable(ty: TyId, ctx: &Ctx) -> Option<NotKey> {
    let mut visited = HashSet::new();
    walk_ty(ty, ctx, &mut visited)
}

fn walk_ty(ty: TyId, ctx: &Ctx, visited: &mut HashSet<String>) -> Option<NotKey> {
    let tys = ctx.tys;
    match &*tys.get(ty) {
        Ty::Base(b) if base_is_key(*b) => None,
        Ty::Base(_) => Some(NotKey {
            part: tys.display(ty),
            float: true,
        }),
        Ty::Var(_) => None,
        Ty::Option(t) | Ty::List(t) => walk_ty(*t, ctx, visited),
        Ty::Named {
            name, kind, args, ..
        } => {
            if let NamedKind::Refined(b) | NamedKind::Opaque(b) = kind
                && !base_is_key(*b)
            {
                return Some(NotKey {
                    part: tys.display(ty),
                    float: true,
                });
            }
            let args = args.clone();
            let name = name.clone();
            for a in &args {
                if let Some(found) = walk_ty(*a, ctx, visited) {
                    return Some(found);
                }
            }
            walk_decl(&name, &ctx.input.types, visited)
        }
        _ => Some(NotKey {
            part: tys.display(ty),
            float: false,
        }),
    }
}

/// Walk a named type's declared fields. Each name is walked once per check, which
/// keeps recursive types finite.
fn walk_decl(
    name: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    visited: &mut HashSet<String>,
) -> Option<NotKey> {
    if !visited.insert(name.to_string()) {
        return None;
    }
    let decl = types.get(name)?;
    let params: Vec<&str> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.as_str())
        .collect();
    match &decl.body {
        TypeBody::Record(r) => r
            .fields
            .iter()
            .find_map(|f| walk_ref(&f.type_ref, &params, types, visited)),
        TypeBody::Sum(s) => s
            .variants
            .iter()
            .flat_map(|v| v.payload.iter())
            .find_map(|p| walk_ref(&p.type_ref, &params, types, visited)),
        TypeBody::Opaque { base, .. } | TypeBody::Refined { base, .. } => (!base_is_key(*base))
            .then(|| NotKey {
                part: base.name().to_string(),
                float: true,
            }),
    }
}

/// The declaration-side walk: the first part of a declared type that is not `Key`,
/// with the declaring type's own parameters (`params`) taken as `Key`.
pub(crate) fn typeref_not_key(
    t: &TypeRef,
    params: &[&str],
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<NotKey> {
    walk_ref(t, params, types, &mut HashSet::new())
}

fn walk_ref(
    t: &TypeRef,
    params: &[&str],
    types: &HashMap<String, Arc<TypeDecl>>,
    visited: &mut HashSet<String>,
) -> Option<NotKey> {
    match t {
        TypeRef::Base(b, _) => (!base_is_key(*b)).then(|| NotKey {
            part: b.name().to_string(),
            float: true,
        }),
        TypeRef::Option(a, _) | TypeRef::List(a, _) => walk_ref(a, params, types, visited),
        TypeRef::Named(id) if params.contains(&id.name.as_str()) => None,
        // An unresolved name is reported by the resolver, not here.
        TypeRef::Named(id) => walk_decl(&id.name, types, visited),
        TypeRef::App { name, args, .. } => args
            .iter()
            .find_map(|a| walk_ref(a, params, types, visited))
            .or_else(|| walk_decl(&name.name, types, visited)),
        _ => Some(NotKey {
            part: super::equality::render(t),
            float: false,
        }),
    }
}

/// What a use of `Key` is for, which decides the diagnostic's wording.
pub(crate) enum KeySite<'a> {
    /// A `Map` key.
    MapKey,
    /// A `Set` member.
    SetElement,
    /// A `List` kernel's element or key (`distinct`, `distinctBy`, `groupBy`,
    /// `joinOn`, `leftJoin`).
    Kernel(&'a str),
    /// A call to a `Key`-bounded callee: `callee` requires `param: Key`.
    Call { callee: &'a str, param: &'a str },
}

/// Check that `ty` can key a collection at `span`: every concrete part is `Key`
/// ([`not_keyable`]) and every type parameter in it is declared `Key`
/// ([`require_key_vars`]). Reports and returns `false` otherwise.
pub(crate) fn check_key(ctx: &mut Ctx, ty: TyId, span: Span, site: &KeySite) -> bool {
    if let Some(blocker) = not_keyable(ty, ctx) {
        report_not_keyable(ctx, ty, span, site, blocker);
        return false;
    }
    let clause = match site {
        KeySite::MapKey => "a `Map` key must be `Key`".to_string(),
        KeySite::SetElement => "a `Set` element must be `Key`".to_string(),
        KeySite::Kernel(method) => format!("`{method}` keys by this type"),
        KeySite::Call { callee, param } => format!("`{callee}` requires `{param}: Key`"),
    };
    require_key_vars(ctx, ty, span, &clause)
}

/// The note every `not_keyable` diagnostic carries.
pub(crate) fn key_note(blocker: &NotKey) -> String {
    format!(
        "`{}` is not `Key`; key types are `String`, `Int`, `Bool`, `Instant`, `Duration`, `Bytes`, `Option` and `List` of a key type, refined and opaque types over them, and records, sums and enums made only of key types",
        blocker.part
    )
}

/// The tail of a `not_keyable` message when the blocker is a `Float`.
pub(crate) fn float_clause(blocker: &NotKey) -> &'static str {
    if blocker.float {
        " (a `Float` is never a key: `NaN` is not equal to itself)"
    } else {
        ""
    }
}

fn report_not_keyable(ctx: &mut Ctx, ty: TyId, span: Span, site: &KeySite, blocker: NotKey) {
    let shown = ty.display(ctx.tys);
    let message = match site {
        KeySite::MapKey => format!("a `Map` key must be `Key`, but `{shown}` is not"),
        KeySite::SetElement => format!("a `Set` element must be `Key`, but `{shown}` is not"),
        KeySite::Kernel(method) => {
            format!("`{method}` needs a `Key` element or key, but got `{shown}`")
        }
        KeySite::Call { callee, param } => {
            format!("`{callee}` requires `{param}: Key`, but this call makes `{param}` `{shown}`")
        }
    };
    let message = format!("{message}{}", float_clause(&blocker));
    let note = key_note(&blocker);
    ctx.errors
        .push(CompileError::new("bynk.types.not_keyable", span, message).with_note(note));
}

/// At a call, check each `Key`-bounded type parameter of the callee (`params`)
/// against its instantiation (`subst`): a `Key` type, or a type parameter the
/// caller declares `Key`. Returns `false` after reporting the first violation.
pub(crate) fn check_declared_key_args(
    callee: &str,
    call_span: Span,
    params: &[String],
    subst: &HashMap<String, TyId>,
    ctx: &mut Ctx,
) -> bool {
    for param in params {
        let Some(&arg_ty) = subst.get(param) else {
            continue;
        };
        if !check_key(ctx, arg_ty, call_span, &KeySite::Call { callee, param }) {
            return false;
        }
    }
    true
}

/// The type parameters of `f` declared `Key`: its own (inline or in `where`) and
/// its receiver type's.
pub fn key_params(
    f: &bynk_syntax::ast::FnDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Vec<String> {
    use super::equality::has_key;
    let where_key = |name: &str| {
        f.where_bounds
            .iter()
            .any(|wb| wb.param.name == name && has_key(&wb.bounds))
    };
    let mut out = Vec::new();
    if let bynk_syntax::ast::FnName::Method { type_name, .. } = &f.name
        && let Some(decl) = types.get(&type_name.name)
    {
        for tp in &decl.type_params {
            if has_key(&tp.bounds) || where_key(&tp.name.name) {
                out.push(tp.name.name.clone());
            }
        }
    }
    for tp in &f.type_params {
        if has_key(&tp.bounds) || where_key(&tp.name.name) {
            out.push(tp.name.name.clone());
        }
    }
    out
}

/// Walk `ty` for `Map` keys and `Set` members that mention a type parameter, and
/// require each to be declared `Key`. Used on a signature, where a bound is
/// declared rather than discovered (DECISION F): without it
/// `fn mk[K](k: K) -> Map[K, Int]` could key a map by a function.
pub(crate) fn require_key_positions(
    tys: &Types,
    type_vars: &HashSet<String>,
    eq: &EqScope,
    errors: &mut Vec<CompileError>,
    ty: TyId,
    span: Span,
) {
    let go = |t: TyId, errors: &mut Vec<CompileError>| {
        require_key_positions(tys, type_vars, eq, errors, t, span)
    };
    match &*tys.get(ty) {
        Ty::Map(k, v) => {
            require_key_vars_in(
                tys,
                type_vars,
                eq,
                errors,
                *k,
                span,
                "a `Map` key must be `Key`",
            );
            go(*k, errors);
            go(*v, errors);
        }
        Ty::Set(e) => {
            require_key_vars_in(
                tys,
                type_vars,
                eq,
                errors,
                *e,
                span,
                "a `Set` element must be `Key`",
            );
            go(*e, errors);
        }
        Ty::Option(a)
        | Ty::List(a)
        | Ty::Effect(a)
        | Ty::HttpResult(a)
        | Ty::HttpOutcome(a)
        | Ty::Query(a)
        | Ty::Stream(a)
        | Ty::Connection(a) => go(*a, errors),
        Ty::Result(a, b) => {
            go(*a, errors);
            go(*b, errors);
        }
        Ty::Fn { params, ret } => {
            for p in params {
                go(*p, errors);
            }
            go(*ret, errors);
        }
        Ty::Named { args, .. } => {
            for a in args {
                go(*a, errors);
            }
        }
        _ => {}
    }
}
