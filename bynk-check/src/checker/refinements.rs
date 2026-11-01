//! Refinement, literal, and zero-value logic.
//!
//! Split out of `checker.rs` (v0.29.10) verbatim; the parent module
//! re-exports these via `use refinements::*`.

use super::*;

pub(crate) fn check_type_decl(
    t: &TypeDecl,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
    errors: &mut Vec<CompileError>,
) {
    match &t.body {
        TypeBody::Refined {
            base,
            base_span,
            refinement,
        } => {
            check_refinement(*base, *base_span, refinement.as_ref(), errors);
        }
        TypeBody::Opaque {
            base,
            base_span,
            refinement,
        } => {
            // Opaque types share refinement-validity rules with refined types.
            check_refinement(*base, *base_span, refinement.as_ref(), errors);
        }
        TypeBody::Record(r) => {
            for f in &r.fields {
                if let Some(ref_r) = &f.refinement {
                    // Inline refinements on fields must apply to the field's base type.
                    if let Some(b) = field_base_type(&f.type_ref, types) {
                        check_refinement(b, f.type_ref.span(), Some(ref_r), errors);
                    } else {
                        errors.push(CompileError::new(
                            "bynk.types.field_refinement_not_base",
                            ref_r.span,
                            format!(
                                "inline refinement on field `{}` requires a base or refined type",
                                f.name.name
                            ),
                        ));
                    }
                }
            }
        }
        TypeBody::Sum(s) => {
            check_embeds(&t.name.name, s, types, errors, tys);
        }
    }
}

/// v0.154 (ADR 0178): validate a sum's declared error embeddings. Each
/// `embeds E as V` requires the named variant `V` to exist in this sum and to
/// have **exactly one payload field whose type is `E`** — that is the shape a
/// value of `E` auto-wraps into. A source type may be embedded by at most one
/// variant, so `?`'s conversion is unambiguous.
pub(crate) fn check_embeds(
    sum_name: &str,
    s: &SumBody,
    types: &HashMap<String, Arc<TypeDecl>>,
    errors: &mut Vec<CompileError>,
    tys: &Types,
) {
    let mut seen_sources: Vec<TyId> = Vec::new();
    for clause in &s.embeds {
        let Some(source_ty) = resolve_type_ref(&clause.source_type, types, tys) else {
            // An unresolvable type ref is already reported by reference
            // resolution; skip to avoid a duplicate error.
            continue;
        };
        // The named variant must exist in this sum.
        let Some(variant) = s
            .variants
            .iter()
            .find(|v| v.name.name == clause.variant.name)
        else {
            errors.push(CompileError::new(
                "bynk.types.embeds_unknown_variant",
                clause.variant.span,
                format!(
                    "`embeds … as {}` names no variant of `{}`",
                    clause.variant.name, sum_name
                ),
            ));
            continue;
        };
        // The variant must be a single-payload wrapper of the embedded type.
        if variant.payload.len() != 1 {
            errors.push(
                CompileError::new(
                    "bynk.types.embeds_variant_shape",
                    clause.span,
                    format!(
                        "`embeds … as {}` requires `{}` to have exactly one payload field, but it has {}",
                        clause.variant.name,
                        clause.variant.name,
                        variant.payload.len()
                    ),
                )
                .with_note("a value of the embedded type is wrapped into that single field"),
            );
            continue;
        }
        let field_ty = resolve_type_ref(&variant.payload[0].type_ref, types, tys);
        if let Some(field_ty) = &field_ty
            && !compatible(source_ty, *field_ty, tys)
        {
            errors.push(CompileError::new(
                "bynk.types.embeds_variant_shape",
                clause.span,
                format!(
                    "`embeds {} as {}` — but `{}`'s payload field has type `{}`, not `{}`",
                    source_ty.display(tys),
                    clause.variant.name,
                    clause.variant.name,
                    field_ty.display(tys),
                    source_ty.display(tys)
                ),
            ));
            continue;
        }
        // The same source type may be embedded once at most (unambiguous `?`).
        if seen_sources.iter().any(|t| compatible(*t, source_ty, tys)) {
            errors.push(CompileError::new(
                "bynk.types.embeds_ambiguous",
                clause.span,
                format!(
                    "`{}` is embedded more than once by `{}` — the conversion would be ambiguous",
                    source_ty.display(tys),
                    sum_name
                ),
            ));
            continue;
        }
        seen_sources.push(source_ty);
    }
}

/// The base type of a field's type-ref (chasing through named refined types).
fn field_base_type(r: &TypeRef, types: &HashMap<String, Arc<TypeDecl>>) -> Option<BaseType> {
    match r {
        TypeRef::Base(b, _) => Some(*b),
        TypeRef::Named(id) => match types.get(&id.name).map(|t| &t.body) {
            Some(TypeBody::Refined { base, .. }) => Some(*base),
            _ => None,
        },
        _ => None,
    }
}

/// The implicit base type of a TypeDecl whose constructor would be `T.of`:
/// Refined and Opaque types alike share the `of(base) -> Result[T, _]` shape.
/// Returns None for record / sum types.
pub(crate) fn type_decl_base(decl: &TypeDecl) -> Option<BaseType> {
    match &decl.body {
        TypeBody::Refined { base, .. } => Some(*base),
        TypeBody::Opaque { base, .. } => Some(*base),
        _ => None,
    }
}

/// The refinement attached to a refined or opaque type declaration, if any.
pub(crate) fn type_decl_refinement(decl: &TypeDecl) -> Option<&Refinement> {
    match &decl.body {
        TypeBody::Refined { refinement, .. } | TypeBody::Opaque { refinement, .. } => {
            refinement.as_ref()
        }
        _ => None,
    }
}

/// Extract a compile-time literal from an expression, if it is one v0.9.4's
/// static refinement check accepts: an int/string/bool/unit literal, or a unary
/// minus applied directly to an int literal. Anything else (arithmetic, idents,
/// calls) is not statically evaluated and keeps the runtime `Result` path.
pub(crate) fn const_literal(e: &Expr) -> Option<ConstLit> {
    match &e.kind {
        ExprKind::IntLit { value: n, .. } => Some(ConstLit::Int(*n)),
        ExprKind::FloatLit { value, .. } => Some(ConstLit::Float(*value)),
        ExprKind::StrLit(s) => Some(ConstLit::Str(s.clone())),
        ExprKind::BoolLit(b) => Some(ConstLit::Bool(*b)),
        ExprKind::UnitLit => Some(ConstLit::Unit),
        ExprKind::UnaryOp(UnaryOp::Neg, inner) => match &inner.kind {
            ExprKind::IntLit { value: n, .. } => Some(ConstLit::Int(n.checked_neg()?)),
            ExprKind::FloatLit { value, .. } => Some(ConstLit::Float(-*value)),
            _ => None,
        },
        _ => None,
    }
}

