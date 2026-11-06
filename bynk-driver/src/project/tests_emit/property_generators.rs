//! Property-test value generators and canonicalisers.
//!
//! Crate-hygiene D1 (#2982, PR 2): split out of `tests_emit.rs`.

use super::*;
// #1759: every namespace a test module imports, from one definition.
use bynk_check::checker::Types;
use bynk_check::test_suites::{self};
use bynk_syntax::ast::{BaseType, PredKind, Refinement, TypeBody, TypeDecl};
use bynk_ts::{TsArrowBody, TsExpr, TsLit, TsParam, TsType};
use std::sync::Arc;

/// Integer generation bounds `(lo, hi, floor)` derived from a refinement: `lo`
/// and `hi` bound the random draw (and the boundary values), `floor` is the
/// shrink target. Unrefined `Int` draws over `[-1000, 1000]` toward `0`.
pub(super) fn int_bounds(refinement: Option<&Refinement>) -> (i64, i64, i64) {
    let mut lo = -1000i64;
    let mut hi = 1000i64;
    let mut floor = 0i64;
    if let Some(r) = refinement {
        for p in &r.predicates {
            match &p.kind {
                PredKind::Positive => {
                    lo = lo.max(1);
                    floor = floor.max(1);
                }
                PredKind::NonNegative => {
                    lo = lo.max(0);
                    floor = floor.max(0);
                }
                PredKind::InRange(a, b) => {
                    lo = a.value;
                    hi = b.value;
                    floor = a.value;
                }
                _ => {}
            }
        }
    }
    if lo > hi {
        hi = lo;
    }
    if floor < lo {
        floor = lo;
    }
    (lo, hi, floor)
}

/// Float generation bounds `(lo, hi)` from a refinement (`lo` doubles as the
/// shrink target). Unrefined `Float` draws over `[-1000, 1000]`.
pub(super) fn float_bounds(refinement: Option<&Refinement>) -> (f64, f64) {
    let mut lo = -1000.0f64;
    let mut hi = 1000.0f64;
    if let Some(r) = refinement {
        for p in &r.predicates {
            match &p.kind {
                PredKind::Positive => lo = lo.max(1.0),
                PredKind::NonNegative => lo = lo.max(0.0),
                PredKind::InRangeF(a, b) => {
                    lo = a.value;
                    hi = b.value;
                }
                _ => {}
            }
        }
    }
    if lo > hi {
        hi = lo;
    }
    (lo, hi)
}

/// Minimum string length a refinement demands (`0` if none).
pub(super) fn str_min(refinement: Option<&Refinement>) -> i64 {
    let mut min = 0i64;
    if let Some(r) = refinement {
        for p in &r.predicates {
            match p.kind {
                PredKind::NonEmpty => min = min.max(1),
                PredKind::MinLength(k) | PredKind::Length(k) => min = min.max(k),
                _ => {}
            }
        }
    }
    min
}

/// A canonical (deterministic) TypeScript literal inhabiting a base type — used
/// for opaque `.unsafe` wrapping of the exotic bases (`Duration`, `Bytes`, …).
pub(super) fn base_canon(base: BaseType) -> String {
    match base {
        BaseType::Int => "0n".to_string(),
        BaseType::String => "\"\"".to_string(),
        BaseType::Bool => "true".to_string(),
        BaseType::Float => "0".to_string(),
        BaseType::Duration | BaseType::Instant => "0".to_string(),
        BaseType::Bytes => "new globalThis.Uint8Array()".to_string(),
        BaseType::Decimal => "{ coefficient: 0n, scale: 0 }".to_string(),
    }
}

/// `Decimal` generation bounds `(lo, hi, scale)` as coefficients at `scale`,
/// from a refinement (#2622). Unrefined draws over `[-1000.00, 1000.00]`.
pub(super) fn decimal_bounds(refinement: Option<&Refinement>) -> (i128, i128, u32) {
    let mut scale = 2u32;
    if let Some(r) = refinement {
        for p in &r.predicates {
            if let PredKind::InRangeD(a, b) = &p.kind {
                scale = scale.max(a.scale).max(b.scale);
            }
        }
    }
    let at = |coef: &str, from: u32| -> Option<i128> {
        coef.parse::<i128>()
            .ok()?
            .checked_mul(10i128.checked_pow(scale - from)?)
    };
    let (mut lo, mut hi) = (-100_000i128, 100_000i128);
    if let Some(r) = refinement {
        for p in &r.predicates {
            match &p.kind {
                PredKind::Positive => lo = lo.max(1),
                PredKind::NonNegative => lo = lo.max(0),
                PredKind::InRangeD(a, b) => {
                    if let (Some(x), Some(y)) = (at(&a.coef, a.scale), at(&b.coef, b.scale)) {
                        lo = x;
                        hi = y;
                    }
                }
                _ => {}
            }
        }
    }
    if lo > hi {
        hi = lo;
    }
    (lo, hi, scale)
}

/// A TypeScript expression drawing a random inhabitant of `base` within
/// `refinement`'s domain, unbranded — the draw [`refined_gen_ts`] brands, and
/// the draw a record field with an inline `where` uses as is.
pub(super) fn base_draw_ts(base: BaseType, refinement: Option<&Refinement>) -> TsExpr {
    match base {
        BaseType::Int => {
            let (lo, hi, _) = int_bounds(refinement);
            method_call(
                ident("rng"),
                "int",
                vec![num_lit(format!("{lo}n")), num_lit(format!("{hi}n"))],
            )
        }
        BaseType::Float => {
            let (lo, hi) = float_bounds(refinement);
            method_call(
                ident("rng"),
                "float",
                vec![num_lit(lo.to_string()), num_lit(hi.to_string())],
            )
        }
        BaseType::String => {
            let min = str_min(refinement);
            method_call(
                ident("rng"),
                "str",
                vec![num_lit(min.to_string()), num_lit((min + 8).to_string())],
            )
        }
        BaseType::Bool => method_call(ident("rng"), "bool", vec![]),
        // #2622: a random coefficient at a fixed scale.
        BaseType::Decimal => {
            let (lo, hi, scale) = decimal_bounds(refinement);
            ident(format!(
                "{{ coefficient: {}, scale: {scale} }}",
                bynk_ts::print_expr(&method_call(
                    ident("rng"),
                    "int",
                    vec![num_lit(format!("{lo}n")), num_lit(format!("{hi}n"))],
                ))
            ))
        }
        _ => ident(base_canon(base)),
    }
}

