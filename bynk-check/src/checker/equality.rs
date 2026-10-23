//! Which types support `==`/`!=`. #1652 (runtime-semantics track #1648, slice
//! S3; decided in ADR 0421, the track's summary in
//! `design/archive/retired-tracks.md`).
//!
//! `==` is **structural**: two values are equal when they have the same shape and
//! their parts compare equal, recursively (type-system §2.3.5). That only means
//! something for types made of values. Some types are not values in that sense:
//!
//! - a function (`(A) -> B`), an `Effect[T]`, a `Query[T]`: a computation, not
//!   data (ADRs 0030/0031/0115);
//! - a `Stream[T]`: a live value-over-time source (real-time track slice 0);
//! - a held `Connection[F]`: it has identity, not value equality (§2.9.3).
//!
//! §2.3.5 makes the rule recursive: a type containing one of these **anywhere**
//! (inside an `Option`, a `List`, a `Map`, a record field, a sum payload, or a
//! generic argument) is not equality-supporting, and the compiler rejects
//! `==` on it. Before #1652 the checker only looked at the *top-level* type, so
//! `Option[Connection[F]]`, `List[(Int) -> Int]`, a record with an `Effect`
//! field, and `Box[(Int) -> Int]` were all accepted.
//!
//! Inside a generic function a type variable is equality-supporting, so `==`
//! on `T` there is accepted; the bound moves to the call site instead (#1688).
//! [`compared_type_params`] infers which of a generic function's type
//! parameters it compares — directly with `==`/`!=`, or by passing them to a
//! compared parameter of another generic function — and the call checks each
//! such argument with [`not_comparable`]. Bynk generics are not monomorphised,
//! so the instantiation is the only place the concrete type is known.
//!
//! One gap remains, and it is permissive (the runtime walker then compares the
//! offending part by identity): a record imported from another unit is absent
//! from `ctx.input.types`, so `walk_decl` cannot see its fields.
//!
//! Records and sums are walked through their declared field types
//! (`TypeRef`s), because the interned `Ty::Named` carries only the name and the
//! applied arguments. A declared field that names one of the type's own
//! parameters is covered by walking the corresponding applied argument. A
//! visited set keeps recursive types (`Node = { next: Option[Node] }`) finite.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

use bynk_syntax::ast::{CommonsItem, FnDecl, FnName, TypeBody, TypeRef};

use super::{Ctx, Ty, TyId, Types};
use crate::hints::HintSink;
use crate::index::RefSink;
use crate::locals::LocalsSink;
use crate::requirements::RequirementSink;
use crate::resolver::ResolvedCommons;

thread_local! {
    /// #1688: one frame per generic body being scanned by
    /// [`compared_type_params`]; `==` and compared call arguments add the type
    /// variables they mention to the innermost frame. Empty outside a scan, so
    /// ordinary checking records nothing.
    static RECORDING: RefCell<Vec<HashSet<String>>> = const { RefCell::new(Vec::new()) };
    /// #1688: each generic function's compared type parameters, keyed by the
    /// address of its shared declaration (`Arc<FnDecl>`, the same allocation in
    /// its own unit and in every importer) and valid for one project check
    /// (see [`reset_compared_cache`]). `None` marks a scan in progress, which a
    /// recursive call reads as "compares nothing yet".
    static COMPARED: RefCell<HashMap<usize, Option<Rc<HashSet<String>>>>> =
        RefCell::new(HashMap::new());
}

/// #1688: clear the cache of compared type parameters. Called at the start of
/// each project check (and of a single-file check), so a declaration address
/// never outlives the program it was computed for.
pub(crate) fn reset_compared_cache() {
    COMPARED.with(|c| c.borrow_mut().clear());
}

/// #1688: inside a [`compared_type_params`] scan, note that every type variable
/// in `ty` is compared. A no-op during ordinary checking.
pub(crate) fn record_compared(ty: TyId, tys: &Types) {
    RECORDING.with(|r| {
        if let Some(frame) = r.borrow_mut().last_mut() {
            collect_vars(ty, tys, frame);
        }
    });
}

fn collect_vars(ty: TyId, tys: &Types, out: &mut HashSet<String>) {
    match &*tys.get(ty) {
        Ty::Var(name) => {
            out.insert(name.clone());
        }
        Ty::Result(a, b) | Ty::Map(a, b) => {
            collect_vars(*a, tys, out);
            collect_vars(*b, tys, out);
        }
        Ty::Option(a)
        | Ty::Effect(a)
        | Ty::HttpResult(a)
        | Ty::HttpOutcome(a)
        | Ty::List(a)
        | Ty::Query(a)
        | Ty::Stream(a)
        | Ty::Connection(a) => collect_vars(*a, tys, out),
        Ty::Fn { params, ret } => {
            for p in params {
                collect_vars(*p, tys, out);
            }
            collect_vars(*ret, tys, out);
        }
        Ty::Named { args, .. } => {
            for a in args {
                collect_vars(*a, tys, out);
            }
        }
        Ty::Base(_)
        | Ty::Error
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::ValidationError
        | Ty::JsonError
        | Ty::Unit
        | Ty::Actor(_)
        | Ty::ActorSum(_) => {}
    }
}