/// Evaluate a single predicate against a constant literal. A predicate whose
/// expected base type doesn't match the literal (e.g. a length predicate on an
/// int) returns `true` here — the base/predicate mismatch is a declaration-time
/// error reported by `check_refinement`, not a construction concern. String
/// length is measured in Unicode scalar values, which agrees with JS `.length`
/// for the BMP (the range fixtures use ASCII).
pub(crate) fn eval_predicate(pred: &PredKind, lit: &ConstLit) -> bool {
    match (pred, lit) {
        (PredKind::NonNegative, ConstLit::Int(n)) => *n >= 0,
        (PredKind::Positive, ConstLit::Int(n)) => *n > 0,
        (PredKind::InRange(lo, hi), ConstLit::Int(n)) => lo.value <= *n && *n <= hi.value,
        (PredKind::NonNegative, ConstLit::Float(v)) => *v >= 0.0,
        (PredKind::Positive, ConstLit::Float(v)) => *v > 0.0,
        (PredKind::InRangeF(lo, hi), ConstLit::Float(v)) => lo.value <= *v && *v <= hi.value,
        (PredKind::MinLength(k), ConstLit::Str(s)) => s.chars().count() as i64 >= *k,
        (PredKind::MaxLength(k), ConstLit::Str(s)) => (s.chars().count() as i64) <= *k,
        (PredKind::Length(k), ConstLit::Str(s)) => s.chars().count() as i64 == *k,
        (PredKind::NonEmpty, ConstLit::Str(s)) => !s.is_empty(),
        (PredKind::Matches(pat), ConstLit::Str(s)) => {
            // Evaluated with the same engine semantics the emitted
            // `new RegExp(...)` runs under (ECMAScript, no flags).
            regress::Regex::new(&format!("^(?:{pat})$"))
                .map(|re| re.find(s).is_some())
                .unwrap_or(false)
        }
        _ => true,
    }
}

/// The first predicate the literal fails, or `None` if it satisfies them all.
pub(crate) fn first_failed_predicate<'a>(
    refinement: &'a Refinement,
    lit: &ConstLit,
) -> Option<&'a PredKind> {
    // #1651: the runtime checks' order, so a literal rejected here names the
    // predicate the boundary would.
    bynk_syntax::ast::in_check_order(&refinement.predicates, |p| &p.kind)
        .into_iter()
        .find(|p| !eval_predicate(&p.kind, lit))
        .map(|p| &p.kind)
}

/// `LocaleTag`'s refinement, read once from the firstparty `bynk.locale.types`
/// source — the single source of truth the emitter also lowers to a
/// `new RegExp(...)`. `None` only if that type ever loses its refinement or the
/// firstparty source stops parsing (both guarded elsewhere), in which case the
/// tag check below accepts everything rather than spuriously rejecting.
fn locale_tag_refinement() -> Option<&'static Refinement> {
    use std::sync::OnceLock;
    static REFINEMENT: OnceLock<Option<Refinement>> = OnceLock::new();
    REFINEMENT
        .get_or_init(|| {
            let src = crate::firstparty::BYNK_LOCALE_TYPES_SRC;
            let tokens = bynk_syntax::lexer::tokenize(src).ok()?;
            let unit = bynk_syntax::parser::parse_unit(&tokens, src).ok()?;
            let bynk_syntax::ast::SourceUnit::Commons(commons) = unit else {
                return None;
            };
            commons.items.iter().find_map(|item| match item {
                CommonsItem::Type(t) if t.name.name == "LocaleTag" => {
                    type_decl_refinement(t).cloned()
                }
                _ => None,
            })
        })
        .as_ref()
}

/// The pattern `LocaleTag`'s refinement matches against, for a diagnostic that
/// names it. `None` if the type carries no `Matches` predicate.
pub fn locale_tag_pattern() -> Option<&'static str> {
    locale_tag_refinement()?.predicates.iter().find_map(|p| {
        if let PredKind::Matches(pat) = &p.kind {
            Some(pat.as_str())
        } else {
            None
        }
    })
}

/// Whether `tag` satisfies `LocaleTag`'s refinement — the check behind
/// `bynk.messages.invalid_locale_tag`. Evaluated with the same regex-engine
/// semantics (`regress`, anchored, no flags) the emitted `new RegExp(...)`
/// runs under, so a tag accepted here is one the runtime cast is honest about.
pub fn locale_tag_accepts(tag: &str) -> bool {
    match locale_tag_refinement() {
        Some(refinement) => {
            first_failed_predicate(refinement, &ConstLit::Str(tag.to_string())).is_none()
        }
        None => true,
    }
}

pub(crate) fn literal_matches_base(lit: &ConstLit, base: BaseType) -> bool {
    matches!(
        (lit, base),
        (ConstLit::Int(_), BaseType::Int)
            | (ConstLit::Str(_), BaseType::String)
            | (ConstLit::Bool(_), BaseType::Bool)
            | (ConstLit::Float(_), BaseType::Float)
    )
}

/// v0.9.4: expected-type-directed literal admission. When a position expects a
/// **refined** type `T` and `expr` is a compile-time literal of `T`'s base, the
/// literal takes the type `T` directly (the emitter lowers it to an inline brand
/// cast, `(lit as T)` — ADR 0182); a literal that violates the refinement is a
/// compile error. Returns `None` when no refined type is expected (so the caller
/// keeps the literal's base type) — `.of` remains the only constructor for
/// runtime values.
/// Opaque types are intentionally excluded: their representation is hidden, so
/// they are still built via `T.of(...)`.
///
/// Looks through one `Effect[_]` layer first (Locale capability track, slice 1,
/// #844): a `stub Cap.op() returns <lit>` RHS is checked as a bare tail value
/// against the op's full `Effect[T]` return type (§tail-position auto-lift
/// handles the reverse direction, wrapping a matched bare value back up to
/// `Effect[T]`), so a refined `T` reached only through that wrapper must still
/// be visible here or the literal falls back to its unrefined base type and the
/// auto-lift's own `compatible` check then rejects it.
pub(crate) fn admit_refined_literal(
    expr: &Expr,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let input = ctx.input;
    admit_refined_literal_in(expr, expected, &input.types, ctx)
}

