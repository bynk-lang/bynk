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
//! Inside a generic function a type variable is equality-supporting only when
//! its declaration says so: `T: Eq` on the function, or `where A: Eq` for a
//! method's receiver parameter (constrained-generics slice 0, which replaced
//! ADR 0430's inferred bound with a declared one). `==` on a type that mentions
//! an undeclared type parameter is `bynk.generics.missing_eq_bound`
//! ([`require_eq`]), and so is passing one to an `Eq`-bounded parameter of
//! another generic. At a call, [`check_declared_eq_args`] checks each
//! `Eq`-bounded type parameter's instantiation with [`not_comparable`]. Bynk
//! generics are not monomorphised, so the instantiation is the only place the
//! concrete type is known.
//!
//! ADR 0430's inference is kept for one purpose: [`compared_type_params`] still
//! computes which type parameters a generic compares — directly with
//! `==`/`!=`, or by passing them to a compared parameter of another generic,
//! across units — so that a call to a generic that compares a parameter it has
//! not declared yet is held to it too. One pass then reports every link of a
//! chain that needs the bound, each with its fix, and a single fix-all migrates
//! the lot; without it each round of fixes would expose the next caller. It
//! never decides acceptance: a program it flags already has the
//! `missing_eq_bound` in the callee's own body.
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

use bynk_syntax::ast::{CommonsItem, FnDecl, FnName, Ident, TypeBody, TypeDecl, TypeRef};
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

use super::{Ctx, Ty, TyId, Types};
use crate::hints::HintSink;
use crate::index::RefSink;
use crate::locals::LocalsSink;
use crate::requirements::RequirementSink;
use crate::resolver::ResolvedCommons;

/// The constraint names a bound may use. Slice 0 admitted only `Eq`; slice 1
/// added `Ord`, slice 2 `Key`, and slice 3 `Codable`. The order is the witness
/// order within one type parameter (comparator before codec).
pub(crate) const CONSTRAINTS: &[&str] = &["Eq", "Ord", "Key", "Codable"];

/// Whether `bounds` includes `Ord`.
pub(crate) fn has_ord(bounds: &[Ident]) -> bool {
    bounds.iter().any(|b| b.name == "Ord")
}

/// Whether `bounds` includes `Key`.
pub(crate) fn has_key(bounds: &[Ident]) -> bool {
    bounds.iter().any(|b| b.name == "Key")
}

/// Whether `bounds` includes `Codable`.
pub(crate) fn has_codable(bounds: &[Ident]) -> bool {
    bounds.iter().any(|b| b.name == "Codable")
}

/// Whether `bounds` grants `Eq`: an explicit `Eq`, or `Ord` or `Key`, which imply
/// it (constrained-generics slice 1 DECISION C; slice 2 DECISION G).
pub(crate) fn has_eq(bounds: &[Ident]) -> bool {
    bounds
        .iter()
        .any(|b| b.name == "Eq" || b.name == "Ord" || b.name == "Key")
}

/// Every bound name in `bounds` that is not a known constraint, as an
/// `unknown_constraint` error at its own span.
pub(crate) fn unknown_constraints(bounds: &[Ident], param: &str) -> Vec<CompileError> {
    bounds
        .iter()
        .filter(|b| !CONSTRAINTS.contains(&b.name.as_str()))
        .map(|b| {
            CompileError::new(
                "bynk.generics.unknown_constraint",
                b.span,
                format!(
                    "`{}` is not a constraint (type parameter `{param}`)",
                    b.name
                ),
            )
            .with_note(format!(
                "the constraints are `{}`",
                CONSTRAINTS.join("`, `")
            ))
        })
        .collect()
}

/// `T: Eq + Ord` or `T: Eq + Key`: `Ord` and `Key` imply `Eq`, so the `Eq` is
/// redundant. A warning whose fix removes it.
pub(crate) fn redundant_eq(bounds: &[Ident], param: &str) -> Option<CompileError> {
    let implied_by = if has_ord(bounds) {
        "Ord"
    } else if has_key(bounds) {
        "Key"
    } else {
        return None;
    };
    let i = bounds.iter().position(|b| b.name == "Eq")?;
    let eq = &bounds[i];
    let remove = if i + 1 < bounds.len() {
        Span::new_in(eq.span.file, eq.span.start, bounds[i + 1].span.start)
    } else {
        Span::new_in(eq.span.file, bounds[i - 1].span.end, eq.span.end)
    };
    Some(
        CompileError::new(
            "bynk.generics.redundant_eq_bound",
            eq.span,
            format!("`{param}: Eq` is redundant: `{implied_by}` implies `Eq`"),
        )
        .with_suggestion(
            "remove `Eq`",
            vec![(remove, String::new())],
            Applicability::MachineApplicable,
        ),
    )
}