/// The canonical (boundary) inhabitant of `base` within `refinement`'s domain,
/// unbranded: the refinement's floor, or a string of its minimum length.
pub(super) fn base_canon_ts(base: BaseType, refinement: Option<&Refinement>) -> TsExpr {
    match base {
        BaseType::Int => {
            let (lo, _, _) = int_bounds(refinement);
            num_lit(format!("{lo}n"))
        }
        BaseType::Float => {
            let (lo, _) = float_bounds(refinement);
            num_lit(lo.to_string())
        }
        BaseType::String => {
            let min = str_min(refinement);
            str_lit("x".repeat(min.max(0) as usize))
        }
        BaseType::Bool => TsExpr::Lit(TsLit::Bool(true)),
        BaseType::Decimal => {
            let (lo, hi, scale) = decimal_bounds(refinement);
            // The value closest to zero the refinement admits.
            let c = 0i128.clamp(lo, hi);
            ident(format!("{{ coefficient: {c}n, scale: {scale} }}"))
        }
        other => ident(base_canon(other)),
    }
}

/// A TypeScript expression that draws a random inhabitant of `base` (refined by
/// `refinement`), branded to `<name>` — via `<name>.unsafe(...)` for an opaque
/// type, or an inline `as` cast for a refined/alias type (ADR 0182).
pub(super) fn refined_gen_ts(
    name: &str,
    base: BaseType,
    refinement: Option<&Refinement>,
    is_opaque: bool,
) -> TsExpr {
    let draw = base_draw_ts(base, refinement);
    // `unchecked_construct_test` is also called from `lower.rs`'s own
    // permanently-excluded general expression lowering (Third correction) and
    // must keep its exact `-> String` signature — its own branching (a real
    // `.unsafe(...)` call vs a real `(... as any)` cast) is not duplicated here
    // as a second node-building path; its already-formed text is carried as an
    // opaque `Ident` instead, the same "an unconverted shared sibling's own
    // text stays opaque" posture this track already uses (P7.9's `ts_base`,
    // #1369's `Cell` zero value).
    ident(emitter::unchecked_construct_test(
        name,
        &bynk_ts::print_expr(&draw),
        is_opaque,
    ))
}

/// Coerce a Sum-variant-payload/Record-field value to `number` when its own
/// resolved type draws/emits as `bigint` — a bare `Int`, or a refined/opaque
/// `Int` (#1398). `gen_ts_for_ty`'s/`canon_ts_for_ty`'s own top-level
/// `Base(Int)` arm draws/emits `bigint` — correct there, since a top-level
/// `for all` binding's own boundaries/shrink machinery is bigint-typed
/// throughout (the "P7.2: deferred, not narrowed" representation gap, #1426
/// tracks its own further consequences). But a Sum variant's real compiled
/// constructor and a Record's real compiled object shape both type an `Int`
/// field `number` (`ts_type_ref_to_ts_type`'s own mapping) — including a
/// refined/opaque one, which erases to `number & { __brand }`, not `bigint`
/// — so a bigint-drawing payload/field value recursed into from *inside* a
/// Sum/Record arm needs this coercion the top-level arm itself must not
/// have. A refined/opaque `Int` (`Percent`, etc.) routes through
/// `refined_gen_ts`'s/its `canon_ts_for_ty` sibling's own
/// `unchecked_construct_test`-wrapped cast (`(rng.int(...) as any)` /
/// `Pct.unsafe(rng.int(...))`) — that cast is compile-time only and does
/// nothing to the runtime value, so it still needs this same coercion
/// (review of #1428): confirmed the exact `TypeError: Cannot mix BigInt and
/// other types` this issue names still reproduces for a refined-Int record
/// field without it. `Number(...)` around either wrapper form is safe:
/// `.unsafe`/`as any` are both identity at runtime, never anything more than
/// a re-tag. Every other type's own recursive result is already
/// number/string/bool/object-shaped, so this stays a no-op there. And the
/// depth-exhaustion `undefined` sentinel (`canon_ts_for_ty`'s own `depth ==
/// 0` early return, reachable through a self-recursive Sum whose *first*
/// variant carries an `Int` payload — `prop_binding_generable` validates only
/// that variant, `bynk-check/src/test_suites.rs`) is passed through
/// unwrapped rather than turned into a silent `Number(undefined)` (`NaN`) —
/// also review of #1428.
pub(super) fn coerce_int_field(
    t: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
    value: TsExpr,
) -> TsExpr {
    if matches!(&value, TsExpr::Ident(s) if s == "undefined") {
        return value;
    }
    if ty_draws_bigint(t, types, tys) {
        call(ident("globalThis.Number"), vec![value])
    } else {
        value
    }
}