/// [`admit_refined_literal`] against an explicit declaration table: the one
/// that declares `expected`'s names. #2025: a cross-context argument's
/// parameter type is named in the consumed context's table, so a literal
/// passed straight to it is checked against the callee's declaration, not
/// whatever the caller binds under that name (or nothing, when the caller
/// has no `uses` of the declaring commons).
pub(crate) fn admit_refined_literal_in(
    expr: &Expr,
    expected: Option<TyId>,
    types: &HashMap<String, Arc<TypeDecl>>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let expected = match expected.map(|e| tys.get(e)).as_deref() {
        Some(Ty::Effect(inner)) => Some(*inner),
        _ => expected,
    };
    let expected_node = expected.map(|e| tys.get(e));
    let Some(Ty::Named {
        name,
        kind: NamedKind::Refined(base),
        ..
    }) = expected_node.as_deref()
    else {
        return None;
    };
    let lit = const_literal(expr)?;
    if !literal_matches_base(&lit, *base) {
        return None;
    }
    let decl = types.get(name)?.clone();
    if let Some(refinement) = type_decl_refinement(&decl)
        && let Some(failed) = first_failed_predicate(refinement, &lit)
    {
        ctx.errors.push(CompileError::new(
            "bynk.refine.literal_violates",
            expr.span,
            format!(
                "literal {} does not satisfy `{}` required by type `{}`",
                lit.display(),
                failed.name(),
                name
            ),
        ));
    }
    Some(named_ty(&decl, tys))
}

pub(crate) fn check_refinement(
    base: BaseType,
    base_span: Span,
    refinement: Option<&Refinement>,
    errors: &mut Vec<CompileError>,
) {
    let Some(refinement) = refinement else {
        return;
    };

    for pred in &refinement.predicates {
        if !pred_applies_to(&pred.kind, base) {
            // v0.21: `InRange` bounds must match the numeric base type —
            // `Float where InRange(0, 1)` is the no-coercion rule applied
            // to refinement bounds, not a predicate/base mismatch.
            let numeric_bound_mismatch = matches!(
                (&pred.kind, base),
                (PredKind::InRange(_, _), BaseType::Float)
                    | (PredKind::InRangeF(_, _), BaseType::Int)
            );
            if numeric_bound_mismatch {
                let (bounds, want) = if base == BaseType::Float {
                    ("`Int`", "`InRange(0.0, 1.0)`")
                } else {
                    ("`Float`", "`InRange(0, 1)`")
                };
                errors.push(
                    CompileError::new(
                        "bynk.types.no_numeric_coercion",
                        pred.span,
                        format!(
                            "`InRange` bounds are {bounds} literals, but the base type is `{}`",
                            base.name()
                        ),
                    )
                    .with_label(
                        base_span,
                        format!("base type `{}` declared here", base.name()),
                    )
                    .with_note(format!(
                        "refinement bounds must match the base type — e.g. {want}"
                    )),
                );
                continue;
            }
            errors.push(
                CompileError::new(
                    "bynk.types.predicate_base_mismatch",
                    pred.span,
                    format!(
                        "predicate `{}` cannot be applied to base type `{}`",
                        pred.kind.name(),
                        base.name()
                    ),
                )
                .with_label(
                    base_span,
                    format!("base type `{}` declared here", base.name()),
                )
                .with_note(predicate_base_help(pred.kind.name())),
            );
        }
        match &pred.kind {
            PredKind::Matches(pat) => {
                // Validate with ECMAScript semantics (the `regress` engine):
                // the emitted check runs the pattern under JS `RegExp`, so a
                // pattern the Rust `regex` crate accepts but JS rejects
                // (`(?P<name>…)`, inline flags) would otherwise compile
                // cleanly and then throw at runtime — a 500 on the request
                // path instead of a compile error.
                if let Err(e) = regress::Regex::new(pat) {
                    errors.push(
                        CompileError::new(
                            "bynk.types.invalid_regex",
                            pred.span,
                            format!("invalid regular expression in `Matches(\"{pat}\")`"),
                        )
                        .with_note(format!("regex parse error (JS `RegExp` semantics): {e}")),
                    );
                } else if has_nested_unbounded_quantifier(pat) {
                    // The emitted boundary check runs this pattern under JS
                    // `RegExp`, a backtracking engine. A repeated group that
                    // itself contains an unbounded quantifier (`(a+)+`) makes
                    // matching take exponential time on a crafted near-miss
                    // input — a refined `String` on an HTTP boundary would let
                    // an unauthenticated client stall the Worker (ReDoS, #724).
                    // Reject the pattern at compile time rather than ship the
                    // hazard. This cheap first pass keeps its own, clearer
                    // message; the ambiguity analysis below covers the rest.
                    errors.push(
                        CompileError::new(
                            "bynk.types.catastrophic_regex",
                            pred.span,
                            format!(
                                "the pattern in `Matches(\"{pat}\")` nests unbounded quantifiers, \
                                 which can cause catastrophic backtracking (ReDoS)"
                            ),
                        )
                        .with_note(
                            "a repeated group that itself contains `*`, `+`, or `{n,}` makes \
                             matching take exponential time on crafted input; restructure the \
                             pattern so no unbounded quantifier is nested inside another",
                        ),
                    );
                } else if let Some(e) = ambiguity_diagnostic(
                    pat,
                    "",
                    pred.span,
                    &format!("`Matches(\"{pat}\")`"),
                    // The tightest length bound the refinement states. Length
                    // predicates run before `Matches` (`in_check_order`), so it
                    // caps the input the pattern ever sees.
                    Some(
                        refinement
                            .predicates
                            .iter()
                            .filter_map(|p| match p.kind {
                                PredKind::MaxLength(n) | PredKind::Length(n) => Some(n),
                                _ => None,
                            })
                            .min(),
                    ),
                ) {
                    errors.push(e);
                }
            }
            PredKind::InRange(lo, hi) => {
                if lo.value > hi.value {
                    errors.push(
                        CompileError::new(
                            "bynk.types.inverted_range",
                            pred.span,
                            format!(
                                "`InRange({}, {})` has its bounds inverted (`min` must be ≤ `max`)",
                                lo.value, hi.value
                            ),
                        )
                        // v0.40 (ADR 0073): a machine-applicable swap — replace
                        // each bound's text with the other's, in place.
                        .with_suggestion(
                            "swap the bounds",
                            vec![
                                (lo.span, hi.value.to_string()),
                                (hi.span, lo.value.to_string()),
                            ],
                            Applicability::MachineApplicable,
                        ),
                    );
                }
            }
            PredKind::InRangeF(lo, hi) => {
                if lo.value > hi.value {
                    errors.push(
                        CompileError::new(
                            "bynk.types.inverted_range",
                            pred.span,
                            format!(
                                "`InRange({}, {})` has its bounds inverted (`min` must be ≤ `max`)",
                                lo.lexeme, hi.lexeme
                            ),
                        )
                        .with_suggestion(
                            "swap the bounds",
                            vec![(lo.span, hi.lexeme.clone()), (hi.span, lo.lexeme.clone())],
                            Applicability::MachineApplicable,
                        ),
                    );
                }
            }
            PredKind::MinLength(n) | PredKind::MaxLength(n) | PredKind::Length(n) => {
                if *n < 0 {
                    errors.push(CompileError::new(
                        "bynk.types.negative_length",
                        pred.span,
                        format!("length argument must be non-negative, got {n}"),
                    ));
                }
            }
            PredKind::NonNegative | PredKind::Positive | PredKind::NonEmpty => {}
        }
    }

    let all_compatible = refinement
        .predicates
        .iter()
        .all(|p| pred_applies_to(&p.kind, base));
    if !all_compatible {
        return;
    }
    match base {
        BaseType::Int => check_int_refinement_consistency(refinement, errors),
        BaseType::String => check_string_refinement_consistency(refinement, errors),
        BaseType::Bool => {}
        BaseType::Float => check_float_refinement_consistency(refinement, errors),
        // v0.86/v0.90/v0.110: no refinement predicate applies to `Duration`,
        // `Instant`, or `Bytes` (none is in any `pred_applies_to` row), so a
        // refined one is rejected upstream and there is nothing to
        // consistency-check here.
        BaseType::Duration | BaseType::Instant | BaseType::Bytes => {}
    }
}