/// Where a `missing_eq_bound` / `missing_ord_bound` fix edits the bounds, for one
/// function.
#[derive(Default, Clone)]
struct FixSite {
    /// An own type parameter: the end of its name, or of its last bound.
    own: HashMap<String, (Span, bool)>,
    /// A receiver parameter already named in the `where` clause: the end of its
    /// entry.
    where_entry: HashMap<String, Span>,
    /// The span of an explicit `Eq` bound on a parameter (own or in a `where`
    /// entry), which a missing `Ord` replaces (DECISION C: `Ord` implies `Eq`).
    eq_bound: HashMap<String, Span>,
    /// The end of the last `where` entry, when there is a clause.
    where_end: Option<Span>,
    /// The end of the return type, where a new clause goes.
    ret_end: Option<Span>,
}

/// The constraint bounds in scope while a body is checked: which type parameters
/// are declared `Eq` (or `Ord`, which implies it) and which `Ord`, and where a
/// missing bound would be added. Empty outside a generic function.
#[derive(Default, Clone)]
pub(crate) struct EqScope {
    /// Parameters that support `==`: declared `Eq` or `Ord`.
    declared: HashSet<String>,
    /// Parameters declared `Ord`.
    ord: HashSet<String>,
    /// Parameters declared `Key`.
    key: HashSet<String>,
    /// Parameters declared `Codable`.
    codable: HashSet<String>,
    /// Shared by every clone (a function's contract clauses and its body check
    /// in separate contexts), so a parameter is reported once per function.
    reported: Rc<RefCell<HashSet<String>>>,
    reported_ord: Rc<RefCell<HashSet<String>>>,
    reported_key: Rc<RefCell<HashSet<String>>>,
    reported_codable: Rc<RefCell<HashSet<String>>>,
    fix: FixSite,
}

impl EqScope {
    /// The scope of `f`'s body: its own `T: Eq` / `T: Ord` bounds, `where A: C`
    /// entries, and the bounds its receiver type declares on its own parameters.
    pub(crate) fn for_fn(f: &FnDecl, input: &ResolvedCommons) -> Self {
        let mut declared = HashSet::new();
        let mut ord = HashSet::new();
        let mut key = HashSet::new();
        let mut codable = HashSet::new();
        let mut fix = FixSite::default();
        for tp in &f.type_params {
            if has_eq(&tp.bounds) {
                declared.insert(tp.name.name.clone());
            }
            if has_ord(&tp.bounds) {
                ord.insert(tp.name.name.clone());
            }
            if has_key(&tp.bounds) {
                key.insert(tp.name.name.clone());
            }
            if has_codable(&tp.bounds) {
                codable.insert(tp.name.name.clone());
            }
            if let Some(eq) = tp.bounds.iter().find(|b| b.name == "Eq") {
                fix.eq_bound.insert(tp.name.name.clone(), eq.span);
            }
            let end = tp.bounds.last().map_or(tp.name.span, |b| b.span);
            fix.own.insert(
                tp.name.name.clone(),
                (
                    Span::new_in(end.file, end.end, end.end),
                    !tp.bounds.is_empty(),
                ),
            );
        }
        for wb in &f.where_bounds {
            if has_eq(&wb.bounds) {
                declared.insert(wb.param.name.clone());
            }
            if has_ord(&wb.bounds) {
                ord.insert(wb.param.name.clone());
            }
            if has_key(&wb.bounds) {
                key.insert(wb.param.name.clone());
            }
            if has_codable(&wb.bounds) {
                codable.insert(wb.param.name.clone());
            }
            if let Some(eq) = wb.bounds.iter().find(|b| b.name == "Eq") {
                fix.eq_bound.insert(wb.param.name.clone(), eq.span);
            }
            let end = wb.span;
            fix.where_entry.insert(
                wb.param.name.clone(),
                Span::new_in(end.file, end.end, end.end),
            );
        }
        if let Some(last) = f.where_bounds.last() {
            fix.where_end = Some(Span::new_in(last.span.file, last.span.end, last.span.end));
        }
        let ret = f.return_type.span();
        fix.ret_end = Some(Span::new_in(ret.file, ret.end, ret.end));
        if let FnName::Method { type_name, .. } = &f.name
            && let Some(decl) = input.types.get(&type_name.name)
        {
            for tp in &decl.type_params {
                if has_eq(&tp.bounds) {
                    declared.insert(tp.name.name.clone());
                }
                if has_ord(&tp.bounds) {
                    ord.insert(tp.name.name.clone());
                }
                if has_key(&tp.bounds) {
                    key.insert(tp.name.name.clone());
                }
                if has_codable(&tp.bounds) {
                    codable.insert(tp.name.name.clone());
                }
            }
        }
        EqScope {
            declared,
            ord,
            key,
            codable,
            reported: Rc::default(),
            reported_ord: Rc::default(),
            reported_key: Rc::default(),
            reported_codable: Rc::default(),
            fix,
        }
    }