/// #1688: the type parameters of generic function `decl` that it compares —
/// that reach an `==`/`!=` operand in its body, or a compared parameter of a
/// generic function it calls (transitively). A caller must instantiate each
/// with an equality-supporting type.
///
/// A function declared in the unit being checked is scanned here, on first
/// use, in this unit's environment. An imported one was scanned while its own
/// unit was checked ([`scan_local_generics`]; units are checked `uses`-first),
/// so it is only looked up: scanning it here would resolve its body against
/// the *caller's* names (#1702 review), missing what it calls through its own
/// `uses` and seeing local functions that shadow them. A miss reads as
/// "compares nothing", which can only accept.
pub(crate) fn compared_type_params(decl: &Arc<FnDecl>, ctx: &Ctx) -> Rc<HashSet<String>> {
    let local = ctx
        .input
        .commons
        .items
        .iter()
        .any(|i| matches!(i, CommonsItem::Fn(f) if same_fn(&f.name, &decl.name)));
    if local {
        compared_in(decl, ctx.input, ctx.tys)
    } else {
        cached(decl).flatten().unwrap_or_default()
    }
}

/// #1688: scan every generic function and method declared in `input`'s own
/// unit, so the units checked after it (its importers) can look the results up.
pub(crate) fn scan_local_generics(input: &ResolvedCommons, tys: &Types) {
    for item in &input.commons.items {
        let CommonsItem::Fn(f) = item else {
            continue;
        };
        let decl = match &f.name {
            FnName::Free(id) => input.fns.get(&id.name),
            FnName::Method {
                type_name,
                method_name,
            } => input.methods.get(&type_name.name).and_then(|t| {
                t.instance
                    .get(&method_name.name)
                    .or_else(|| t.statics.get(&method_name.name))
            }),
        };
        if let Some(decl) = decl {
            compared_in(decl, input, tys);
        }
    }
}

/// Whether two names denote the same declaration (a method's identity is its
/// type and method name together; [`FnName::display`] keeps only the latter).
fn same_fn(a: &FnName, b: &FnName) -> bool {
    match (a, b) {
        (FnName::Free(x), FnName::Free(y)) => x.name == y.name,
        (
            FnName::Method {
                type_name: t1,
                method_name: m1,
            },
            FnName::Method {
                type_name: t2,
                method_name: m2,
            },
        ) => t1.name == t2.name && m1.name == m2.name,
        _ => false,
    }
}

/// The type parameters a scan of `decl` tracks: its own, plus — for a method
/// on a generic type — the receiver type's, which its body sees as rigid
/// variables too (#1702 review: `fn Box.has(self, x: A)` compares `A`).
fn tracked_params(decl: &FnDecl, input: &ResolvedCommons) -> Vec<String> {
    let mut params: Vec<String> = decl
        .type_params
        .iter()
        .map(|tp| tp.name.name.clone())
        .collect();
    if let FnName::Method { type_name, .. } = &decl.name
        && let Some(t) = input.types.get(&type_name.name)
    {
        params.extend(t.type_params.iter().map(|tp| tp.name.name.clone()));
    }
    params
}

fn cached(decl: &Arc<FnDecl>) -> Option<Option<Rc<HashSet<String>>>> {
    let key = Arc::as_ptr(decl) as usize;
    COMPARED.with(|c| c.borrow().get(&key).cloned())
}

/// Check `decl`'s body again into throwaway sinks with a recording frame
/// pushed, in `input`'s environment, which must be `decl`'s own unit. Its
/// diagnostics were reported by the ordinary check, so they are discarded.
fn compared_in(decl: &Arc<FnDecl>, input: &ResolvedCommons, tys: &Types) -> Rc<HashSet<String>> {
    let tracked = tracked_params(decl, input);
    if tracked.is_empty() {
        return Rc::new(HashSet::new());
    }
    if let Some(entry) = cached(decl) {
        return entry.unwrap_or_default();
    }
    let key = Arc::as_ptr(decl) as usize;
    COMPARED.with(|c| c.borrow_mut().insert(key, None));
    RECORDING.with(|r| r.borrow_mut().push(HashSet::new()));
    super::calls::check_fn(
        decl,
        input,
        &mut HashMap::new(),
        &mut HashMap::new(),
        &mut Vec::new(),
        &mut RefSink::new(),
        &mut HintSink::new(),
        &mut LocalsSink::new(),
        &mut RequirementSink::new(),
        tys,
    );
    let seen = RECORDING.with(|r| r.borrow_mut().pop()).unwrap_or_default();
    let compared: HashSet<String> = tracked.into_iter().filter(|n| seen.contains(n)).collect();
    let compared = Rc::new(compared);
    COMPARED.with(|c| c.borrow_mut().insert(key, Some(Rc::clone(&compared))));
    compared
}

