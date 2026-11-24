//! #3951: a constant as a refinement bound (`InRange(1, MaxQty)`), under ADR
//! 0826's rule.
//!
//! The parser keeps a constant it meets in a predicate argument as a name
//! ([`bynk_syntax::ast::PendingPred`]); it has no constant table. This pass, run
//! with the annotation-argument pass ([`crate::annotation_consts`]) and ahead of
//! every reader of a refinement, replaces the name with the literal it denotes
//! and builds the predicate's real [`PredKind`]. Literal admission, boundary
//! subsumption, the OpenAPI schema and the property-test generator then read a
//! predicate that is, to them, written with literals; none holds a constant
//! table or can miss one.
//!
//! The resolution is a lookup, not an evaluation ([`crate::consts`]): the
//! constant is declared exactly the argument's type and bound to a literal or
//! to another such constant. A constant that is not is left as written with the
//! reason recorded, and [`pending_errors`] reports it.

use bynk_syntax::ast::{
    BaseType, BoundRefusal, ConstRef, DecimalBound, DurationBound, ExprKind, FloatBound, IntBound,
    PendingArg, PendingPred, PredBound, PredKind, Refinement, RefinementPred, TypeRef, UnaryOp,
    UnresolvedBound,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::annotation_consts::Scope;
use crate::index::{RefSink, SymbolKind};

/// The base type a constant in argument position must be declared: a string for
/// `Matches`, an integer for a length, and the refined type's own base for a
/// range bound.
fn argument_base(predicate: &str, base: BaseType) -> BaseType {
    match predicate {
        "Matches" => BaseType::String,
        "MinLength" | "MaxLength" | "Length" => BaseType::Int,
        _ => base,
    }
}

/// The argument a literal `kind` stands for, written at `span`.
fn literal_arg(kind: &ExprKind, span: Span) -> Option<PendingArg> {
    let (negated, kind) = match kind {
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => (true, &inner.kind),
        k => (false, k),
    };
    let sign = if negated { "-" } else { "" };
    Some(match kind {
        ExprKind::StrLit(s) => PendingArg::Str(s.clone()),
        ExprKind::IntLit { value, .. } => PendingArg::Num(PredBound::Int(IntBound {
            value: if negated { -value } else { *value },
            lexeme: [sign, &value.to_string()].concat(),
            span,
        })),
        ExprKind::FloatLit { value, lexeme } => PendingArg::Num(PredBound::Float(FloatBound {
            value: if negated { -value } else { *value },
            lexeme: [sign, lexeme].concat(),
            span,
        })),
        ExprKind::DecimalLit {
            coef,
            scale,
            lexeme,
        } => PendingArg::Num(PredBound::Decimal(DecimalBound {
            coef: [sign, coef].concat(),
            scale: *scale,
            lexeme: [sign, lexeme].concat(),
            span,
        })),
        ExprKind::DurationLit {
            value,
            unit,
            millis,
        } => PendingArg::Num(PredBound::Duration(DurationBound {
            millis: *millis,
            lexeme: [&value.to_string(), ".", unit.name_for(*value)].concat(),
            span,
        })),
        _ => return None,
    })
}

/// Why `name` is not usable as a bound whose constant must be declared `want`.
/// `owner` names the refined type being declared, when there is one, so a
/// constant declared that type is reported as the cycle it is.
fn refusal(scope: &Scope, name: &str, want: BaseType, owner: Option<&str>) -> BoundRefusal {
    let Some((table, uses)) = scope.lookup(scope.unit) else {
        return BoundRefusal::Unknown;
    };
    let vis = crate::consts::visible_consts(table, uses, |u| scope.lookup(u).map(|(t, _)| t));
    let Some(found) = vis.get(name) else {
        return BoundRefusal::Unknown;
    };
    match &found.decl.ty {
        TypeRef::Named(ty) if Some(ty.name.as_str()) == owner => {
            BoundRefusal::Cycle(vec![name.to_string(), ty.name.clone(), name.to_string()])
        }
        TypeRef::Base(b, _) if *b == want => BoundRefusal::NotLiteral,
        ty => BoundRefusal::Declared(crate::context_checks::ts_type_ref_display(ty)),
    }
}

/// The predicate `pending` stands for once every argument is a literal, or
/// `None` when the arguments do not fit it (mixed numeric types, a wrong count).
fn build_kind(pending: &PendingPred) -> Option<PredKind> {
    let nums = || -> Option<Vec<&PredBound>> {
        pending
            .args
            .iter()
            .map(|a| match a {
                PendingArg::Num(b) => Some(b),
                _ => None,
            })
            .collect()
    };
    let length = || match nums()?.as_slice() {
        [PredBound::Int(b)] => Some(b.value),
        _ => None,
    };
    match pending.name.as_str() {
        "Matches" => match pending.args.as_slice() {
            [PendingArg::Str(s)] => Some(PredKind::Matches(s.clone())),
            _ => None,
        },
        "MinLength" => length().map(PredKind::MinLength),
        "MaxLength" => length().map(PredKind::MaxLength),
        "Length" => length().map(PredKind::Length),
        "AtLeast" | "AtMost" => {
            let [b] = nums()?.as_slice().try_into().ok()?;
            let b = (*b).clone();
            Some(if pending.name == "AtLeast" {
                PredKind::AtLeast(b)
            } else {
                PredKind::AtMost(b)
            })
        }
        "InRange" => match nums()?.as_slice() {
            [PredBound::Int(a), PredBound::Int(b)] => Some(PredKind::InRange(a.clone(), b.clone())),
            [PredBound::Float(a), PredBound::Float(b)] => {
                Some(PredKind::InRangeF(a.clone(), b.clone()))
            }
            [PredBound::Decimal(a), PredBound::Decimal(b)] => {
                Some(PredKind::InRangeD(a.clone(), b.clone()))
            }
            [PredBound::Duration(a), PredBound::Duration(b)] => {
                Some(PredKind::InRangeDur(a.clone(), b.clone()))
            }
            _ => None,
        },
        _ => None,
    }
}

/// Substitute the constants of one predicate. Returns whether it changed.
fn resolve_pred(
    pred: &mut RefinementPred,
    base: BaseType,
    owner: Option<&str>,
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    let Some(mut pending) = pred.pending.take() else {
        return false;
    };
    let mut unresolved = Vec::new();
    for arg in &mut pending.args {
        let PendingArg::Const(id) = arg else {
            continue;
        };
        let want = argument_base(&pending.name, base);
        let found =
            crate::consts::resolve_const_literal(|u| scope.lookup(u), scope.unit, &id.name, want);
        match found.and_then(|f| literal_arg(&f.literal.kind, id.span).map(|a| (f, a))) {
            Some((f, literal)) => {
                let same_file = f.owner.is_none() && scope.file_consts.contains(&id.name);
                pred.from_const.push(ConstRef {
                    name: id.clone(),
                    decl_span: same_file.then_some(f.decl.name.span),
                });
                refs.record(id.span, SymbolKind::Const, &id.name);
                *arg = literal;
            }
            None => unresolved.push(UnresolvedBound {
                name: id.clone(),
                why: refusal(scope, &id.name, want, owner),
            }),
        }
    }
    if unresolved.is_empty()
        && let Some(kind) = build_kind(&pending)
    {
        pred.kind = kind;
        return true;
    }
    pending.unresolved = unresolved;
    pred.pending = Some(pending);
    true
}

/// Substitute the constants in every predicate of `refinement`, a refinement of
/// a value of type `base`. `owner` is the refined type being declared, if any.
pub(crate) fn resolve_refinement(
    refinement: &mut Option<Refinement>,
    base: BaseType,
    owner: Option<&str>,
    scope: &Scope,
    refs: &mut RefSink,
) -> bool {
    let Some(r) = refinement else {
        return false;
    };
    let mut changed = false;
    for pred in &mut r.predicates {
        changed |= resolve_pred(pred, base, owner, scope, refs);
    }
    changed
}

/// The diagnostics for a predicate whose constants did not resolve, or whose
/// literal arguments do not fit it.
pub fn pending_errors(pred: &RefinementPred, base: BaseType) -> Vec<CompileError> {
    let Some(pending) = &pred.pending else {
        return Vec::new();
    };
    let mut errors: Vec<CompileError> = pending
        .unresolved
        .iter()
        .map(|u| bound_error(&pending.name, base, u))
        .collect();
    if errors.is_empty() {
        errors.push(
            CompileError::new(
                "bynk.types.no_numeric_coercion",
                pred.span,
                format!(
                    "the arguments of `{}` are not all of the type `{}` takes",
                    pending.name,
                    argument_base(&pending.name, base).name()
                ),
            )
            .with_note(
                "a bound and the constant standing for it are the same numeric type; \
                 there is no coercion between them",
            ),
        );
    }
    errors
}

fn bound_error(predicate: &str, base: BaseType, u: &UnresolvedBound) -> CompileError {
    let want = argument_base(predicate, base).name();
    let name = &u.name.name;
    if let BoundRefusal::Cycle(path) = &u.why {
        let shown = path
            .iter()
            .map(|n| format!("`{n}`"))
            .collect::<Vec<_>>()
            .join(" → ");
        return CompileError::new(
            "bynk.resolve.const_cycle",
            u.name.span,
            format!("constant `{name}` refers to itself: {shown}"),
        )
        .with_note(
            "a constant's value cannot depend on itself; this one is declared the refined \
             type whose bound it is, so the bound would need the type and the type the bound",
        );
    }
    let why = match &u.why {
        BoundRefusal::Unknown => format!("`{name}` is not a constant in scope"),
        BoundRefusal::Declared(ty) => {
            format!("`{name}` is declared `{ty}`, not exactly `{want}`")
        }
        BoundRefusal::NotLiteral | BoundRefusal::Cycle(_) => {
            format!("`{name}` is not bound to a literal, or to another constant that is")
        }
    };
    CompileError::new(
        "bynk.refine.bound_not_literal",
        u.name.span,
        format!("`{name}` cannot stand as an argument of `{predicate}`: {why}"),
    )
    .with_note(format!(
        "an argument of a refinement predicate is a literal, or a constant declared exactly \
         `{want}` and bound to a literal or to another such constant (ADR 0826); the constant \
         is replaced by that literal before the refinement is read"
    ))
    .with_note("bind the constant to a literal of the argument's type, or write the literal here")
}