    /// The edit that declares `constraint` (`Eq`, `Ord` or `Key`) on `param`. A
    /// missing `Ord` or `Key` on a parameter already declared `Eq` replaces the
    /// `Eq`, which both imply.
    pub(crate) fn fix_for(
        &self,
        param: &str,
        constraint: &str,
    ) -> Option<(String, Vec<(Span, String)>)> {
        let msg = format!("declare `{param}: {constraint}`");
        // `Ord` and `Key` imply `Eq`, so they replace an explicit `Eq`; `Codable`
        // does not, so it is added beside it.
        if matches!(constraint, "Ord" | "Key")
            && let Some(at) = self.fix.eq_bound.get(param)
        {
            return Some((msg, vec![(*at, constraint.to_string())]));
        }
        if let Some((at, bounded)) = self.fix.own.get(param) {
            let text = if *bounded {
                format!(" + {constraint}")
            } else {
                format!(": {constraint}")
            };
            return Some((msg, vec![(*at, text)]));
        }
        if let Some(at) = self.fix.where_entry.get(param) {
            return Some((msg, vec![(*at, format!(" + {constraint}"))]));
        }
        if let Some(at) = self.fix.where_end {
            return Some((msg, vec![(at, format!(", {param}: {constraint}"))]));
        }
        let at = self.fix.ret_end?;
        Some((msg, vec![(at, format!(" where {param}: {constraint}"))]))
    }
}

/// Require `Eq` of every type parameter in `ty` that the enclosing function
/// has not declared it for (`ctx.eq`). `site` says why, as a clause (`` `==`
/// compares values ``). Reports `bynk.generics.missing_eq_bound`, once per
/// parameter per function, and returns `false` when any was missing.
pub(crate) fn require_eq(ctx: &mut Ctx, ty: TyId, span: Span, site: &str) -> bool {
    let mut vars = HashSet::new();
    collect_vars(ty, ctx.tys, &mut vars);
    let mut missing: Vec<String> = vars
        .into_iter()
        .filter(|v| ctx.type_vars.contains(v) && !ctx.eq.declared.contains(v))
        .collect();
    if missing.is_empty() {
        return true;
    }
    missing.sort();
    for param in missing {
        if !ctx.eq.reported.borrow_mut().insert(param.clone()) {
            continue;
        }
        let mut err = CompileError::new(
            "bynk.generics.missing_eq_bound",
            span,
            format!("{site}, but type parameter `{param}` has no `Eq` bound"),
        )
        .with_note(format!(
            "a type parameter that is compared must say so: declare `{param}: Eq`"
        ));
        if let Some((msg, edits)) = ctx.eq.fix_for(&param, "Eq") {
            err = err.with_suggestion(msg, edits, Applicability::MachineApplicable);
        }
        ctx.errors.push(err);
    }
    false
}

/// Require `Ord` of every type parameter in `ty` that the enclosing function has
/// not declared it for. Reports `bynk.generics.missing_ord_bound`, once per
/// parameter per function, and returns `false` when any was missing. The
/// structural half of the requirement (is the concrete part orderable?) is
/// [`super::ordering::check_ord`].
pub(crate) fn require_ord_vars(ctx: &mut Ctx, ty: TyId, span: Span, site: &str) -> bool {
    let mut vars = HashSet::new();
    collect_vars(ty, ctx.tys, &mut vars);
    let mut missing: Vec<String> = vars
        .into_iter()
        .filter(|v| ctx.type_vars.contains(v) && !ctx.eq.ord.contains(v))
        .collect();
    if missing.is_empty() {
        return true;
    }
    missing.sort();
    for param in missing {
        if !ctx.eq.reported_ord.borrow_mut().insert(param.clone()) {
            continue;
        }
        let mut err = CompileError::new(
            "bynk.generics.missing_ord_bound",
            span,
            format!("{site}, but type parameter `{param}` has no `Ord` bound"),
        )
        .with_note(format!(
            "a type parameter that is ordered must say so: declare `{param}: Ord`"
        ));
        if let Some((msg, edits)) = ctx.eq.fix_for(&param, "Ord") {
            err = err.with_suggestion(msg, edits, Applicability::MachineApplicable);
        }
        ctx.errors.push(err);
    }
    false
}