fn pred_applies_to(pred: &PredKind, base: BaseType) -> bool {
    matches!(
        (pred, base),
        (PredKind::Matches(_), BaseType::String)
            | (PredKind::InRange(_, _), BaseType::Int)
            | (PredKind::InRangeF(_, _), BaseType::Float)
            | (PredKind::MinLength(_), BaseType::String)
            | (PredKind::MaxLength(_), BaseType::String)
            | (PredKind::Length(_), BaseType::String)
            | (PredKind::NonNegative, BaseType::Int | BaseType::Float)
            | (PredKind::Positive, BaseType::Int | BaseType::Float)
            | (PredKind::NonEmpty, BaseType::String)
    )
}

fn predicate_base_help(name: &str) -> &'static str {
    match name {
        "Matches" | "MinLength" | "MaxLength" | "Length" | "NonEmpty" => {
            "this predicate applies to `String` only"
        }
        "NonNegative" | "Positive" => "this predicate applies to `Int` and `Float` only",
        "InRange" => {
            "this predicate applies to `Int` and `Float` only, with bounds matching the base"
        }
        _ => "see the documentation for valid predicate-base combinations",
    }
}

pub(crate) fn check_int_refinement_consistency(
    refinement: &Refinement,
    errors: &mut Vec<CompileError>,
) {
    let mut lo: i64 = i64::MIN;
    let mut hi: i64 = i64::MAX;
    for p in &refinement.predicates {
        match &p.kind {
            PredKind::Positive => lo = lo.max(1),
            PredKind::NonNegative => lo = lo.max(0),
            PredKind::InRange(a, b) => {
                lo = lo.max(a.value);
                hi = hi.min(b.value);
            }
            _ => {}
        }
    }
    if lo > hi {
        errors.push(
            CompileError::new(
                "bynk.types.empty_refinement",
                refinement.span,
                "this refinement has no valid values — the predicates contradict each other",
            )
            .with_note(format!(
                "the effective range is `{lo}..={hi}`, which is empty"
            )),
        );
    }
}

pub(crate) fn check_float_refinement_consistency(
    refinement: &Refinement,
    errors: &mut Vec<CompileError>,
) {
    let mut lo = f64::NEG_INFINITY;
    let mut hi = f64::INFINITY;
    // `Positive` excludes the lower endpoint (0.0 itself is not positive).
    let mut lo_exclusive = false;
    for p in &refinement.predicates {
        match &p.kind {
            PredKind::Positive if 0.0 >= lo => {
                lo = 0.0;
                lo_exclusive = true;
            }
            PredKind::NonNegative if 0.0 > lo => {
                lo = 0.0;
                lo_exclusive = false;
            }
            PredKind::InRangeF(a, b) => {
                if a.value > lo {
                    lo = a.value;
                    lo_exclusive = false;
                }
                hi = hi.min(b.value);
            }
            _ => {}
        }
    }
    if lo > hi || (lo == hi && lo_exclusive) {
        errors.push(
            CompileError::new(
                "bynk.types.empty_refinement",
                refinement.span,
                "this refinement has no valid values — the predicates contradict each other",
            )
            .with_note(format!(
                "the effective range is `{lo}..={hi}`{}, which is empty",
                if lo_exclusive {
                    " (lower bound exclusive)"
                } else {
                    ""
                }
            )),
        );
    }
}

pub(crate) fn check_string_refinement_consistency(
    refinement: &Refinement,
    errors: &mut Vec<CompileError>,
) {
    let mut min_len: i64 = 0;
    let mut max_len: i64 = i64::MAX;
    let mut exact_len: Option<i64> = None;
    for p in &refinement.predicates {
        match &p.kind {
            PredKind::MinLength(n) => min_len = min_len.max(*n),
            PredKind::MaxLength(n) => max_len = max_len.min(*n),
            PredKind::NonEmpty => min_len = min_len.max(1),
            PredKind::Length(n) => {
                if let Some(prev) = exact_len {
                    if prev != *n {
                        errors.push(CompileError::new(
                            "bynk.types.empty_refinement",
                            refinement.span,
                            format!(
                                "conflicting exact lengths: `Length({prev})` and `Length({n})` cannot both hold"
                            ),
                        ));
                    }
                } else {
                    exact_len = Some(*n);
                }
                min_len = min_len.max(*n);
                max_len = max_len.min(*n);
            }
            _ => {}
        }
    }
    if min_len > max_len {
        errors.push(
            CompileError::new(
                "bynk.types.empty_refinement",
                refinement.span,
                "this refinement has no valid values — minimum length exceeds maximum length",
            )
            .with_note(format!(
                "the effective length range is `{min_len}..={max_len}`, which is empty"
            )),
        );
    }
}

// -- function body type checking --

/// v0.9.1: `assert e` as an expression. Test-privileged. Requires `e : Bool`.
/// Always yields type `()`.
/// True if a refinement cannot be satisfied by a generated default value — i.e.
/// it contains a `Matches` predicate, where bare `Val[T]` must be given an
/// explicit pin instead.
pub(crate) fn refinement_needs_pin(refinement: &Refinement) -> bool {
    refinement
        .predicates
        .iter()
        .any(|p| matches!(p.kind, PredKind::Matches(_)))
}

/// The TypeScript zero-value expression for `type_ref` (with an optional
/// inline field refinement), or `None` if the type is not zeroable.
pub fn zero_value_ts(
    type_ref: &TypeRef,
    inline: Option<&Refinement>,
    types: &HashMap<String, Arc<TypeDecl>>,
) -> Option<String> {
    zero_value_ts_inner(type_ref, inline, types, &mut Vec::new())
}

