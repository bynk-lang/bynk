//! Which types are `Codable`, and the checks that need it. Constrained generics
//! slice 3 (#2715; the model is the track's Q4/Q7 in
//! `design/tracks/constrained-generics.md`).
//!
//! `Codable` is automatic, never derived, and its meaning is one predicate: the
//! domain `Json.decode` accepts , which `Json.encode`,
//! `Json.decode` and a `Codable` bound all share so they cannot drift. A type is
//! `Codable` when it is:
//!
//! - a base type (including `Float` and `Bytes`), or a refined or opaque type;
//! - `Option`, `List`, `Set`, `Map` or `Result` of `Codable` types;
//! - a record, sum or enum, or a generic instantiation whose arguments are
//!   `Codable`;
//! - a type parameter declared `T: Codable` (checked separately: see
//!   `super::equality::require_codable_vars`).
//!
//! Not `Codable`: top-level `()` (it encodes but does not decode), functions,
//! `Effect`, `Query`, `Stream` and anything that carries one, `Connection`, the
//! HTTP and queue result types, `Asset`, request headers, token claims,
//! statements, actors, a recursive generic record, and an unbounded type
//! parameter. The declaration-position rule (`context_checks::reject_fn_types`)
//! stays separate: its answer depends on the position.
//!
//! A `Codable` type parameter gets a hidden codec witness (`__codec_T`), ordered
//! with the comparator witnesses by [`witness_list`].

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bynk_syntax::ast::{FnDecl, FnName, TypeDecl};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use super::equality::{has_codable, has_ord, require_codable_vars};
use super::ordering::{OrdSite, check_ord};
use super::{Ctx, Ty, TyId, Types};

/// The domain a `Json` codec covers. A type variable counts when it is in `vars`
/// (the type parameters in scope); whether it declared `Codable` is
/// `require_codable_vars`'s question.
pub(crate) fn json_codable(
    t: TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
    vars: &HashSet<String>,
) -> bool {
    match &*tys.get(t) {
        // R4.3: an already-diagnosed type doesn't also earn a
        // `bynk.types.json_uncodable` diagnostic — the resolution failure was
        // reported once, at its own site.
        Ty::Error | Ty::Never => true,
        // K-streams (#2623): a record or sum carrying a `Stream` has no codec.
        Ty::Named { name, .. } if bynk_syntax::ast::type_carries_stream(name, types) => false,
        // v0.174 (#592): a generic record instantiation is boundary-serialisable
        // through its monomorphised codec exactly when its type arguments are.
        Ty::Named { args, .. } if !args.is_empty() => {
            args.iter().all(|a| json_codable(*a, types, tys, vars))
        }
        Ty::Base(_) | Ty::Named { .. } | Ty::Unit => true,
        Ty::Result(a, b) => {
            json_codable(*a, types, tys, vars) && json_codable(*b, types, tys, vars)
        }
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => json_codable(*a, types, tys, vars),
        Ty::Map(k, v) => json_codable(*k, types, tys, vars) && json_codable(*v, types, tys, vars),
        Ty::Var(name) => vars.contains(name),
        Ty::Fn { .. }
        | Ty::Effect(_)
        | Ty::Query(_)
        | Ty::Stream(_)
        | Ty::Connection(_)
        | Ty::HttpResult(_)
        | Ty::HttpOutcome(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::TokenClaims
        | Ty::ValidationError
        | Ty::JsonError
        | Ty::Actor(_)
        | Ty::ActorSum(_) => false,
    }
}

/// The first user generic type applied to a type variable in `t`, as written.
/// Its codec would need a factory per generic declaration, which is deferred
/// (`bynk.generics.codec_generic_at_rigid`).
fn generic_at_rigid(t: TyId, vars: &HashSet<String>, tys: &Types) -> Option<String> {
    match &*tys.get(t) {
        Ty::Named { args, .. } if !args.is_empty() => {
            let mut inner = HashSet::new();
            for a in args {
                super::equality::collect_vars(*a, tys, &mut inner);
            }
            if inner.iter().any(|v| vars.contains(v)) {
                return Some(tys.display(t));
            }
            args.iter().find_map(|a| generic_at_rigid(*a, vars, tys))
        }
        Ty::Result(a, b) | Ty::Map(a, b) => {
            generic_at_rigid(*a, vars, tys).or_else(|| generic_at_rigid(*b, vars, tys))
        }
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => generic_at_rigid(*a, vars, tys),
        _ => None,
    }
}

/// The variable-dependent half of a `Codable` requirement on `ty`: no user
/// generic type applied to a type parameter, and every type parameter in it
/// declared `Codable`. Reports and returns `false` otherwise.
pub(crate) fn require_codable_rigid(ctx: &mut Ctx, ty: TyId, span: Span, clause: &str) -> bool {
    if let Some(shown) = generic_at_rigid(ty, &ctx.type_vars, ctx.tys) {
        ctx.errors.push(
            CompileError::new(
                "bynk.generics.codec_generic_at_rigid",
                span,
                format!("{clause}, but `{shown}` is a generic type applied to a type parameter"),
            )
            .with_note(
                "a codec for a user generic type over a type parameter is not supported yet; \
                 use a built-in container (`List`, `Option`, `Set`, `Map`, `Result`) of the \
                 parameter, or a concrete instantiation",
            ),
        );
        return false;
    }
    require_codable_vars(ctx, ty, span, clause)
}

/// What a use of `Codable` is for, which decides the diagnostic's wording.
pub(crate) enum CodableSite<'a> {
    /// A call to a `Codable`-bounded callee: `callee` requires `param: Codable`.
    Call { callee: &'a str, param: &'a str },
    /// A call to a capability operation whose `param` is `Codable`.
    Op { op: &'a str, param: &'a str },
}