/// Require `Key` of every type parameter in `ty` that the enclosing function has
/// not declared it for. Reports `bynk.generics.missing_key_bound`, once per
/// parameter per function, and returns `false` when any was missing. The
/// structural half of the requirement (is the concrete part a key?) is
/// [`super::keying::check_key`].
pub(crate) fn require_key_vars(ctx: &mut Ctx, ty: TyId, span: Span, site: &str) -> bool {
    require_key_vars_in(ctx.tys, &ctx.type_vars, &ctx.eq, ctx.errors, ty, span, site)
}

/// [`require_key_vars`] over the pieces of a `Ctx` it needs, so a signature can
/// be checked before any body context exists.
pub(crate) fn require_key_vars_in(
    tys: &Types,
    type_vars: &HashSet<String>,
    eq: &EqScope,
    errors: &mut Vec<CompileError>,
    ty: TyId,
    span: Span,
    site: &str,
) -> bool {
    let mut vars = HashSet::new();
    collect_vars(ty, tys, &mut vars);
    let mut missing: Vec<String> = vars
        .into_iter()
        .filter(|v| type_vars.contains(v) && !eq.key.contains(v))
        .collect();
    if missing.is_empty() {
        return true;
    }
    missing.sort();
    for param in missing {
        if !eq.reported_key.borrow_mut().insert(param.clone()) {
            continue;
        }
        let mut err = CompileError::new(
            "bynk.generics.missing_key_bound",
            span,
            format!("{site}, but type parameter `{param}` has no `Key` bound"),
        )
        .with_note(format!(
            "a type parameter that keys a `Map` or `Set` must say so: declare `{param}: Key`"
        ));
        if let Some((msg, edits)) = eq.fix_for(&param, "Key") {
            err = err.with_suggestion(msg, edits, Applicability::MachineApplicable);
        }
        errors.push(err);
    }
    false
}

/// Require `Codable` of every type parameter in `ty` that the enclosing function
/// has not declared it for. Reports `bynk.generics.missing_codable_bound`, once
/// per parameter per function, and returns `false` when any was missing. The
/// structural half of the requirement is [`super::codable::check_codable`].
pub(crate) fn require_codable_vars(ctx: &mut Ctx, ty: TyId, span: Span, site: &str) -> bool {
    let mut vars = HashSet::new();
    collect_vars(ty, ctx.tys, &mut vars);
    let mut missing: Vec<String> = vars
        .into_iter()
        .filter(|v| ctx.type_vars.contains(v) && !ctx.eq.codable.contains(v))
        .collect();
    if missing.is_empty() {
        return true;
    }
    missing.sort();
    for param in missing {
        if !ctx.eq.reported_codable.borrow_mut().insert(param.clone()) {
            continue;
        }
        let mut err = CompileError::new(
            "bynk.generics.missing_codable_bound",
            span,
            format!("{site}, but type parameter `{param}` has no `Codable` bound"),
        )
        .with_note(format!(
            "a type parameter that crosses the boundary must say so: declare `{param}: Codable`"
        ));
        if let Some((msg, edits)) = ctx.eq.fix_for(&param, "Codable") {
            err = err.with_suggestion(msg, edits, Applicability::MachineApplicable);
        }
        ctx.errors.push(err);
    }
    false
}

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