/// Does `t` draw/emit as a JS `bigint` — a bare `Int`, or a refined/opaque
/// type over one? The shared predicate behind [`coerce_int_field`]'s own
/// `Number(…)`-wrap decision and [`destructure_vals`]'s "does this binding
/// set need coercion at all" check (#1426).
pub(super) fn ty_draws_bigint(
    t: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> bool {
    match &*tys.get(t) {
        checker::Ty::Base(BaseType::Int) => true,
        checker::Ty::Named { name, .. } => matches!(
            types.get(name).map(|d| &d.body),
            Some(TypeBody::Refined {
                base: BaseType::Int,
                ..
            }) | Some(TypeBody::Opaque {
                base: BaseType::Int,
                ..
            })
        ),
        _ => false,
    }
}

/// Coerce a top-level `for all`/attack-parameter binding's own drawn value to
/// `number` (#1426) — like [`coerce_int_field`], but for the bind site rather
/// than a nested Sum/Record field, which needs a genuinely different result
/// shape for a *named* refined/opaque `Int`, not just the same `Number(…)`
/// wrap. Review of #1432, finding 1: a bare `Int` really does compile to
/// plain `number` everywhere, so `Number(value)` alone is correct and
/// sufficient there. But a refined/opaque `Int` (`Percent`, etc.) compiles to
/// a *branded* type (`number & { readonly __brand: "Percent" }`,
/// `emit_refined_type`) — a plain `Number(value)` local's own inferred type
/// is bare `number`, not the brand, so passing it to a real function
/// expecting the named type is a genuine new `tsc --strict` `TS2345` error
/// this fix would otherwise introduce (confirmed: reverting to a bare
/// `Number(…)` wrap here reproduces it for a minimal `fn label(p: Percent)`
/// repro, which compiled and ran fine pre-#1426 since the pre-fix local was
/// `any`, not a concrete type). Closed the same way `refined_gen_ts` already
/// keeps a branded draw compiling — wrap the coerced `Number(…)` in `(… as
/// any)`, restoring the escape hatch a plain array-destructured `any[]`
/// element always had for this shape, while still fixing the *runtime* value
/// (`as any` is compile-time only; the `Number(…)` underneath still runs).
/// Every other type's own value (including a bare `Int`) is unaffected —
/// only a *named* refined/opaque `Int` binding gets the extra wrap.
pub(super) fn coerce_top_level_int_binding(
    t: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
    value: TsExpr,
) -> TsExpr {
    let is_named_int = matches!(
        &*tys.get(t),
        checker::Ty::Named { .. } if ty_draws_bigint(t, types, tys)
    );
    let coerced = coerce_int_field(t, types, tys, value);
    if is_named_int {
        TsExpr::As {
            expr: Box::new(coerced),
            ty: TsType::named("any"),
        }
    } else {
        coerced
    }
}

/// Destructure `__vals` into its named locals, coercing each `Int`-drawing
/// (bare, refined, or opaque) binding's own runtime value to `number` at the
/// bind site (#1426) via [`coerce_top_level_int_binding`]. `binding_gen`'s
/// own boundaries/shrink machinery for a top-level `for all`/attack-parameter
/// `Int` binding is deliberately `bigint`-typed throughout —
/// `coerce_int_field`'s own doc comment names this as correct, since the
/// shrink loop's `current[i]`/`vals[i]` elements must keep matching what
/// `gens[i].shrink(v)` expects — but the *local* a predicate/attacked-function
/// body evaluates ordinary Bynk `Int` arithmetic against must not stay
/// `bigint`, or `n + 1` throws `TypeError: Cannot mix BigInt and other types`
/// the instant it meets a `number` literal.  Never previously hit: every
/// existing fixture's predicate either avoided arithmetic on a drawn `Int`
/// entirely, or routed through a refined type's own compile-time-only `as
/// any` cast (`refined_gen_ts`), which suppresses the *static* `tsc` error,
/// not the *runtime* one. Reuses `coerce_int_field`'s own "does this type
/// draw bigint" check — the same representation gap #1398/#1428 already
/// closed for record/sum-field values, just at the binding site itself
/// rather than a nested field; `binding_types[i]` is `None` when the
/// binding's own type didn't resolve (mirrors `gens`' own
/// `unwrap_or`/`undefined`-sentinel fallback above — `__vals[i]` is
/// `undefined` there regardless of what type it "should" have been, so it
/// passes through unwrapped either way).
///
/// Falls back to the original plain `const [a, b, ...] = __vals;` array
/// destructure when *no* binding actually needs coercion — a record/sum/
/// string/bool binding (the common case) draws its own top-level value
/// already `number`/`string`/`boolean`/object-shaped, only a bare or
/// refined/opaque `Int` binding draws `bigint` — so the common case emits
/// byte-identical text to before this fix, and only a fixture with a real
/// top-level `Int`-drawing binding sees its own destructure change shape.
pub(super) fn destructure_vals(
    names: &[String],
    binding_types: &[Option<checker::TyId>],
    types: Option<&HashMap<String, Arc<TypeDecl>>>,
    tys: &Arc<Types>,
) -> String {
    let needs_coercion = binding_types
        .iter()
        .any(|ty| matches!((ty, types), (Some(t), Some(types)) if ty_draws_bigint(*t, types, tys)));
    if !needs_coercion {
        return format!("const [{}] = __vals;", names.join(", "));
    }
    names
        .iter()
        .zip(binding_types)
        .enumerate()
        .map(|(i, (name, ty))| {
            let raw = TsExpr::Index {
                object: Box::new(ident("__vals")),
                index: Box::new(num_lit(i.to_string())),
            };
            let value = match (ty, types) {
                (Some(t), Some(types)) => coerce_top_level_int_binding(*t, types, tys, raw),
                _ => raw,
            };
            format!("const {name} = {};", bynk_ts::print_expr(&value))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A TypeScript expression drawing a random inhabitant of a resolved type using
/// the in-scope `rng` — the property generator (DECISION P: a type is its own
/// inhabitant space). Sums pick a random variant; records generate every field.
pub(super) fn gen_ts_for_ty(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    depth: u32,
    tys: &Arc<Types>,
) -> TsExpr {
    if depth == 0 {
        // #1429: a bare `1` here starved `canon_ts_for_ty`'s own recursion
        // one level short of what `prop_binding_generable` (`bynk-check`)
        // already promised was reachable *for the top-level bound type*.
        // Both functions share the same recursion shape — `depth == 0`
        // checked unconditionally at entry, one unit spent per `Sum`/
        // `Record` step down into a field's own type, `Ty::Base` never
        // itself gating on `depth` — so a type `prop_binding_generable`
        // accepts within `PROP_GEN_DEPTH` is guaranteed to bottom out in
        // `canon_ts_for_ty` within that same budget: a self-recursive type
        // like `T = Base(n: Int) | Cons(tail: T)` previously exhausted
        // this fallback's own budget one field short of `Base`'s terminal
        // `n: Int`, landing a bare `undefined` inside a real, `number`-
        // typed constructor argument (`T.Base(undefined)`, `tsc TS2345`).
        //
        // Review of #1434, finding 1: `prop_binding_generable` only ever
        // descends a `Sum`'s *first* variant, but this function's own Sum
        // branch (below) builds a thunk for *every* variant — so `ty` here
        // can be a type the checker never validated at all (reached only
        // through a non-first variant, e.g. `Nope(b: Bad)` on
        // `T = Plain(n: Int) | Nope(b: Bad)`, where `Bad = Node(l: Bad, r:
        // Bad) | Leaf(n: Int)` is itself first-variant-unterminating). For
        // such a type hand it the full `PROP_GEN_DEPTH` budget regardless,
        // and `canon_ts_for_ty`'s own "always expand the first variant"
        // strategy walks a full branching tree before ever reaching the
        // `Leaf` escape — confirmed to reproduce a real multi-second,
        // 170MB+ emit for exactly this shape (a plausible emit-time
        // hang/OOM, not merely an invalid file) once this fallback started
        // handing out 12 levels of budget instead of 1.
        //
        // So: only spend the full budget when the checker actually
        // promised `ty` itself bottoms out within it: that's exactly what
        // `prop_binding_generable` decides, and — same "first variant
        // only, `.all` short-circuits on the first failing field" shape as
        // `canon_ts_for_ty` — costs at most `PROP_GEN_DEPTH` recursive
        // calls to answer, not a tree walk. Any other type falls back to
        // the original, deliberately conservative `1` — the same "produces
        // a small invalid file fast" behaviour this fallback always had
        // for a type the checker never vouched for, not a regression.
        if test_suites::prop_binding_generable(ty, types, test_suites::PROP_GEN_DEPTH, tys) {
            return canon_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys);
        }
        return canon_ts_for_ty(ty, types, 1, tys);
    }
    match &*tys.get(ty) {
        checker::Ty::Base(BaseType::Int) => {
            let (lo, hi, _) = int_bounds(None);
            method_call(
                ident("rng"),
                "int",
                vec![num_lit(format!("{lo}n")), num_lit(format!("{hi}n"))],
            )
        }
        checker::Ty::Base(BaseType::String) => {
            method_call(ident("rng"), "str", vec![num_lit("0"), num_lit("8")])
        }
        checker::Ty::Base(BaseType::Bool) => method_call(ident("rng"), "bool", vec![]),
        checker::Ty::Base(BaseType::Float) => method_call(
            ident("rng"),
            "float",
            vec![num_lit("-1000"), num_lit("1000")],
        ),
        checker::Ty::Base(b) => ident(base_canon(*b)),
        checker::Ty::Named { name, .. } => {
            let Some(decl) = types.get(name) else {
                return ident("undefined");
            };
            match &decl.body {
                TypeBody::Refined {
                    base, refinement, ..
                }
                | TypeBody::Opaque {
                    base, refinement, ..
                } => refined_gen_ts(
                    name,
                    *base,
                    refinement.as_ref(),
                    matches!(decl.body, TypeBody::Opaque { .. }),
                ),
                TypeBody::Sum(s) => {
                    let thunks: Vec<TsExpr> = s
                        .variants
                        .iter()
                        .map(|v| {
                            let body = if v.payload.is_empty() {
                                member(ident(name), &v.name.name)
                            } else {
                                let args: Vec<TsExpr> = v
                                    .payload
                                    .iter()
                                    .map(|f| {
                                        checker::resolve_type_ref(&f.type_ref, types, tys)
                                            .map(|t| {
                                                coerce_int_field(
                                                    t,
                                                    types,
                                                    tys,
                                                    gen_ts_for_ty(t, types, depth - 1, tys),
                                                )
                                            })
                                            .unwrap_or_else(|| ident("undefined"))
                                    })
                                    .collect();
                                call(member(ident(name), &v.name.name), args)
                            };
                            TsExpr::Arrow {
                                params: Vec::new(),
                                is_async: false,
                                generics: Vec::new(),
                                return_type: None,
                                body: Box::new(TsArrowBody::Expr(Box::new(body))),
                            }
                        })
                        .collect();
                    if thunks.is_empty() {
                        ident("undefined")
                    } else {
                        method_call(ident("rng"), "pick", vec![TsExpr::array(thunks)])
                    }
                }
                TypeBody::Record(r) => {
                    let fields: Vec<(String, TsExpr)> = r
                        .fields
                        .iter()
                        .map(|f| {
                            let g = checker::resolve_type_ref(&f.type_ref, types, tys)
                                .map(|t| {
                                    let v = match (&*tys.get(t), f.refinement.as_ref()) {
                                        // #2667: an inline `where` on a base-typed
                                        // field draws from its own domain, as a
                                        // declared refined type over that base does.
                                        (checker::Ty::Base(b), Some(r)) => {
                                            base_draw_ts(*b, Some(r))
                                        }
                                        _ => gen_ts_for_ty(t, types, depth - 1, tys),
                                    };
                                    coerce_int_field(t, types, tys, v)
                                })
                                .unwrap_or_else(|| ident("undefined"));
                            (f.name.name.clone(), g)
                        })
                        .collect();
                    // The same `"{  }"` double-space quirk `workers.rs`/
                    // `project.rs`/`emit.rs` already carry as opaque text
                    // (#1321/#1327/#1390): the pre-conversion `format!("{{
                    // {} }}", fields.join(", "))` template always has a space
                    // on each side of its `{}` slot, so zero fields literally
                    // produced a double space, not the tight `"{}"`
                    // `TsExpr::object`'s own empty-entries shortcut renders.
                    // A zero-field record is exotic but not provably
                    // unreachable — guarded rather than assumed.
                    if fields.is_empty() {
                        ident("{  }")
                    } else {
                        TsExpr::object(fields)
                    }
                }
            }
        }
        _ => ident("undefined"),
    }
}

/// A canonical (deterministic, boundary) inhabitant of a resolved type — the
/// boundary value the runner draws first (refinement floor / minimum length /
/// first variant), and the shrink target for sums.
pub(super) fn canon_ts_for_ty(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    depth: u32,
    tys: &Arc<Types>,
) -> TsExpr {
    if depth == 0 {
        return ident("undefined");
    }
    match &*tys.get(ty) {
        checker::Ty::Base(BaseType::Int) => num_lit("0n"),
        checker::Ty::Base(BaseType::String) => str_lit(""),
        checker::Ty::Base(BaseType::Bool) => TsExpr::Lit(TsLit::Bool(true)),
        checker::Ty::Base(BaseType::Float) => num_lit("0"),
        checker::Ty::Base(b) => ident(base_canon(*b)),
        checker::Ty::Named { name, .. } => {
            let Some(decl) = types.get(name) else {
                return ident("undefined");
            };
            match &decl.body {
                TypeBody::Refined {
                    base, refinement, ..
                }
                | TypeBody::Opaque {
                    base, refinement, ..
                } => {
                    let lit = base_canon_ts(*base, refinement.as_ref());
                    ident(emitter::unchecked_construct_test(
                        name,
                        &bynk_ts::print_expr(&lit),
                        matches!(decl.body, TypeBody::Opaque { .. }),
                    ))
                }
                TypeBody::Sum(s) => match s.variants.first() {
                    None => ident("undefined"),
                    Some(v) if v.payload.is_empty() => member(ident(name), &v.name.name),
                    Some(v) => {
                        let args: Vec<TsExpr> = v
                            .payload
                            .iter()
                            .map(|f| {
                                checker::resolve_type_ref(&f.type_ref, types, tys)
                                    .map(|t| {
                                        coerce_int_field(
                                            t,
                                            types,
                                            tys,
                                            canon_ts_for_ty(t, types, depth - 1, tys),
                                        )
                                    })
                                    .unwrap_or_else(|| ident("undefined"))
                            })
                            .collect();
                        call(member(ident(name), &v.name.name), args)
                    }
                },
                TypeBody::Record(r) => {
                    let fields: Vec<(String, TsExpr)> = r
                        .fields
                        .iter()
                        .map(|f| {
                            let g = checker::resolve_type_ref(&f.type_ref, types, tys)
                                .map(|t| {
                                    let v = match (&*tys.get(t), f.refinement.as_ref()) {
                                        // #2667: an inline `where` on a base-typed
                                        // field draws from its own domain, as a
                                        // declared refined type over that base does.
                                        (checker::Ty::Base(b), Some(r)) => {
                                            base_canon_ts(*b, Some(r))
                                        }
                                        _ => canon_ts_for_ty(t, types, depth - 1, tys),
                                    };
                                    coerce_int_field(t, types, tys, v)
                                })
                                .unwrap_or_else(|| ident("undefined"));
                            (f.name.name.clone(), g)
                        })
                        .collect();
                    // Same `"{  }"` double-space quirk as `gen_ts_for_ty`'s
                    // own Record branch above — see its comment for why.
                    if fields.is_empty() {
                        ident("{  }")
                    } else {
                        TsExpr::object(fields)
                    }
                }
            }
        }
        _ => ident("undefined"),
    }
}

/// The generator descriptor for one `for all` binding: boundary values, a random
/// generator, and a shrinker (all real TypeScript expressions, evaluated in the
/// runner's scope where the type constructors are in scope).
pub(super) struct BindingGen {
    pub(super) boundaries: Vec<TsExpr>,
    pub(super) gen_ts: TsExpr,
    pub(super) shrink: TsExpr,
}

/// Build the generator descriptor for a binding whose resolved type is `ty`.
pub(super) fn binding_gen(
    ty: checker::TyId,
    types: &HashMap<String, Arc<TypeDecl>>,
    tys: &Arc<Types>,
) -> BindingGen {
    let gen_ts = gen_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys);
    let (boundaries, shrink) = match &*tys.get(ty) {
        checker::Ty::Base(BaseType::Int) => {
            let (lo, hi, floor) = int_bounds(None);
            (
                vec![
                    num_lit(format!("{floor}n")),
                    num_lit(format!("{hi}n")),
                    num_lit(format!("{lo}n")),
                ],
                call(
                    ident("__bynkShrinkInt"),
                    vec![ident("v"), num_lit(format!("{floor}n"))],
                ),
            )
        }
        checker::Ty::Base(BaseType::String) => (
            vec![str_lit("")],
            call(ident("__bynkShrinkString"), vec![ident("v"), num_lit("0")]),
        ),
        checker::Ty::Base(BaseType::Bool) => (
            vec![
                TsExpr::Lit(TsLit::Bool(true)),
                TsExpr::Lit(TsLit::Bool(false)),
            ],
            TsExpr::Paren(Box::new(cond_expr(
                ident("v"),
                TsExpr::array(vec![TsExpr::Lit(TsLit::Bool(false))]),
                TsExpr::array(vec![]),
            ))),
        ),
        checker::Ty::Named { name, .. } => match types.get(name).map(|d| &d.body) {
            Some(TypeBody::Refined {
                base, refinement, ..
            })
            | Some(TypeBody::Opaque {
                base, refinement, ..
            }) => {
                // ADR 0182: brand via `.unsafe` for opaque, inline `as` cast for
                // refined/alias. The or-pattern can't capture which variant, so
                // re-derive it by name.
                let is_opaque = matches!(
                    types.get(name).map(|d| &d.body),
                    Some(TypeBody::Opaque { .. })
                );
                match base {
                    BaseType::Int => {
                        let (lo, hi, floor) = int_bounds(refinement.as_ref());
                        let shrunk = emitter::unchecked_construct_test(name, "__n", is_opaque);
                        (
                            vec![
                                ident(emitter::unchecked_construct_test(
                                    name,
                                    &format!("{lo}n"),
                                    is_opaque,
                                )),
                                ident(emitter::unchecked_construct_test(
                                    name,
                                    &format!("{hi}n"),
                                    is_opaque,
                                )),
                            ],
                            method_call(
                                call(
                                    ident("__bynkShrinkInt"),
                                    vec![ident("v"), num_lit(format!("{floor}n"))],
                                ),
                                "map",
                                vec![TsExpr::Arrow {
                                    params: vec![TsParam {
                                        name: "__n".to_string(),
                                        ty: Some(TsType::named("bigint")),
                                        optional: false,
                                    }],
                                    is_async: false,
                                    generics: Vec::new(),
                                    return_type: None,
                                    body: Box::new(TsArrowBody::Expr(Box::new(ident(shrunk)))),
                                }],
                            ),
                        )
                    }
                    BaseType::String => {
                        let min = str_min(refinement.as_ref());
                        let lit = format!("\"{}\"", "x".repeat(min.max(0) as usize));
                        let shrunk = emitter::unchecked_construct_test(name, "__s", is_opaque);
                        (
                            vec![ident(emitter::unchecked_construct_test(
                                name, &lit, is_opaque,
                            ))],
                            method_call(
                                call(
                                    ident("__bynkShrinkString"),
                                    vec![ident("v"), num_lit(min.to_string())],
                                ),
                                "map",
                                vec![TsExpr::Arrow {
                                    params: vec![TsParam {
                                        name: "__s".to_string(),
                                        ty: Some(TsType::named("string")),
                                        optional: false,
                                    }],
                                    is_async: false,
                                    generics: Vec::new(),
                                    return_type: None,
                                    body: Box::new(TsArrowBody::Expr(Box::new(ident(shrunk)))),
                                }],
                            ),
                        )
                    }
                    _ => (
                        vec![canon_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys)],
                        TsExpr::array(vec![]),
                    ),
                }
            }
            Some(TypeBody::Sum(_)) => {
                let canon = canon_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys);
                (vec![canon.clone()], TsExpr::array(vec![canon]))
            }
            _ => (
                vec![canon_ts_for_ty(ty, types, test_suites::PROP_GEN_DEPTH, tys)],
                TsExpr::array(vec![]),
            ),
        },
        _ => (Vec::new(), TsExpr::array(vec![])),
    };
    BindingGen {
        boundaries,
        gen_ts,
        shrink,
    }
}