fn zero_value_ts_inner(
    type_ref: &TypeRef,
    inline: Option<&Refinement>,
    types: &HashMap<String, Arc<TypeDecl>>,
    visiting: &mut Vec<String>,
) -> Option<String> {
    match type_ref {
        TypeRef::Base(b, _) => {
            if refinement_admits_zero(*b, inline) {
                zero_of_base(*b)
            } else {
                None
            }
        }
        // Option's zero is None, regardless of the inner type.
        TypeRef::Option(_, _) => Some("None".to_string()),
        TypeRef::Named(id) => {
            let decl = types.get(&id.name)?;
            match &decl.body {
                TypeBody::Refined {
                    base, refinement, ..
                } => {
                    if refinement_admits_zero(*base, refinement.as_ref()) {
                        zero_of_base(*base)
                    } else {
                        None
                    }
                }
                TypeBody::Record(rec) => {
                    // A record cycle (`A = { b: B }`, `B = { a: A }`) has no
                    // finite zero value; without this guard the recursion is
                    // unbounded and overflows the stack. The resolver rejects
                    // such cycles, but this walk must terminate regardless of
                    // what reaches it.
                    if visiting.iter().any(|n| n == &id.name) {
                        return None;
                    }
                    visiting.push(id.name.clone());
                    let z = agent_state_zero_record(&rec.fields, types, visiting);
                    visiting.pop();
                    z
                }
                // Non-Option sum types and opaque types have no defined zero.
                TypeBody::Sum(_) | TypeBody::Opaque { .. } => None,
            }
        }
        // Result / Effect / HttpResult / ValidationError / Unit are not
        // admissible state-field types and have no zero.
        _ => None,
    }
}

/// The zero record `{ f₁: z₁, …, fₙ: zₙ }` for a set of fields, or `None` if
/// any field is not zeroable.
fn agent_state_zero_record(
    fields: &[RecordField],
    types: &HashMap<String, Arc<TypeDecl>>,
    visiting: &mut Vec<String>,
) -> Option<String> {
    let mut parts = Vec::new();
    for f in fields {
        let z = zero_value_ts_inner(&f.type_ref, f.refinement.as_ref(), types, visiting)?;
        parts.push(format!("{}: {}", f.name.name, z));
    }
    Some(format!("{{ {} }}", parts.join(", ")))
}

fn zero_of_base(b: BaseType) -> Option<String> {
    Some(
        match b {
            BaseType::Int => "0",
            BaseType::Bool => "false",
            BaseType::String => "\"\"",
            BaseType::Float => "0",
            // v0.86/v0.90: a `Duration` is milliseconds and an `Instant` is
            // epoch milliseconds; the zero of each is `0` (the Unix epoch).
            BaseType::Duration | BaseType::Instant => "0",
            // v0.110 (ADR 0142): the zero of `Bytes` is the empty octet
            // sequence (`""` in base64), erased to an empty `Uint8Array`.
            BaseType::Bytes => "new globalThis.Uint8Array()",
        }
        .to_string(),
    )
}

/// Whether the zero value of `base` satisfies every predicate in `refinement`.
/// Conservative: any predicate we cannot prove admits the zero returns false,
/// surfacing the `non_zeroable_state_field` diagnostic rather than risking an
/// invalid fresh state.
fn refinement_admits_zero(base: BaseType, refinement: Option<&Refinement>) -> bool {
    let Some(r) = refinement else {
        return true;
    };
    r.predicates.iter().all(|p| pred_admits_zero(base, &p.kind))
}

fn pred_admits_zero(base: BaseType, k: &PredKind) -> bool {
    match base {
        BaseType::Int => match k {
            PredKind::NonNegative => true,
            PredKind::Positive => false,
            PredKind::InRange(lo, hi) => lo.value <= 0 && 0 <= hi.value,
            // Length/Matches predicates don't apply to Int; reject conservatively.
            _ => false,
        },
        BaseType::String => match k {
            PredKind::Matches(p) => regex_matches_empty(p),
            PredKind::MinLength(n) => *n <= 0,
            PredKind::MaxLength(n) => *n >= 0,
            PredKind::Length(n) => *n == 0,
            PredKind::NonEmpty => false,
            // Numeric predicates don't apply to String; reject conservatively.
            _ => false,
        },
        // The only Bool zero is `false`; no Bool refinement predicates exist.
        BaseType::Bool => true,
        // No refinement predicate applies to `Duration`, `Instant`, or
        // `Bytes`, so the question is vacuous — admit it (mirrors `Bool`).
        BaseType::Duration | BaseType::Instant | BaseType::Bytes => true,
        BaseType::Float => match k {
            PredKind::NonNegative => true,
            PredKind::Positive => false,
            PredKind::InRangeF(lo, hi) => lo.value <= 0.0 && 0.0 <= hi.value,
            // Other predicates don't apply to Float; reject conservatively.
            _ => false,
        },
    }
}

/// Does the refinement pattern match the empty string? Anchored exactly as the
/// emitted refined-type constructor anchors it (`^(?:pattern)$`), and
/// evaluated with the same engine semantics (ECMAScript, no flags).
fn regex_matches_empty(pattern: &str) -> bool {
    match regress::Regex::new(&format!("^(?:{pattern})$")) {
        Ok(re) => re.find("").is_some(),
        Err(_) => false,
    }
}