/// Why a type is not equality-supporting: the first offending part found.
pub(crate) enum NotComparable {
    /// A `Stream[T]` (`bynk.types.stream_not_comparable`).
    Stream,
    /// A held `Connection[F]` (`bynk.types.held_not_comparable`). The string is
    /// the held type's display form.
    Held(String),
    /// A function, `Effect` or `Query` (`bynk.types.not_comparable`). The string
    /// names the offending part for the diagnostic.
    Computation(String),
}

/// The first part of `ty` that has no value equality, or `None` when `ty` is
/// equality-supporting all the way down.
pub(crate) fn not_comparable(ty: TyId, ctx: &Ctx) -> Option<NotComparable> {
    let mut visited = HashSet::new();
    walk_ty(ty, ctx, &mut visited)
}

fn walk_ty(ty: TyId, ctx: &Ctx, visited: &mut HashSet<String>) -> Option<NotComparable> {
    let tys = ctx.tys;
    match &*tys.get(ty) {
        Ty::Stream(_) => Some(NotComparable::Stream),
        Ty::Connection(_) => Some(NotComparable::Held(tys.display(ty))),
        Ty::Fn { .. } | Ty::Effect(_) | Ty::Query(_) => {
            Some(NotComparable::Computation(tys.display(ty)))
        }
        Ty::Option(t) | Ty::List(t) | Ty::HttpResult(t) | Ty::HttpOutcome(t) => {
            walk_ty(*t, ctx, visited)
        }
        Ty::Result(a, b) | Ty::Map(a, b) => {
            walk_ty(*a, ctx, visited).or_else(|| walk_ty(*b, ctx, visited))
        }
        Ty::Named { name, args, .. } => {
            let args = args.clone();
            let name = name.clone();
            for a in &args {
                if let Some(found) = walk_ty(*a, ctx, visited) {
                    return Some(found);
                }
            }
            walk_decl(&name, ctx, visited)
        }
        // Base types, `()`, the built-in error and verdict types, actor
        // bindings, and type variables (`T` inside a generic body) compare as
        // values: base types by `===`, everything else by the structural
        // runtime walker.
        Ty::Error
        | Ty::Base(_)
        | Ty::QueueResult
        | Ty::BoundaryRejection
        | Ty::Asset
        | Ty::RequestHeaders
        | Ty::ValidationError
        | Ty::JsonError
        | Ty::Unit
        | Ty::Actor(_)
        | Ty::ActorSum(_)
        | Ty::Var(_) => None,
    }
}

/// Walk a named type's declared fields. Each name is walked once per check,
/// which keeps recursive types finite and makes the walk linear in the number of
/// types reachable from the operand.
fn walk_decl(name: &str, ctx: &Ctx, visited: &mut HashSet<String>) -> Option<NotComparable> {
    if !visited.insert(name.to_string()) {
        return None;
    }
    let decl = ctx.input.types.get(name)?;
    let params: Vec<&str> = decl
        .type_params
        .iter()
        .map(|p| p.name.name.as_str())
        .collect();
    let fields: Vec<&TypeRef> = match &decl.body {
        TypeBody::Record(r) => r.fields.iter().map(|f| &f.type_ref).collect(),
        TypeBody::Sum(s) => s
            .variants
            .iter()
            .flat_map(|v| v.payload.iter().map(|f| &f.type_ref))
            .collect(),
        TypeBody::Refined { .. } | TypeBody::Opaque { .. } => Vec::new(),
    };
    fields
        .into_iter()
        .find_map(|t| walk_ref(t, &params, ctx, visited))
}

fn walk_ref(
    t: &TypeRef,
    params: &[&str],
    ctx: &Ctx,
    visited: &mut HashSet<String>,
) -> Option<NotComparable> {
    let display = || render(t);
    match t {
        TypeRef::Stream(..) => Some(NotComparable::Stream),
        TypeRef::Connection(..) => Some(NotComparable::Held(display())),
        TypeRef::Fn(..) | TypeRef::Effect(..) | TypeRef::Query(..) => {
            Some(NotComparable::Computation(display()))
        }
        TypeRef::Option(a, _)
        | TypeRef::List(a, _)
        | TypeRef::HttpResult(a, _)
        | TypeRef::History(a, _) => walk_ref(a, params, ctx, visited),
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            walk_ref(a, params, ctx, visited).or_else(|| walk_ref(b, params, ctx, visited))
        }
        // A type parameter is checked through the applied argument at the use
        // site (`walk_ty`'s `Named` arm), not here.
        TypeRef::Named(id) if params.contains(&id.name.as_str()) => None,
        TypeRef::Named(id) => walk_decl(&id.name, ctx, visited),
        TypeRef::App { name, args, .. } => args
            .iter()
            .find_map(|a| walk_ref(a, params, ctx, visited))
            .or_else(|| walk_decl(&name.name, ctx, visited)),
        TypeRef::Base(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => None,
    }
}