pub(crate) fn collect_vars(ty: TyId, tys: &Types, out: &mut HashSet<String>) {
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
        | Ty::Set(a)
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
        | Ty::TokenClaims
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
    let types = &ctx.input.types;
    match &*tys.get(ty) {
        Ty::Stream(_) => Some(NotComparable::Stream),
        Ty::Connection(_) => Some(NotComparable::Held(tys.display(ty))),
        Ty::Fn { .. } | Ty::Effect(_) | Ty::Query(_) => {
            Some(NotComparable::Computation(tys.display(ty)))
        }
        Ty::Option(t) | Ty::List(t) | Ty::Set(t) | Ty::HttpResult(t) | Ty::HttpOutcome(t) => {
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
            walk_decl(&name, types, visited)
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
        | Ty::TokenClaims
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
pub(crate) fn walk_decl(
    name: &str,
    types: &HashMap<String, Arc<TypeDecl>>,
    visited: &mut HashSet<String>,
) -> Option<NotComparable> {
    if !visited.insert(name.to_string()) {
        return None;
    }
    let decl = types.get(name)?;
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
        .find_map(|t| walk_ref(t, &params, types, visited))
}

pub(crate) fn walk_ref(
    t: &TypeRef,
    params: &[&str],
    types: &HashMap<String, Arc<TypeDecl>>,
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
        | TypeRef::Set(a, _)
        | TypeRef::HttpResult(a, _)
        | TypeRef::History(a, _) => walk_ref(a, params, types, visited),
        TypeRef::Result(a, b, _) | TypeRef::Map(a, b, _) => {
            walk_ref(a, params, types, visited).or_else(|| walk_ref(b, params, types, visited))
        }
        // A type parameter is checked through the applied argument at the use
        // site (`walk_ty`'s `Named` arm), not here.
        TypeRef::Named(id) if params.contains(&id.name.as_str()) => None,
        TypeRef::Named(id) => walk_decl(&id.name, types, visited),
        TypeRef::App { name, args, .. } => args
            .iter()
            .find_map(|a| walk_ref(a, params, types, visited))
            .or_else(|| walk_decl(&name.name, types, visited)),
        TypeRef::Base(..)
        | TypeRef::QueueResult(_)
        | TypeRef::Asset(_)
        | TypeRef::RequestHeaders(_)
        | TypeRef::TokenClaims(_)
        | TypeRef::BoundaryRejection(_)
        | TypeRef::ValidationError(_)
        | TypeRef::JsonError(_)
        | TypeRef::Unit(_) => None,
    }
}

/// Render a declared field type as the author wrote it, for the diagnostic.
/// (`context_checks::type_ref_to_display` covers only the shapes its own callers
/// meet; this walk needs every `TypeRef`.)
pub(crate) fn render(t: &TypeRef) -> String {
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
        TypeRef::Set(a, _) => one("Set", a),
        TypeRef::Query(a, _) => one("Query", a),
        TypeRef::Stream(a, _) => one("Stream", a),
        TypeRef::Connection(a, _) => one("Connection", a),
        TypeRef::History(a, _) => one("History", a),
        TypeRef::QueueResult(_) => "QueueResult".to_string(),
        TypeRef::Asset(_) => "Asset".to_string(),
        TypeRef::RequestHeaders(_) => "RequestHeaders".to_string(),
        TypeRef::TokenClaims(_) => "TokenClaims".to_string(),
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

/// The diagnostic code and reason for a non-comparable `blocker`; `top` is
/// whether the type itself (not a part of it) is the offender.
pub(crate) fn explain(blocker: NotComparable, top: bool) -> (&'static str, String) {
    let it = if top { "it is" } else { "it contains" };
    match blocker {
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
    }
}

/// At a call to a generic function or method, check that every `Eq`-bounded
/// type parameter of the callee (`eq_params`, in report order with its
/// declaration span) is instantiated (`subst`) with an equality-supporting type
/// the caller can vouch for: a concrete one that [`not_comparable`] accepts, or
/// type parameters the caller itself declares `Eq` (`ctx.eq`;
/// `bynk.generics.missing_eq_bound` otherwise). `callee` names it in the
/// message. Returns `false` after reporting the first violation.
pub(crate) fn check_declared_eq_args(
    callee: &str,
    call_span: Span,
    eq_params: &[(String, Option<Span>)],
    subst: &HashMap<String, TyId>,
    ctx: &mut Ctx,
) -> bool {
    let tys = ctx.tys;
    for (param, decl_span) in eq_params {
        let Some(&arg_ty) = subst.get(param) else {
            continue;
        };
        // A scan of a generic caller sees the requirement pass through.
        record_compared(arg_ty, tys);
        if let Some(blocker) = not_comparable(arg_ty, ctx) {
            let shown = arg_ty.display(tys);
            let top = matches!(
                &*tys.get(arg_ty),
                Ty::Stream(_) | Ty::Connection(_) | Ty::Fn { .. } | Ty::Effect(_) | Ty::Query(_)
            );
            let (code, why) = explain(blocker, top);
            let mut err = CompileError::new(
                code,
                call_span,
                format!(
                    "`{callee}` requires `{param}: Eq`, but this call makes `{param}` `{shown}` — {why}"
                ),
            );
            // A label renders against the caller's file, so point at the
            // parameter only when the callee is declared in that same file.
            if let Some(span) = decl_span
                && span.file == call_span.file
            {
                err = err.with_label(*span, "declared `Eq` here");
            }
            ctx.errors.push(err);
            return false;
        }
        if !require_eq(
            ctx,
            arg_ty,
            call_span,
            &format!("`{callee}` requires `{param}: Eq`"),
        ) {
            return false;
        }
    }
    true
}