/// `{ [name: "…",] boundaries: […], gen: (rng: any) => …, shrink: (v: any) => …,
/// show: (v: any) => __bynkShow(v) }` — the one generator-descriptor object
/// shape [`emit_test_property_function`]/[`emit_test_history_property_function`]/
/// [`emit_contract_attack_function`] each build once per binding/param, now
/// directly from `binding_gen`'s own real `TsExpr` fields (Arc C slice A,
/// #1395) instead of printing them back to text and re-embedding via
/// `format!` — the seam those three functions' own doc comments named as
/// "not yet converted" is closed here. `name` is absent for a history
/// handler's own per-param generator (no `name` key at all in that shape).
pub(super) fn gen_descriptor_entry(name: Option<TsExpr>, bg: &BindingGen) -> TsExpr {
    let mut entries: Vec<(String, TsExpr)> = Vec::new();
    if let Some(n) = name {
        entries.push(("name".to_string(), n));
    }
    entries.push((
        "boundaries".to_string(),
        TsExpr::array(bg.boundaries.clone()),
    ));
    entries.push((
        "gen".to_string(),
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "rng".to_string(),
                ty: Some(TsType::named("any")),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            // #1397: an unparenthesised object-literal arrow body parses as
            // a block (`{ a: … }` reads `a:` as a statement label, and a
            // bare `{  }` reads as an empty block) — for a record-typed
            // binding, `bg.gen_ts` is `TsExpr::Object` for a non-empty
            // record, or `gen_ts_for_ty`'s own `"{  }"` sentinel `TsExpr::
            // Ident` (its Record arm's double-space-quirk comment) for a
            // zero-field one. Wrap either in `TsExpr::Paren` here, the one
            // choke point all three call sites share. The `Ident` arm
            // matches by leading `{` rather than the sentinel's exact
            // double-space spelling (review of #1425): a valid identifier
            // can never start with `{`, so this is still a no-op for every
            // other `gen_ts` shape, but it keeps matching if that
            // formatting quirk is ever normalised away (the same cleanup
            // #1321/#1327/#1390 already invite for its other two copies).
            body: Box::new(TsArrowBody::Expr(Box::new(match &bg.gen_ts {
                o @ TsExpr::Object { .. } => TsExpr::Paren(Box::new(o.clone())),
                o @ TsExpr::Ident(s) if s.trim_start().starts_with('{') => {
                    TsExpr::Paren(Box::new(o.clone()))
                }
                e => e.clone(),
            }))),
        },
    ));
    entries.push((
        "shrink".to_string(),
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "v".to_string(),
                ty: Some(TsType::named("any")),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(bg.shrink.clone()))),
        },
    ));
    entries.push((
        "show".to_string(),
        TsExpr::Arrow {
            params: vec![TsParam {
                name: "v".to_string(),
                ty: Some(TsType::named("any")),
                optional: false,
            }],
            is_async: false,
            generics: Vec::new(),
            return_type: None,
            body: Box::new(TsArrowBody::Expr(Box::new(call(
                ident("__bynkShow"),
                vec![ident("v")],
            )))),
        },
    ));
    TsExpr::object(entries)
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{base, named};
    use super::*;
    use bynk_syntax::ast::Trivia;
    use bynk_syntax::ast::{BaseType, Ident, TypeRef};
    use bynk_syntax::span::Span;

    fn refined_int_decl(name: &str) -> TypeDecl {
        TypeDecl {
            derives: Vec::new(),
            derives_kw: None,
            name: Ident {
                name: name.to_string(),
                span: Span::default(),
            },
            type_params: Vec::new(),
            body: TypeBody::Refined {
                base: BaseType::Int,
                base_span: Span::default(),
                refinement: None,
            },
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
            private: None,
        }
    }

    fn opaque_int_decl(name: &str) -> TypeDecl {
        TypeDecl {
            derives: Vec::new(),
            derives_kw: None,
            name: Ident {
                name: name.to_string(),
                span: Span::default(),
            },
            type_params: Vec::new(),
            body: TypeBody::Opaque {
                base: BaseType::Int,
                base_span: Span::default(),
                refinement: None,
            },
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
            private: None,
        }
    }

    #[test]
    fn coerce_int_field_wraps_a_bare_int_draw_in_number() {
        let tys = Arc::new(Types::new());
        let int_ty = tys.intern(checker::Ty::Base(BaseType::Int));
        let types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        let draw = ident("rng.int(-1000n, 1000n)");

        let coerced = coerce_int_field(int_ty, &types, &tys, draw);
        assert_eq!(
            bynk_ts::print_expr(&coerced),
            "globalThis.Number(rng.int(-1000n, 1000n))"
        );
    }

    #[test]
    fn coerce_int_field_wraps_a_refined_int_field_too() {
        // Review of #1428: `refined_gen_ts`'s own `(value as any)` cast is
        // compile-time only — the runtime value underneath is still a real
        // bigint, so a refined-Int payload/field needs the same coercion a
        // bare Int does, or the exact `TypeError: Cannot mix BigInt and
        // other types` #1398 names still reproduces one type-alias away.
        let tys = Arc::new(Types::new());
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        types.insert("Pct".to_string(), Arc::new(refined_int_decl("Pct")));
        let pct_ty = tys.intern(checker::Ty::Named {
            name: "Pct".to_string(),
            kind: checker::NamedKind::Refined(BaseType::Int),
            args: Vec::new(),
        });
        let draw = ident("(rng.int(0n, 100n) as any)");

        let coerced = coerce_int_field(pct_ty, &types, &tys, draw);
        assert_eq!(
            bynk_ts::print_expr(&coerced),
            "globalThis.Number((rng.int(0n, 100n) as any))"
        );
    }

    #[test]
    fn coerce_int_field_wraps_an_opaque_int_field_too() {
        let tys = Arc::new(Types::new());
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        types.insert("Pct".to_string(), Arc::new(opaque_int_decl("Pct")));
        let pct_ty = tys.intern(checker::Ty::Named {
            name: "Pct".to_string(),
            kind: checker::NamedKind::Opaque(BaseType::Int),
            args: Vec::new(),
        });
        let draw = ident("Pct.unsafe(rng.int(0n, 100n))");

        let coerced = coerce_int_field(pct_ty, &types, &tys, draw);
        assert_eq!(
            bynk_ts::print_expr(&coerced),
            "globalThis.Number(Pct.unsafe(rng.int(0n, 100n)))"
        );
    }

    #[test]
    fn coerce_int_field_passes_the_undefined_sentinel_through_unwrapped() {
        // Review of #1428: the depth-exhaustion `undefined` sentinel
        // (`canon_ts_for_ty`'s own `depth == 0` early return) must not
        // become `Number(undefined)` — a silent `NaN` where the unwrapped
        // sentinel at least fails loudly (a separate, pre-existing gap
        // tracked as #1429, not fixed here).
        let tys = Arc::new(Types::new());
        let int_ty = tys.intern(checker::Ty::Base(BaseType::Int));
        let types: HashMap<String, Arc<TypeDecl>> = HashMap::new();

        let coerced = coerce_int_field(int_ty, &types, &tys, ident("undefined"));
        assert_eq!(bynk_ts::print_expr(&coerced), "undefined");
    }

    #[test]
    fn coerce_int_field_leaves_a_non_int_value_untouched() {
        let tys = Arc::new(Types::new());
        let string_ty = tys.intern(checker::Ty::Base(BaseType::String));
        let types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        let draw = ident("rng.str(0, 8)");

        let coerced = coerce_int_field(string_ty, &types, &tys, draw);
        assert_eq!(bynk_ts::print_expr(&coerced), "rng.str(0, 8)");
    }

    // -- gen_ts_for_ty's depth-exhaustion fallback (#1429, review of #1434) --

    fn sum_decl(name: &str, variants: Vec<(&str, Vec<(&str, TypeRef)>)>) -> TypeDecl {
        TypeDecl {
            derives: Vec::new(),
            derives_kw: None,
            name: Ident {
                name: name.to_string(),
                span: Span::default(),
            },
            type_params: Vec::new(),
            body: TypeBody::Sum(bynk_syntax::ast::SumBody {
                variants: variants
                    .into_iter()
                    .map(|(vname, fields)| bynk_syntax::ast::Variant {
                        documentation: None,
                        name: Ident {
                            name: vname.to_string(),
                            span: Span::default(),
                        },
                        payload: fields
                            .into_iter()
                            .map(|(fname, type_ref)| bynk_syntax::ast::VariantField {
                                documentation: None,
                                name: Ident {
                                    name: fname.to_string(),
                                    span: Span::default(),
                                },
                                type_ref,
                                span: Span::default(),
                            })
                            .collect(),
                        span: Span::default(),
                        trivia: Default::default(),
                    })
                    .collect(),
                embeds: Vec::new(),
                span: Span::default(),
                trailing_comments: Default::default(),
            }),
            documentation: None,
            span: Span::default(),
            trivia: Trivia::default(),
            private: None,
        }
    }

    #[test]
    fn gen_ts_for_ty_bottoms_out_a_first_variant_recursive_sum_the_checker_validated() {
        // #1429's own repro: `T = Base(n: Int) | Cons(tail: T)`. The checker
        // accepts `T` as a `for all` binding (`prop_binding_generable` only
        // needs the first variant, `Base`, to be generable) — the fallback
        // must be able to actually build a `Base(n: Int)` once generation
        // depth is exhausted deep inside a `Cons` chain, not `undefined`.
        let tys = Arc::new(Types::new());
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        types.insert(
            "T".to_string(),
            Arc::new(sum_decl(
                "T",
                vec![
                    ("Base", vec![("n", base(BaseType::Int))]),
                    ("Cons", vec![("tail", named("T"))]),
                ],
            )),
        );
        let t_ty = tys.intern(checker::Ty::Named {
            name: "T".to_string(),
            kind: checker::NamedKind::Sum,
            args: Vec::new(),
        });

        // depth: 0 forces the fallback immediately, the same state
        // `gen_ts_for_ty`'s own recursion reaches once its budget runs out.
        let generated = gen_ts_for_ty(t_ty, &types, 0, &tys);
        let printed = bynk_ts::print_expr(&generated);
        assert!(
            !printed.contains("undefined"),
            "a checker-validated recursive sum must not fall back to `undefined`, got: {printed}"
        );
        assert!(
            printed.contains("T.Base("),
            "expected the fallback to build T's own first (validated) variant, got: {printed}"
        );
    }

    #[test]
    fn gen_ts_for_ty_stays_bounded_for_a_branching_variant_the_checker_never_validated() {
        // Review of #1434, finding 1: `prop_binding_generable` only ever
        // descends a sum's *first* variant, but `gen_ts_for_ty`'s own Sum
        // branch builds a thunk for *every* variant — so a non-first
        // variant's field type can reach the depth-0 fallback fully
        // unvalidated. `Bad`'s own first variant (`Node`) is branching and
        // never bottoms out on its own; handing the fallback the full
        // `PROP_GEN_DEPTH` budget for `Bad` (not just `T`, the type the
        // checker actually validated) would walk a full binary tree before
        // ever reaching `Leaf`'s escape — confirmed to reproduce a real
        // ~176MB, 50+ second emit for this exact shape before the guard
        // below. The fallback must recognise `Bad` itself was never
        // validated and fall back to the original, small budget for it.
        let tys = Arc::new(Types::new());
        let mut types: HashMap<String, Arc<TypeDecl>> = HashMap::new();
        types.insert(
            "Bad".to_string(),
            Arc::new(sum_decl(
                "Bad",
                vec![
                    ("Node", vec![("l", named("Bad")), ("r", named("Bad"))]),
                    ("Leaf", vec![("n", base(BaseType::Int))]),
                ],
            )),
        );
        types.insert(
            "T".to_string(),
            Arc::new(sum_decl(
                "T",
                vec![
                    ("Plain", vec![("n", base(BaseType::Int))]),
                    ("Nope", vec![("b", named("Bad"))]),
                ],
            )),
        );
        let t_ty = tys.intern(checker::Ty::Named {
            name: "T".to_string(),
            kind: checker::NamedKind::Sum,
            args: Vec::new(),
        });

        // depth: 1 reaches `Nope`'s own `b: Bad` field at depth 0 — `T`
        // itself is still checker-validated (via `Plain`), so this isn't
        // testing the depth-0-at-the-top-level case above, only what
        // happens once the *nested*, unvalidated `Bad` field is reached.
        let generated = gen_ts_for_ty(t_ty, &types, 1, &tys);
        let printed = bynk_ts::print_expr(&generated);
        // A full `PROP_GEN_DEPTH`-deep expansion of `Bad`'s own branching
        // first variant is tens of thousands of characters at minimum
        // (confirmed empirically: ~200KB unguarded vs. ~200 bytes guarded,
        // for the equivalent whole-fixture emit); a generous bound well
        // below that catches a regression without pinning an exact byte
        // count against unrelated formatting changes.
        assert!(
            printed.len() < 2_000,
            "expected a bounded fallback for Bad's own unvalidated branching \
             variant, got {} chars (a regression here likely means the depth-0 \
             fallback stopped checking prop_binding_generable before spending \
             the full PROP_GEN_DEPTH budget): {printed}",
            printed.len()
        );
    }
}