/// Check that `ty` can be a `Codable` instantiation at `span`: it is in the
/// codec domain, top-level `()` excepted, it names no recursive generic or
/// statement, and every type parameter in it is declared `Codable`.
pub(crate) fn check_codable(ctx: &mut Ctx, ty: TyId, span: Span, site: &CodableSite) -> bool {
    let tys = ctx.tys;
    let (what, param) = match site {
        CodableSite::Call { callee, param } => (format!("`{callee}`"), *param),
        CodableSite::Op { op, param } => (format!("capability operation `{op}`"), *param),
    };
    let clause = format!("{what} requires `{param}: Codable`");
    if let Some(rec) = super::kernels::first_recursive_generic(ty, &ctx.input.types, tys) {
        ctx.errors
            .push(super::kernels::recursive_generic_error(&rec, span));
        return false;
    }
    if super::d1::carries_statement(ty, &ctx.input.types, tys) {
        ctx.errors
            .push(super::d1::statement_at_boundary(span, "a `Codable` type"));
        return false;
    }
    if !json_codable(ty, &ctx.input.types, tys, &ctx.type_vars) || ty == tys.intern(Ty::Unit) {
        let shown = tys.display(ty);
        ctx.errors.push(
            CompileError::new(
                "bynk.types.not_codable",
                span,
                format!(
                    "{clause}, but this call makes `{param}` `{shown}`, which is not `Codable`"
                ),
            )
            .with_note(
                "`Codable` types are the ones `Json.decode` accepts: base types, named types, \
                 and the built-in containers over them — not `()`, functions, effects, streams \
                 (or a type that carries one), or error builtins",
            ),
        );
        return false;
    }
    require_codable_rigid(ctx, ty, span, &clause)
}

/// Which hidden witness a type parameter takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WitnessKind {
    /// A comparator, for `T: Ord` (`__cmp_T`).
    Cmp,
    /// A codec, for `T: Codable` (`__codec_T`).
    Codec,
}

/// One hidden trailing parameter of a bounded generic function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessParam {
    /// The type parameter's name.
    pub param: String,
    pub kind: WitnessKind,
}

/// The hidden witness parameters of `f`, in order: the receiver type's
/// parameters first, then the function's own, each in declaration order; within
/// one parameter the comparator comes before the codec (the order of
/// `CONSTRAINTS`). A bound is declared inline (`T: Ord`), in a `where` entry, or,
/// for a receiver parameter, on the type. The emitter adds one trailing parameter
/// per entry and every call site passes one argument per entry, so this order is
/// the contract between them across units; it is not part of the published ABI.
pub fn witness_list(f: &FnDecl, types: &HashMap<String, Arc<TypeDecl>>) -> Vec<WitnessParam> {
    let where_has = |name: &str, has: fn(&[bynk_syntax::ast::Ident]) -> bool| {
        f.where_bounds
            .iter()
            .any(|wb| wb.param.name == name && has(&wb.bounds))
    };
    let mut out = Vec::new();
    let mut push = |name: &str, bounds: &[bynk_syntax::ast::Ident]| {
        if has_ord(bounds) || where_has(name, has_ord) {
            out.push(WitnessParam {
                param: name.to_string(),
                kind: WitnessKind::Cmp,
            });
        }
        if has_codable(bounds) || where_has(name, has_codable) {
            out.push(WitnessParam {
                param: name.to_string(),
                kind: WitnessKind::Codec,
            });
        }
    };
    if let FnName::Method { type_name, .. } = &f.name
        && let Some(decl) = types.get(&type_name.name)
    {
        for tp in &decl.type_params {
            push(&tp.name.name, &tp.bounds);
        }
    }
    for tp in &f.type_params {
        push(&tp.name.name, &tp.bounds);
    }
    out
}