/// #1651: the diagnostic for a `Matches` pattern the automaton ambiguity
/// analysis (`regex_ambiguity`) does not prove linear, or `None` when it does.
///
/// Exponential ambiguity, and a pattern the analysis cannot bound, are errors.
/// Polynomial ambiguity is an error unless the refinement also bounds the
/// input's length (`MaxLength` or `Length`) tightly enough for the
/// polynomial's degree (`bound^degree` within [`POLYNOMIAL_STEP_CAP`]): the
/// emitter checks length predicates before `Matches`
/// (`bynk_syntax::ast::in_check_order`), so such a bound caps the cost, and it
/// is a warning.
/// `label` names the pattern in the message (`` `Matches("…")` ``, or a K6c
/// regex argument). `bound` is the input-length bound a refinement states:
/// `Some(None)` when it could state one but does not, `None` when no bound is
/// possible (a K6c regex method runs on any string), which changes the advice.
fn ambiguity_diagnostic(
    pat: &str,
    flags: &str,
    span: Span,
    label: &str,
    bound: Option<Option<i64>>,
) -> Option<CompileError> {
    use super::regex_ambiguity::{Verdict, analyse_with_flags};
    // K6c (#2600): the flags change what `.` and a cased letter match, so the
    // analysis must read the pattern under them (`Matches` has none).
    Some(match analyse_with_flags(pat, flags) {
        Verdict::Linear => return None,
        Verdict::Exponential(a, b) => CompileError::new(
            "bynk.types.catastrophic_regex",
            span,
            format!(
                "the pattern in {label} can match the same text in \
                 exponentially many ways, which can cause catastrophic backtracking (ReDoS)"
            ),
        )
        .with_note(format!(
            "inside a repetition, `{a}` and `{b}` can match the same character on \
             different paths, so each repetition doubles the ways a near-miss input \
             is tried; make the alternatives (or repeat counts) inside the loop \
             unambiguous, e.g. `(a|aa)+` as `a+`"
        )),
        Verdict::Unanalysable(why) => CompileError::new(
            "bynk.types.catastrophic_regex",
            span,
            format!(
                "the pattern in {label} cannot be shown free of catastrophic \
                 backtracking (ReDoS): {why}"
            ),
        )
        .with_note(
            "the boundary check runs this pattern on request input under a backtracking \
             engine, so a pattern that cannot be analysed is rejected; simplify it",
        ),
        Verdict::Polynomial(degree, a, b) => {
            let largest = largest_safe_bound(degree);
            let (code, advice) = match bound.flatten() {
                Some(n) if n <= largest => (
                    "bynk.types.polynomial_regex_capped",
                    format!(
                        "the refinement bounds the input at {n} characters, which keeps \
                         the cost small, so this is a warning; rewrite the pattern so the \
                         repetitions cannot overlap to silence it"
                    ),
                ),
                Some(n) => (
                    "bynk.types.polynomial_regex",
                    format!(
                        "the refinement bounds the input at {n} characters, but at degree \
                         {degree} that still allows about {n}^{degree} steps; lower the \
                         bound to at most {largest}, or rewrite the pattern so the \
                         repetitions cannot overlap"
                    ),
                ),
                None if bound.is_some() => (
                    "bynk.types.polynomial_regex",
                    format!(
                        "add a `MaxLength(n)` predicate with `n` at most {largest} to bound \
                         the input, or rewrite the pattern so the repetitions cannot overlap"
                    ),
                ),
                None => (
                    "bynk.types.polynomial_regex",
                    "a regex method runs on input of any length, so rewrite the pattern so \
                     the repetitions cannot overlap"
                        .to_string(),
                ),
            };
            CompileError::new(
                code,
                span,
                format!(
                    "the pattern in {label} can split the same text between \
                     repetitions in many ways, so matching time grows with the input's \
                     length to the power {degree} (ReDoS)"
                ),
            )
            .with_note(format!(
                "`{a}` and `{b}` repeat in sequence and can match the same characters, as \
                 in `\\d*\\d*`; {advice}"
            ))
        }
    })
}

/// K6c (#2594's sibling, #2600): check a regex method's pattern argument the
/// way a `Matches` pattern is checked: valid under ECMAScript semantics
/// (`regress`), no nested unbounded quantifier, and no ambiguity the automaton
/// analysis finds — with no length bound to excuse a polynomial one. A pattern
/// may open with one inline-flag group, `(?i)`, `(?m)`, `(?s)` or a
/// combination, which JS lacks; it is stripped here and returned as the
/// `RegExp` flags. On success, the pattern without that group and its flags.
pub(crate) fn check_regex_literal(
    literal: &str,
    span: Span,
) -> Result<(String, String), CompileError> {
    let label = format!("`\"{literal}\"`");
    let (pat, flags) = match leading_flag_group(literal) {
        Some((pat, flags)) => {
            let mut seen = String::new();
            for c in flags.chars() {
                if !"ims".contains(c) || seen.contains(c) {
                    return Err(CompileError::new(
                        "bynk.types.invalid_regex",
                        span,
                        format!("invalid inline flags `(?{flags})` in {label}"),
                    )
                    .with_note("the inline flags are `i`, `m` and `s`, each at most once"));
                }
                seen.push(c);
            }
            (pat.to_string(), seen)
        }
        None => (literal.to_string(), String::new()),
    };
    if let Err(e) = regress::Regex::with_flags(&pat, flags.as_str()) {
        return Err(CompileError::new(
            "bynk.types.invalid_regex",
            span,
            format!("invalid regular expression {label}"),
        )
        .with_note(format!("regex parse error (JS `RegExp` semantics): {e}")));
    }
    if has_nested_unbounded_quantifier(&pat) {
        return Err(CompileError::new(
            "bynk.types.catastrophic_regex",
            span,
            format!(
                "the pattern {label} nests unbounded quantifiers, which can cause \
                 catastrophic backtracking (ReDoS)"
            ),
        )
        .with_note(
            "a repeated group that itself contains `*`, `+`, or `{n,}` makes matching take \
             exponential time on crafted input; restructure the pattern so no unbounded \
             quantifier is nested inside another",
        ));
    }
    if let Some(e) = ambiguity_diagnostic(&pat, &flags, span, &label, None) {
        return Err(e);
    }
    Ok((pat, flags))
}

/// A leading `(?flags)` group of letters, split off: the rest of the pattern
/// and the letters. `None` when the literal opens with no such group (a
/// lookaround `(?=`, `(?!`, `(?<` or a non-capturing `(?:` is not one).
fn leading_flag_group(literal: &str) -> Option<(&str, &str)> {
    let rest = literal.strip_prefix("(?")?;
    let end = rest.find(')')?;
    let flags = &rest[..end];
    (!flags.is_empty() && flags.chars().all(|c| c.is_ascii_alphabetic()))
        .then(|| (&rest[end + 1..], flags))
}

/// K6c (#2600): a checked regex literal as the emitter writes it, the pattern
/// and its `RegExp` flags. The checker has already refused a malformed flag
/// group, so this only splits.
pub fn split_regex_flags(literal: &str) -> (String, String) {
    match leading_flag_group(literal) {
        Some((pat, flags)) => (pat.to_string(), flags.to_string()),
        None => (literal.to_string(), String::new()),
    }
}

/// #1651: how many steps a polynomially ambiguous pattern may take on its
/// longest admitted input and still only warn. At `10⁷` V8 matches in tens of
/// milliseconds: `\d*\d*\d*` takes about 23 ms at 256 characters.
const POLYNOMIAL_STEP_CAP: u128 = 10_000_000;

/// The largest length bound `n` with `n^degree` within [`POLYNOMIAL_STEP_CAP`]:
/// 3162 at degree 2, 215 at degree 3, 56 at degree 4.
fn largest_safe_bound(degree: u32) -> i64 {
    let mut n: i64 = 1;
    while (n as u128 + 1)
        .checked_pow(degree)
        .is_some_and(|s| s <= POLYNOMIAL_STEP_CAP)
    {
        n += 1;
    }
    n
}