/// Render a declared field type as the author wrote it, for the diagnostic.
/// (`context_checks::type_ref_to_display` covers only the shapes its own callers
/// meet; this walk needs every `TypeRef`.)
fn render(t: &TypeRef) -> String {
    let one = |name: &str, a: &TypeRef| format!("{name}[{}]", render(a));
    match t {
        TypeRef::Base(b, _) => b.name().to_string(),
        TypeRef::Named(id) => id.name.clone(),
        TypeRef::Result(a, b, _) => format!("Result[{}, {}]", render(a), render(b)),
        TypeRef::Map(a, b, _) => format!("Map[{}, {}]", render(a), render(b)),
        TypeRef::Option(a, _) => one("Option", a),
        TypeRef::Effect(a, _) => one("Effect", a),
        TypeRef::HttpResult(a, _) => one("HttpResult", a),
        TypeRef::List(a, _) => one("List", a),
        TypeRef::Query(a, _) => one("Query", a),
        TypeRef::Stream(a, _) => one("Stream", a),
        TypeRef::Connection(a, _) => one("Connection", a),
        TypeRef::History(a, _) => one("History", a),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::BoundaryRejection(_) => "BoundaryRejection".to_string(),
        TypeRef::ValidationError(_) => "ValidationError".to_string(),
        TypeRef::JsonError(_) => "JsonError".to_string(),
        TypeRef::Unit(_) => "()".to_string(),
        TypeRef::Fn(params, ret, _) => {
            let ps: Vec<String> = params.iter().map(render).collect();
            format!("({}) -> {}", ps.join(", "), render(ret))
        }
        TypeRef::App { name, args, .. } => {
            let a: Vec<String> = args.iter().map(render).collect();
            format!("{}[{}]", name.name, a.join(", "))
        }
    }
}

/// #1688: at a call to a generic function or method, check that every type
/// parameter the callee compares (`compared`) is instantiated (`subst`) with an
/// equality-supporting type. `params` lists the callee's parameters in report
/// order with their declaration spans; `callee` names it in the message. Also
/// records each compared argument, so a scan of a generic caller sees the bound
/// pass through. Returns `false` after reporting the first violation.
pub(crate) fn check_compared_args(
    callee: &str,
    call_span: bynk_syntax::span::Span,
    params: &[(String, Option<bynk_syntax::span::Span>)],
    compared: &HashSet<String>,
    subst: &HashMap<String, TyId>,
    ctx: &mut Ctx,
) -> bool {
    let tys = ctx.tys;
    for (param, decl_span) in params {
        if !compared.contains(param) {
            continue;
        }
        let Some(&arg_ty) = subst.get(param) else {
            continue;
        };
        record_compared(arg_ty, tys);
        let Some(blocker) = not_comparable(arg_ty, ctx) else {
            continue;
        };
        let shown = arg_ty.display(tys);
        let top = matches!(
            &*tys.get(arg_ty),
            Ty::Stream(_) | Ty::Connection(_) | Ty::Fn { .. } | Ty::Effect(_) | Ty::Query(_)
        );
        let it = if top { "it is" } else { "it contains" };
        let (code, why) = match blocker {
            NotComparable::Stream => (
                "bynk.types.stream_not_comparable",
                format!("{it} a `Stream`, a live value-over-time source, not a comparable value"),
            ),
            NotComparable::Held(held) => (
                "bynk.types.held_not_comparable",
                format!("{it} a held `{held}`, which has identity, not value-equality"),
            ),
            NotComparable::Computation(part) if top => (
                "bynk.types.not_comparable",
                format!(
                    "a function, `Effect` or `Query` like `{part}` is a computation, which has no value equality"
                ),
            ),
            NotComparable::Computation(part) => (
                "bynk.types.not_comparable",
                format!("it contains `{part}`, which has no value equality"),
            ),
        };
        let mut err = bynk_syntax::error::CompileError::new(
            code,
            call_span,
            format!(
                "`{callee}` compares values of its type parameter `{param}`, but this call makes `{param}` `{shown}` — {why}"
            ),
        );
        // A label renders against the caller's file, so point at the parameter
        // only when the callee is declared in that same file.
        if let Some(span) = decl_span
            && span.file == call_span.file
        {
            err = err.with_label(*span, "compared inside the function");
        }
        ctx.errors.push(err);
        return false;
    }
    true
}