/// At a call, check each witness-bearing type parameter of the callee (`params`,
/// in witness order) against its instantiation (`subst`) and collect the
/// instantiations: the call's witnesses. `None` after reporting the first
/// violation.
pub(crate) fn check_declared_witness_args(
    callee: &str,
    call_span: Span,
    params: &[WitnessParam],
    subst: &HashMap<String, TyId>,
    ctx: &mut Ctx,
) -> Option<Vec<TyId>> {
    let mut out = Vec::new();
    for w in params {
        let &arg_ty = subst.get(&w.param)?;
        let param = w.param.as_str();
        let ok = match w.kind {
            WitnessKind::Cmp => check_ord(ctx, arg_ty, call_span, &OrdSite::Call { callee, param }),
            WitnessKind::Codec => {
                check_codable(ctx, arg_ty, call_span, &CodableSite::Call { callee, param })
            }
        };
        if !ok {
            return None;
        }
        out.push(arg_ty);
    }
    Some(out)
}

// -- capability operations --

/// Whether `t` mentions any of `names`, as a bare name or a generic argument.
pub fn mentions_any(t: &bynk_syntax::ast::TypeRef, names: &[String]) -> bool {
    if names.is_empty() {
        return false;
    }
    let mut found = Vec::new();
    crate::project_model::type_ref_names(t, &HashSet::new(), &mut found);
    found.iter().any(|n| names.contains(n))
}

/// The type parameters of a capability operation declared `Codable`.
pub fn codable_op_params(op: &bynk_syntax::ast::CapabilityOp) -> Vec<String> {
    op.type_params
        .iter()
        .filter(|tp| has_codable(&tp.bounds))
        .map(|tp| tp.name.name.clone())
        .collect()
}

/// How a call to a capability operation with `Codable` parameters is lowered:
/// the provider receives plain JSON, never a witness (DECISION D).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapEncoding {
    /// Per argument: the instantiated parameter type to encode whole with its
    /// codec, or `None` for an argument that passes through unchanged. An
    /// argument is encoded when its declared type mentions a `Codable` type
    /// parameter.
    pub encode: Vec<Option<TyId>>,
    /// Per type parameter: whether the emitted interface still mentions it. A
    /// parameter that only occurs in encoded arguments is dropped from the
    /// interface (those arguments are `unknown`), and so from a call's type
    /// arguments.
    pub keep_type_args: Vec<bool>,
}

/// The encoding of a call to an operation, given its pattern parameter types
/// (`params`), return type and the call's `subst`. `None` when the operation has
/// no `Codable` parameter, so every other call lowers as it always did.
pub(crate) fn op_encoding(
    type_params: &[String],
    codable: &[String],
    params: &[TyId],
    ret: TyId,
    subst: &HashMap<String, TyId>,
    tys: &Types,
) -> Option<Arc<CapEncoding>> {
    if codable.is_empty() {
        return None;
    }
    let mentions_codable = |t: TyId| {
        let mut vars = HashSet::new();
        super::equality::collect_vars(t, tys, &mut vars);
        vars.iter().any(|v| codable.contains(v))
    };
    let encode: Vec<Option<TyId>> = params
        .iter()
        .map(|p| mentions_codable(*p).then(|| super::substitute(*p, subst, tys)))
        .collect();
    let mut kept = HashSet::new();
    for (p, e) in params.iter().zip(&encode) {
        if e.is_none() {
            super::equality::collect_vars(*p, tys, &mut kept);
        }
    }
    super::equality::collect_vars(ret, tys, &mut kept);
    Some(Arc::new(CapEncoding {
        encode,
        keep_type_args: type_params.iter().map(|tp| kept.contains(tp)).collect(),
    }))
}

/// At a call to a capability operation, check each `Codable` type parameter
/// against its instantiation. Reports and returns `false` on the first failure.
pub(crate) fn check_op_codable_args(
    op: &str,
    span: Span,
    codable: &[String],
    subst: &HashMap<String, TyId>,
    ctx: &mut Ctx,
) -> bool {
    for param in codable {
        let Some(&ty) = subst.get(param) else {
            continue;
        };
        if !check_codable(ctx, ty, span, &CodableSite::Op { op, param }) {
            return false;
        }
    }
    true
}