/// #724 — detect one catastrophic-backtracking (ReDoS) signature: an unbounded
/// quantifier applied to a group that itself contains an unbounded quantifier
/// ("star height ≥ 2", e.g. `(a+)+`, `(a*)*b`, `((ab)+)+`). Under the JS
/// backtracking `RegExp` the emitted boundary check runs, this class takes
/// exponential time on a crafted near-miss input; the conservative structural
/// rule rejects it at compile time.
///
/// "Unbounded" means `*`, `+`, or `{n,}` (open upper bound). A bounded
/// quantifier is not flagged here, though one inside a loop can explode too
/// (`(a{2,3})+`); that is the ambiguity analysis's case. The scan is purely structural — the pattern is
/// already known valid (`regress` accepted it) — so it need not model match
/// semantics, only quantifier nesting through groups. Inner unbounded
/// quantifiers propagate up through *bounded* quantifiers too, so `((a+)?)+`
/// is still caught. The check is conservative in the safe direction: it can
/// reject a star-height-2 pattern whose sub-expressions provably never overlap,
/// but every *nested-quantifier* blowup is flagged.
///
/// This is a cheap first pass, not the whole exponential class. Ambiguous
/// alternation under a single quantifier (`(a|a)+`, `(\d|\d\d)+`), a bounded
/// quantifier inside a loop (`(a{2,3})+`) and the polynomial class
/// (`\d*\d*`) are star height 1 and pass here. `regex_ambiguity` (#1651)
/// decides them on the pattern's automaton, and runs only on patterns this
/// pass accepts, so this pass's clearer message wins where both apply.
fn has_nested_unbounded_quantifier(pat: &str) -> bool {
    // Precondition: `pat` is a valid regex (`regress` accepted it), so its
    // parentheses are balanced. The `stack.last_mut().unwrap()` arms below rely
    // on that — a `)` never pops the root frame — which the sole caller ensures
    // by running this only after the validity check. The `)` arm keeps a
    // defensive `unwrap_or` regardless.
    let chars: Vec<char> = pat.chars().collect();
    // One boolean per open group (index 0 = top level): does this group contain
    // an unbounded quantifier anywhere within it?
    let mut stack: Vec<bool> = vec![false];
    // The atom a following quantifier would apply to: `None` if none is pending,
    // else `Some(inner_unbounded)` where `inner_unbounded` is true when that atom
    // is a group carrying an unbounded quantifier inside it.
    let mut pending: Option<bool> = None;
    let mut i = 0;

    // Fold a pending atom that turned out to be unquantified into the current
    // frame: if it was a group with inner unbounded content, that content still
    // lives in the enclosing group.
    fn fold(pending: &mut Option<bool>, stack: &mut [bool]) {
        if pending.take() == Some(true) {
            *stack.last_mut().unwrap() = true;
        }
    }

    while i < chars.len() {
        match chars[i] {
            // An escape is a single atom; skip the escaped char.
            '\\' => {
                fold(&mut pending, &mut stack);
                i += 2;
                pending = Some(false);
            }
            // A character class is one atom; `*`/`+`/`{` inside it are literal.
            '[' => {
                fold(&mut pending, &mut stack);
                i += 1;
                if i < chars.len() && chars[i] == '^' {
                    i += 1;
                }
                // A leading `]` is a literal member, not the class terminator.
                if i < chars.len() && chars[i] == ']' {
                    i += 1;
                }
                while i < chars.len() && chars[i] != ']' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1; // consume the closing `]`
                pending = Some(false);
            }
            '(' => {
                fold(&mut pending, &mut stack);
                stack.push(false);
                i += 1;
                // Skip a group-type prefix so its punctuation is not mistaken for
                // an atom: `(?:`, `(?=`, `(?!`, `(?<=`, `(?<!`, `(?<name>`.
                if i < chars.len() && chars[i] == '?' {
                    i += 1;
                    if i < chars.len() && matches!(chars[i], ':' | '=' | '!') {
                        i += 1;
                    } else if i < chars.len() && chars[i] == '<' {
                        i += 1;
                        if i < chars.len() && matches!(chars[i], '=' | '!') {
                            i += 1;
                        } else {
                            while i < chars.len() && chars[i] != '>' {
                                i += 1;
                            }
                            if i < chars.len() {
                                i += 1; // consume `>`
                            }
                        }
                    }
                }
            }
            ')' => {
                fold(&mut pending, &mut stack);
                let closed = stack.pop().unwrap_or(false);
                i += 1;
                // The whole group is now an atom in its parent frame.
                pending = Some(closed);
            }
            // Alternation ends the current atom; an unbounded one folds in.
            '|' => {
                fold(&mut pending, &mut stack);
                i += 1;
            }
            // Unbounded quantifier.
            '*' | '+' => {
                let atom_unbounded = pending.take().unwrap_or(false);
                if atom_unbounded {
                    return true; // unbounded nested inside unbounded
                }
                *stack.last_mut().unwrap() = true;
                i += 1;
                if i < chars.len() && chars[i] == '?' {
                    i += 1; // lazy marker
                }
            }
            // Optional: bounded, but inner unbounded content still propagates.
            '?' => {
                if let Some(atom_unbounded) = pending.take() {
                    if atom_unbounded {
                        *stack.last_mut().unwrap() = true;
                    }
                    i += 1;
                    if i < chars.len() && chars[i] == '?' {
                        i += 1; // lazy marker
                    }
                } else {
                    // Stray `?` (unreachable for a valid pattern) — treat as atom.
                    i += 1;
                    pending = Some(false);
                }
            }
            '{' => {
                if let Some((unbounded_q, next)) = parse_brace_quantifier(&chars, i) {
                    let atom_unbounded = pending.take().unwrap_or(false);
                    if unbounded_q && atom_unbounded {
                        return true;
                    }
                    if unbounded_q || atom_unbounded {
                        *stack.last_mut().unwrap() = true;
                    }
                    i = next;
                    if i < chars.len() && chars[i] == '?' {
                        i += 1; // lazy marker
                    }
                } else {
                    // A `{` that is not a quantifier is a literal atom.
                    fold(&mut pending, &mut stack);
                    i += 1;
                    pending = Some(false);
                }
            }
            // Any other char (`.`, literal, `^`, `$`) is an ordinary atom.
            _ => {
                fold(&mut pending, &mut stack);
                i += 1;
                pending = Some(false);
            }
        }
    }
    false
}

/// Parse a `{m}`, `{m,}`, or `{m,n}` quantifier starting at `chars[start] == '{'`.
/// Returns `(is_unbounded, index_after_close)` when it is a well-formed
/// quantifier — `is_unbounded` is true only for `{m,}` (open upper bound) — or
/// `None` when the braces are not a quantifier (then they are literal text, as
/// JS `RegExp` treats them).
fn parse_brace_quantifier(chars: &[char], start: usize) -> Option<(bool, usize)> {
    let mut i = start + 1;
    let lo_start = i;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    if i == lo_start {
        return None; // `{m` requires at least one digit
    }
    let mut unbounded = false;
    if i < chars.len() && chars[i] == ',' {
        i += 1;
        let hi_start = i;
        while i < chars.len() && chars[i].is_ascii_digit() {
            i += 1;
        }
        if i == hi_start {
            unbounded = true; // `{m,}` — no upper bound
        }
    }
    if i < chars.len() && chars[i] == '}' {
        Some((unbounded, i + 1))
    } else {
        None
    }
}

#[cfg(test)]
mod redos_tests {
    use super::has_nested_unbounded_quantifier as redos;

    #[test]
    fn flags_nested_unbounded_quantifiers() {
        // The classic exponential shapes: an unbounded quantifier over a group
        // that itself repeats unboundedly.
        assert!(redos("(a+)+"));
        assert!(redos("(a+)+$"));
        assert!(redos("(a*)*"));
        assert!(redos("(a+)*"));
        assert!(redos("(a*)+"));
        assert!(redos("((ab)+)+"));
        assert!(redos("(a{1,})+")); // `{1,}` is unbounded
        assert!(redos("(a+){2,}")); // outer open bound over inner `+`
        assert!(redos("x(y(z+)+w)+")); // nested a level deep
        assert!(redos("(\\d+)+"));
        assert!(redos("(?:a+)+")); // non-capturing group
        assert!(redos("([a-z]+)*")); // class inside the repeated group
        assert!(redos("((a+)?)+")); // inner unbounded propagates through `?`
        assert!(redos("(a+|b)+")); // alternation does not launder the nesting
    }

    #[test]
    fn allows_safe_patterns() {
        // A single quantifier level is fine, however placed.
        assert!(!redos("a+"));
        assert!(!redos("(a+)")); // repeated once, not nested
        assert!(!redos("(a+)(b+)")); // siblings, not nested
        assert!(!redos("(ab)+")); // repeated group with no inner quantifier
        assert!(!redos("(a+)?")); // bounded outer quantifier
        assert!(!redos("(a+){2,3}")); // finite outer bound
        // Not flagged *here*, though exponential: a bounded quantifier inside a
        // loop is the ambiguity analysis's case (#1651), pinned below.
        assert!(!redos("(a{2,3})+"));
        assert!(!redos("[a-z]+")); // `+` binds the class, not a group
        assert!(!redos("a{2,}b{2,}")); // two unbounded, neither nested
    }

    #[test]
    fn the_ambiguity_analysis_covers_what_the_first_pass_does_not() {
        // #1651: star-height-1 shapes the first pass passes, and the ambiguity
        // analysis decides. `(foo|foobar)+` is unambiguous ({foo, foobar} is
        // uniquely decodable), so it is linear and accepted.
        use super::super::regex_ambiguity::{Verdict, analyse};
        for pat in [
            "(a|a)+",
            "(\\d|\\d\\d)+",
            "(a|aa)*",
            "(a{1,2})+",
            "(a{2,3})+",
        ] {
            assert!(!redos(pat), "the first pass is not meant to flag `{pat}`");
            assert!(
                matches!(analyse(pat), Verdict::Exponential(..)),
                "`{pat}` is exponential"
            );
        }
        assert!(matches!(analyse("\\d*\\d*"), Verdict::Polynomial(..)));
        assert_eq!(analyse("(foo|foobar)+"), Verdict::Linear);
    }

    #[test]
    fn allows_every_pattern_used_in_the_repo() {
        // Guard against a false positive on the `Matches` patterns the fixtures,
        // docs, and examples ship — none nests unbounded quantifiers.
        for pat in [
            "[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            "[a-z][a-z0-9-]*",
            "[A-Z]{3}-[0-9]{4}",
            "[A-Z]{3}",
            "[a-z]+",
            "[A-Z]+",
            "[a-z]+(?<=ing)",
            "[a-z0-9_]+",
            "[a-z0-9]{1,16}",
            "[a-z0-9]{1,8}",
            "[A-Z0-9]{3,16}",
            "[a-z0-9]{3,8}",
            "[a-zA-Z0-9]{6,8}",
            "ab|cd",
            "AUTH-[0-9]{8}",
            "AUTH-[0-9]+",
            "CUST-[0-9]+",
            "https?://.+",
            "ORD-[0-9]{6}",
            "ORD-[0-9]+",
            "SHP-[0-9]{8}",
            "T-[0-9]+",
            "[A-Z0-9]{8}",
            "[A-Z]{2}[0-9]{2}",
            "^[a-z]+$",
            "\\d{4}",
            "\\d+",
        ] {
            assert!(!redos(pat), "false positive on safe pattern `{pat}`");
            assert_eq!(
                super::super::regex_ambiguity::analyse(pat),
                super::super::regex_ambiguity::Verdict::Linear,
                "the ambiguity analysis rejects safe pattern `{pat}`"
            );
        }
        // `LocaleTag`'s pattern is read from the firstparty source rather than
        // duplicated here (its own single-source-of-truth rule, ADR 0279), so
        // fetch it live instead of hand-copying the string.
        let locale_pat = super::locale_tag_pattern().expect("LocaleTag has a Matches predicate");
        assert!(
            !redos(locale_pat),
            "false positive on safe pattern `{locale_pat}`"
        );
        assert_eq!(
            super::super::regex_ambiguity::analyse(locale_pat),
            super::super::regex_ambiguity::Verdict::Linear,
            "the ambiguity analysis rejects `LocaleTag`"
        );
    }
}

#[cfg(test)]
mod locale_tag_tests {
    use super::locale_tag_accepts;

    #[test]
    fn admits_the_pre_existing_shapes() {
        for tag in ["en", "pt-BR", "zh-Hans-CN", "es-419"] {
            assert!(locale_tag_accepts(tag), "expected `{tag}` to be admitted");
        }
    }

    #[test]
    fn admits_variants_extensions_private_use_and_extlang() {
        for tag in [
            "de-CH-1996",          // variant (digit-led)
            "ca-valencia",         // variant (alpha)
            "sl-rozaj",            // variant, shortest admitted length
            "en-scotland-fonipa",  // two variants
            "en-US-u-ca-buddhist", // extension
            "de-CH-x-phonebk",     // attached private-use
            "x-custom",            // standalone private-use
            "zh-yue",              // extlang
        ] {
            assert!(locale_tag_accepts(tag), "expected `{tag}` to be admitted");
        }
    }

    #[test]
    fn rejects_grandfathered_and_malformed_tags() {
        for tag in [
            "i-klingon",      // grandfathered irregular
            "en-GB-oed",      // grandfathered irregular
            "Klingon",        // not a tag at all
            "pt-br",          // region must be uppercase
            "en-SCOTLAND",    // variant must be lowercase
            "en-x-abcdefghi", // private-use subtag over 8 chars
            "x",              // private-use with no subtag
            "",
        ] {
            assert!(!locale_tag_accepts(tag), "expected `{tag}` to be rejected");
        }
    }
}
