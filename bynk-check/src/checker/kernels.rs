//! Kernel methods and collection / JSON statics.
//!
//! Split out of `checker.rs` (v0.29.10) verbatim; the parent module
//! re-exports these via `use kernels::*`.

use super::*;
use crate::kernel_methods::{KernelMethod, LIST_METHODS, QUERY_METHODS};

/// Render a kernel's method names for a `method_not_found` message —
/// `` `map`, `filter`, … `` — generated from a `KernelMethod` table rather
/// than hand-listed at each call site, so the message can't drift from the
/// table the way `Query`'s once did (T1.6′).
fn kernel_vocabulary(methods: &[KernelMethod]) -> String {
    methods
        .iter()
        .map(|k| format!("`{}`", k.name))
        .collect::<Vec<_>>()
        .join(", ")
}

/// v0.20b: `List.empty()` / `Map.empty()` — the built-in collection statics.
/// Their element/key/value types are exactly as uninferable as an empty
/// `[]`, so they share `bynk.types.uninferable_element_type`. The resolver
/// has already rejected any static other than `empty`.
pub(crate) fn check_collection_static(
    type_name: &Ident,
    method: &Ident,
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // K6a (#2596): `Set.of(xs)` builds a set from a list; its element type
    // comes from the list, or from the expected `Set`.
    if type_name.name == SET && method.name == "of" {
        return check_set_of(args, span, expected, ctx);
    }
    for a in args {
        let _ = type_of(a, None, ctx);
    }
    if method.name != "empty" {
        // The resolver owns the unknown-static diagnostic; don't double up.
        return None;
    }
    if !args.is_empty() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`{}.empty` takes no arguments", type_name.name),
        ));
        return None;
    }
    let inferred = match type_name.name.as_str() {
        LIST => expected
            .and_then(|t| peel_to_list(t, tys))
            .map(|t| tys.intern(Ty::List(t))),
        SET => expected
            .and_then(|t| peel_to_set(t, tys))
            .map(|t| tys.intern(Ty::Set(t))),
        _ => expected
            .and_then(|t| peel_to_map(t, tys))
            .map(|(k, v)| tys.intern(Ty::Map(k, v))),
    };
    if inferred.is_none() {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.uninferable_element_type",
                span,
                format!(
                    "`{}.empty()` has no expected type to infer its type arguments from",
                    type_name.name
                ),
            )
            .with_note(
                "annotate the binding (`let xs: List[T] = List.empty()`) or use it where the collection type is expected",
            ),
        );
    }
    inferred
}

/// K6a (#2596): `Set.of(xs: List[T]) -> Set[T]`. An inferred element type
/// never passes the resolver's `Set[T]` check, so the key rule is applied here.
fn check_set_of(args: &[Expr], span: Span, expected: Option<TyId>, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`Set.of` takes 1 argument, got {}", args.len()),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    // An expected element still holding a type variable (a generic `Set[T]`
    // parameter) is no expectation yet: the list literal infers its own.
    let expected_list = expected
        .and_then(|t| peel_to_set(t, tys))
        .filter(|t| !contains_var(*t, tys))
        .map(|t| tys.intern(Ty::List(t)));
    let list = type_of(&args[0], expected_list, ctx)?;
    let Some(elem) = peel_to_list(list, tys) else {
        ctx.errors.push(CompileError::new(
            "bynk.types.type_mismatch",
            args[0].span,
            format!(
                "the `Set.of` argument has type `{}`, but a `List` is required",
                list.display(tys)
            ),
        ));
        return None;
    };
    require_set_element(elem, args[0].span, ctx);
    Some(tys.intern(Ty::Set(elem)))
}

/// K6a (#2596): a `Set`'s element must be value-keyable (the `Map`-key rule),
/// since a JS `Set` compares anything else by identity.
fn require_set_element(elem: TyId, span: Span, ctx: &mut Ctx) {
    let tys = ctx.tys;
    if !is_keyable(elem, tys) && !matches!(&*tys.get(elem), Ty::Var(_)) {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.unkeyable_set_element",
                span,
                format!(
                    "a `Set` element must be value-keyable — `String`, `Int`, or a refined/opaque type over them — not `{}`",
                    elem.display(tys)
                ),
            )
            .with_note(
                "record, sum, collection, and function elements are rejected; value-equality elements need bounded generics",
            ),
        );
    }
}

/// K6a (#2596): the `Set` kernel — `length`, `contains`, `insert`, `remove`
/// and `toList`. Every operation returns a new set; none mutates.
pub(crate) fn check_set_kernel_method(
    method: &Ident,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let sig: Option<(Vec<TyId>, Ty)> = match method.name.as_str() {
        "length" => Some((vec![], Ty::Base(BaseType::Int))),
        "contains" => Some((vec![elem], Ty::Base(BaseType::Bool))),
        "insert" | "remove" => Some((vec![elem], Ty::Set(elem))),
        "toList" => Some((vec![], Ty::List(elem))),
        _ => None,
    };
    let Some((params, ret)) = sig else {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "the built-in `Set[{}]` type has no method `{}` — the kernel is `length`, `contains`, `insert`, `remove`, `toList` (and `bynk.set` has `union`, `intersect`, …)",
                elem.display(tys),
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Set.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (a, p) in args.iter().zip(params.iter()) {
        check_arg(a, *p, &format!("the `Set.{}` element", method.name), ctx);
    }
    Some(tys.intern(ret))
}

/// v0.88 (ADR 0116 D2): the closed orderable base set for `sortBy`/`min`/`max`.
/// A key widens through `Ty::base()` (Refined → base; Opaque does not widen,
/// so an opaque key — whose base is hidden — is not orderable). `Instant`
/// joins this set with slice 1b.
fn is_orderable(t: TyId, tys: &Types) -> bool {
    matches!(
        t.base(tys),
        Some(
            BaseType::Int
                | BaseType::Float
                | BaseType::String
                | BaseType::Duration
                | BaseType::Instant
        )
    )
}

/// v0.88 (ADR 0116 D3): the numeric keys `sum`/`average` accept.
fn is_numeric(t: TyId, tys: &Types) -> bool {
    matches!(
        t.base(tys),
        Some(BaseType::Int | BaseType::Float | BaseType::Duration)
    )
}

/// v0.88: value-keyable for `distinct`/`distinctBy` — the Map-key rule
/// (ADR 0110 D5): `Int`/`String`, including Refined *and* Opaque over them
/// (dedup is by value *equality*, which an opaque id supports even though it
/// does not widen for ordering).
fn is_keyable(t: TyId, tys: &Types) -> bool {
    match &*tys.get(t) {
        Ty::Base(BaseType::Int | BaseType::String) => true,
        Ty::Named {
            kind: NamedKind::Refined(b) | NamedKind::Opaque(b),
            ..
        } => matches!(b, BaseType::Int | BaseType::String),
        _ => false,
    }
}

fn require_orderable(key: TyId, method: &str, span: Span, ctx: &mut Ctx) {
    let tys = ctx.tys;
    if !is_orderable(key, tys) {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.key_not_orderable",
                span,
                format!(
                    "`{method}` needs an orderable key, but the key function returns `{}`",
                    key.display(tys)
                ),
            )
            .with_note(
                "orderable keys are `Int`, `Float`, `String`, `Duration`, `Instant` (and refined types over them)",
            ),
        );
    }
}

fn require_numeric(key: TyId, method: &str, span: Span, ctx: &mut Ctx) {
    let tys = ctx.tys;
    if !is_numeric(key, tys) {
        ctx.errors.push(
            CompileError::new(
                "bynk.query.sum_needs_numeric",
                span,
                format!(
                    "`{method}` needs a numeric key, but the key function returns `{}`",
                    key.display(tys)
                ),
            )
            .with_note("numeric keys are `Int`, `Float`, `Duration` (and refined types over them)"),
        );
    }
}

fn require_keyable(key: TyId, method: &str, span: Span, ctx: &mut Ctx) {
    let tys = ctx.tys;
    if !is_keyable(key, tys) {
        ctx.errors.push(
            CompileError::new(
                "bynk.types.unkeyable_distinct",
                span,
                format!(
                    "`{method}` needs a value-keyable element/key, but got `{}`",
                    key.display(tys)
                ),
            )
            .with_note(
                "value-keyable types are `Int`, `String`, or a refined/opaque type over them",
            ),
        );
    }
}

/// v0.94 (ADR 0120): the element type `U` of a join's `other` collection — it
/// MUST match the receiver's shape (`List` joins `List`, `Query` joins `Query`).
fn join_other_elem(other: TyId, shape_is_query: bool, tys: &Types) -> Option<TyId> {
    match (shape_is_query, &*tys.get(other)) {
        (false, Ty::List(e)) => Some(*e),
        (true, Ty::Query(e)) => Some(*e),
        _ => None,
    }
}

/// v0.94 (ADR 0116/0120): the wrong join-method arity, with the args still typed
/// for error recovery.
fn join_arity_err(kind: &str, name: &str, want: usize, args: &[Expr], span: Span, ctx: &mut Ctx) {
    ctx.errors.push(CompileError::new(
        "bynk.types.method_arity",
        span,
        format!("`{kind}.{name}` takes {want} arguments, got {}", args.len()),
    ));
    for a in args {
        let _ = type_of(a, None, ctx);
    }
}

/// v0.94 (ADR 0120): the shared checker for `joinOn`/`leftJoin` — an `other` of
/// the receiver's shape, `left: T -> K` / `right: U -> K` (value-keyable and
/// matching), and a combiner `into: (T, U) -> V` (or `(T, Option[U]) -> V` for
/// `leftJoin`). Returns the result element `V`; the caller wraps it (`List[V]`
/// eager / `Query[V]` lazy). Arguments are positional (labelled call arguments
/// are a deferred general feature — ADR 0120).
fn check_equi_join(
    args: &[Expr],
    elem: TyId,
    span: Span,
    shape_is_query: bool,
    is_left: bool,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let kind = if shape_is_query { "Query" } else { "List" };
    let name = if is_left { "leftJoin" } else { "joinOn" };
    if args.len() != 4 {
        join_arity_err(kind, name, 4, args, span, ctx);
        return None;
    }
    let other = type_of(&args[0], None, ctx)?;
    let Some(u) = join_other_elem(other, shape_is_query, tys) else {
        ctx.errors.push(CompileError::new(
            "bynk.types.argument_mismatch",
            args[0].span,
            format!(
                "`{kind}.{name}` joins another `{kind}`, but got `{}`",
                other.display(tys)
            ),
        ));
        return None;
    };
    let left = check_kernel_fn_arg(
        &args[1],
        vec![elem],
        &format!("the `{kind}.{name}` left key"),
        ctx,
    )?;
    require_keyable(left, &format!("{kind}.{name}"), args[1].span, ctx);
    let right = check_kernel_fn_arg(
        &args[2],
        vec![u],
        &format!("the `{kind}.{name}` right key"),
        ctx,
    )?;
    if !compatible(left, right, tys) {
        ctx.errors.push(CompileError::new(
            "bynk.query.join_key_mismatch",
            args[2].span,
            format!(
                "`{kind}.{name}` join keys must have the same type — the left key is `{}` but the right key is `{}`",
                left.display(tys),
                right.display(tys)
            ),
        ));
    }
    let other_param = if is_left {
        tys.intern(Ty::Option(u))
    } else {
        u
    };
    let v = check_kernel_fn_arg(
        &args[3],
        vec![elem, other_param],
        &format!("the `{kind}.{name}` combiner"),
        ctx,
    )?;
    Some(v)
}

/// v0.94 (ADR 0120): `join(other, on: (T, U) -> Bool, into: (T, U) -> V)` — a
/// predicate (nested-loop) join. Returns `V`.
fn check_pred_join(
    args: &[Expr],
    elem: TyId,
    span: Span,
    shape_is_query: bool,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let kind = if shape_is_query { "Query" } else { "List" };
    if args.len() != 3 {
        join_arity_err(kind, "join", 3, args, span, ctx);
        return None;
    }
    let other = type_of(&args[0], None, ctx)?;
    let Some(u) = join_other_elem(other, shape_is_query, tys) else {
        ctx.errors.push(CompileError::new(
            "bynk.types.argument_mismatch",
            args[0].span,
            format!(
                "`{kind}.join` joins another `{kind}`, but got `{}`",
                other.display(tys)
            ),
        ));
        return None;
    };
    let on = Ty::Fn {
        params: vec![elem, u],
        ret: tys.intern(Ty::Base(BaseType::Bool)),
    };
    check_arg(
        &args[1],
        tys.intern(on.clone()),
        &format!("the `{kind}.join` predicate"),
        ctx,
    );
    let v = check_kernel_fn_arg(
        &args[2],
        vec![elem, u],
        &format!("the `{kind}.join` combiner"),
        ctx,
    )?;
    Some(v)
}

/// v0.94 (ADR 0116 D7 / 0120): `groupBy(key: T -> K, into: (K, List[T]) -> V)` —
/// partition by a value-keyable key, projecting each group through `into`.
/// Returns `V`.
fn check_group_by(
    args: &[Expr],
    elem: TyId,
    span: Span,
    shape_is_query: bool,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let kind = if shape_is_query { "Query" } else { "List" };
    if args.len() != 2 {
        join_arity_err(kind, "groupBy", 2, args, span, ctx);
        return None;
    }
    let key = check_kernel_fn_arg(
        &args[0],
        vec![elem],
        &format!("the `{kind}.groupBy` key"),
        ctx,
    )?;
    require_keyable(key, &format!("{kind}.groupBy"), args[0].span, ctx);
    let v = check_kernel_fn_arg(
        &args[1],
        vec![key, tys.intern(Ty::List(elem))],
        &format!("the `{kind}.groupBy` combiner"),
        ctx,
    )?;
    Some(v)
}

/// v0.20b: type a built-in `List[T]` kernel method. The fold accumulator is
/// inferred from the `init` argument, then the step function checks against
/// the fully-instantiated function type (params type contextually, v0.20a).
pub(crate) fn check_list_kernel_method(
    method: &Ident,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let arity = |n: usize, ctx: &mut Ctx| {
        if args.len() != n {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`List.{}` takes {n} argument{}, got {}",
                    method.name,
                    if n == 1 { "" } else { "s" },
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    match method.name.as_str() {
        "length" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::Int)))
        }
        "get" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(
                &args[0],
                tys.intern(Ty::Base(BaseType::Int)),
                "the `List.get` index",
                ctx,
            );
            Some(tys.intern(Ty::Option(elem)))
        }
        "prepend" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(&args[0], elem, "the `List.prepend` element", ctx);
            Some(tys.intern(Ty::List(elem)))
        }
        // #1889: `append(x)` mirrors `prepend`, putting the item on the end.
        // Pure and non-mutating: it returns a new list.
        "append" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(&args[0], elem, "the `List.append` element", ctx);
            Some(tys.intern(Ty::List(elem)))
        }
        // #1889: `concat(other)` joins two lists of one element type, the
        // receiver's elements first. `other` checks against the receiver's
        // `List[T]`, so an empty `[]` argument takes its `T` from it.
        "concat" => {
            if !arity(1, ctx) {
                return None;
            }
            let list = tys.intern(Ty::List(elem));
            check_arg(&args[0], list, "the `List.concat` argument", ctx);
            Some(list)
        }
        "fold" => {
            if !arity(2, ctx) {
                return None;
            }
            let acc = type_of(&args[0], None, ctx)?;
            let step = Ty::Fn {
                params: vec![acc, elem],
                ret: acc,
            };
            check_arg(
                &args[1],
                tys.intern(step.clone()),
                "the `List.fold` step function",
                ctx,
            );
            Some(acc)
        }
        FOLD_EFF => {
            if !arity(2, ctx) {
                return None;
            }
            let acc = type_of(&args[0], None, ctx)?;
            let step = Ty::Fn {
                params: vec![acc, elem],
                ret: tys.intern(Ty::Effect(acc)),
            };
            check_arg(
                &args[1],
                tys.intern(step.clone()),
                "the `List.foldEff` step function",
                ctx,
            );
            // `foldEff` runs its effectful step function — like any
            // effectful function-value call, it is confined to effectful
            // contexts (0031).
            if !ctx.effectful {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.effect.fn_value_in_pure_context",
                        span,
                        "`List.foldEff` runs an effectful step function and cannot be called in a pure context",
                    )
                    .with_note(
                        "effectful function values may only be called where the enclosing body is effectful (its return type is an Effect)",
                    ),
                );
            }
            Some(tys.intern(Ty::Effect(acc)))
        }
        // v0.146 (ADR 0170): `forEach(f: T -> Effect[()]) -> Effect[()]` runs an
        // effectful step for each element **in turn**; v0.147 (ADR 0171):
        // `parTraverse` runs them **concurrently** (the fan-out form) and awaits
        // them together. Same type — the difference is sequential-vs-parallel
        // lowering. Both are the `List` analogue of the `Query` terminals; like
        // `foldEff`, each runs an effectful function value and so is confined to
        // effectful contexts.
        FOR_EACH | PAR_TRAVERSE => {
            if !arity(1, ctx) {
                return None;
            }
            let f = Ty::Fn {
                params: vec![elem],
                ret: tys.intern(Ty::Effect(tys.intern(Ty::Unit))),
            };
            check_arg(
                &args[0],
                tys.intern(f.clone()),
                &format!("the `List.{}` function", method.name),
                ctx,
            );
            if !ctx.effectful {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.effect.fn_value_in_pure_context",
                        span,
                        format!(
                            "`List.{}` runs an effectful function and cannot be called in a pure context",
                            method.name
                        ),
                    )
                    .with_note(
                        "effectful function values may only be called where the enclosing body is effectful (its return type is an Effect)",
                    ),
                );
            }
            Some(tys.intern(Ty::Effect(tys.intern(Ty::Unit))))
        }
        // v0.148 (ADR 0172): the collect-all effectful iterators.
        // `traverseAll(f: A -> Effect[Result[B, E]]) -> Effect[List[Result[B, E]]]`
        // runs `f` over each element **in order**; `parTraverseAll` runs them
        // **concurrently**. Neither short-circuits: every element's outcome
        // (`Ok` or `Err`) is collected into the result list — a `Result` `Err` is
        // a value, not a fault, so nothing is discarded. Like `forEach`, each
        // runs an effectful function value and is confined to effectful contexts.
        TRAVERSE_ALL | PAR_TRAVERSE_ALL => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(
                &args[0],
                vec![elem],
                &format!("the `List.{}` function", method.name),
                ctx,
            )?;
            // The function must return `Effect[Result[B, E]]`; peel to the
            // `Result[B, E]` element the result list collects.
            let result_ty = match &*tys.get(ret) {
                Ty::Effect(inner) => match &*tys.get(*inner) {
                    Ty::Result(_, _) => *inner,
                    _ => {
                        ctx.errors.push(CompileError::new(
                            "bynk.types.combinator_return_mismatch",
                            args[0].span,
                            format!(
                                "the `List.{}` function must return `Effect[Result[B, E]]`, but returns `Effect[{}]`",
                                method.name,
                                inner.display(tys)
                            ),
                        ));
                        return None;
                    }
                },
                _ => {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.combinator_return_mismatch",
                        args[0].span,
                        format!(
                            "the `List.{}` function must return `Effect[Result[B, E]]`, but returns `{}`",
                            method.name,
                            ret.display(tys)
                        ),
                    ));
                    return None;
                }
            };
            if !ctx.effectful {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.effect.fn_value_in_pure_context",
                        span,
                        format!(
                            "`List.{}` runs an effectful function and cannot be called in a pure context",
                            method.name
                        ),
                    )
                    .with_note(
                        "effectful function values may only be called where the enclosing body is effectful (its return type is an Effect)",
                    ),
                );
            }
            Some(tys.intern(Ty::Effect(tys.intern(Ty::List(result_ty)))))
        }
        // v0.150 (ADR 0174): the **short-circuit** collect iterators.
        // `traverseTry(f: A -> Effect[Result[B, E]]) -> Effect[Result[List[B], E]]`
        // runs `f` over each element **in order**, stopping at the first `Err` and
        // returning it; `parTraverseTry` issues all concurrently, awaits them, and
        // returns the first `Err` in input order (or `Ok` of the collected values).
        // The fault-propagating counterpart to `traverseAll` (which gathers every
        // outcome). Like `forEach`, each runs an effectful function value and is
        // confined to effectful contexts.
        TRAVERSE_TRY | PAR_TRAVERSE_TRY => {
            if !arity(1, ctx) {
                return None;
            }
            let (ok_ty, err_ty) = check_try_fn_arg(&args[0], elem, &method.name, "List", ctx)?;
            if !ctx.effectful {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.effect.fn_value_in_pure_context",
                        span,
                        format!(
                            "`List.{}` runs an effectful function and cannot be called in a pure context",
                            method.name
                        ),
                    )
                    .with_note(
                        "effectful function values may only be called where the enclosing body is effectful (its return type is an Effect)",
                    ),
                );
            }
            Some(tys.intern(Ty::Effect(
                tys.intern(Ty::Result(tys.intern(Ty::List(ok_ty)), err_ty)),
            )))
        }
        // v0.88 (ADR 0116, query-algebra slice 1): the eager in-memory builder
        // and terminal vocabulary as kernel methods. Lazy storage queries reuse
        // these names over a `Query[T]` receiver (slice 2); here every chain is
        // eager — builders return the collection, terminals return `T`.
        "map" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(&args[0], vec![elem], "the `List.map` function", ctx)?;
            Some(tys.intern(Ty::List(ret)))
        }
        "filter" => {
            if !arity(1, ctx) {
                return None;
            }
            let p = Ty::Fn {
                params: vec![elem],
                ret: tys.intern(Ty::Base(BaseType::Bool)),
            };
            check_arg(
                &args[0],
                tys.intern(p.clone()),
                "the `List.filter` predicate",
                ctx,
            );
            Some(tys.intern(Ty::List(elem)))
        }
        "flatMap" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret =
                check_kernel_fn_arg(&args[0], vec![elem], "the `List.flatMap` function", ctx)?;
            match &*tys.get(ret) {
                Ty::List(_) => Some(ret),
                _ => {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.combinator_return_mismatch",
                        args[0].span,
                        format!(
                            "the `List.flatMap` function must return a `List`, but returns `{}`",
                            ret.display(tys)
                        ),
                    ));
                    None
                }
            }
        }
        "take" | "skip" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(
                &args[0],
                tys.intern(Ty::Base(BaseType::Int)),
                &format!("the `List.{}` count", method.name),
                ctx,
            );
            Some(tys.intern(Ty::List(elem)))
        }
        "count" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::Int)))
        }
        "any" | "all" => {
            if !arity(1, ctx) {
                return None;
            }
            let p = Ty::Fn {
                params: vec![elem],
                ret: tys.intern(Ty::Base(BaseType::Bool)),
            };
            check_arg(
                &args[0],
                tys.intern(p.clone()),
                &format!("the `List.{}` predicate", method.name),
                ctx,
            );
            Some(tys.intern(Ty::Base(BaseType::Bool)))
        }
        "first" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(tys.intern(Ty::Option(elem)))
        }
        "firstOrElse" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(&args[0], elem, "the `List.firstOrElse` fallback", ctx);
            Some(elem)
        }
        // v0.88 (ADR 0116 D2/D3/D4): ordering and aggregate vocabulary. The
        // key is the projection's return type; orderable/numeric widen through
        // `Ty::base()` (so Opaque keys, whose base is hidden, are rejected —
        // ordering an opaque id is meaningless). Empty aggregates are total:
        // min/max/average -> Option, sum -> the zero (D4).
        "sortBy" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(&args[0], vec![elem], "the `List.sortBy` key", ctx)?;
            require_orderable(key, "List.sortBy", args[0].span, ctx);
            Some(tys.intern(Ty::List(elem)))
        }
        "distinct" => {
            if !arity(0, ctx) {
                return None;
            }
            require_keyable(elem, "List.distinct", span, ctx);
            Some(tys.intern(Ty::List(elem)))
        }
        "distinctBy" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(&args[0], vec![elem], "the `List.distinctBy` key", ctx)?;
            require_keyable(key, "List.distinctBy", args[0].span, ctx);
            Some(tys.intern(Ty::List(elem)))
        }
        "sum" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(&args[0], vec![elem], "the `List.sum` key", ctx)?;
            require_numeric(key, "List.sum", args[0].span, ctx);
            Some(key)
        }
        "min" | "max" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(
                &args[0],
                vec![elem],
                &format!("the `List.{}` key", method.name),
                ctx,
            )?;
            require_orderable(key, &format!("List.{}", method.name), args[0].span, ctx);
            Some(tys.intern(Ty::Option(key)))
        }
        "average" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(&args[0], vec![elem], "the `List.average` key", ctx)?;
            require_numeric(key, "List.average", args[0].span, ctx);
            // D3: average of a Duration is a Duration (integer-rounded millis);
            // average of Int/Float is a Float (no truncation).
            let result = match key.base(tys) {
                Some(BaseType::Duration) => Ty::Base(BaseType::Duration),
                _ => Ty::Base(BaseType::Float),
            };
            Some(tys.intern(Ty::Option(tys.intern(result))))
        }
        // v0.94 (ADR 0116/0120): joins & grouping, combiner form. Each projects
        // its result through `into` (no pair type); the result is `List[V]`.
        "joinOn" => Some(tys.intern(Ty::List(check_equi_join(
            args, elem, span, false, false, ctx,
        )?))),
        "leftJoin" => Some(tys.intern(Ty::List(check_equi_join(
            args, elem, span, false, true, ctx,
        )?))),
        "join" => Some(tys.intern(Ty::List(check_pred_join(args, elem, span, false, ctx)?))),
        "groupBy" => Some(tys.intern(Ty::List(check_group_by(args, elem, span, false, ctx)?))),
        // v0.119 (ADR 0155, DECISION C-a): `run.upTo(step)` — the history strictly
        // before `step`. A history-scoped accessor: admitted only when the element
        // is a synthesised history `Step`, so it never leaks onto ordinary lists
        // (it collapses into a general `List` slice if one ever lands). Returns the
        // prefix as an ordinary `List[Step]`.
        "upTo"
            if matches!(&*tys.get(elem), Ty::Named { name, kind: NamedKind::Record, .. }
                if name.starts_with("__History_") && name.ends_with("_Step")) =>
        {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(&args[0], elem, "the `History.upTo` step", ctx);
            Some(tys.intern(Ty::List(elem)))
        }
        _ => {
            // Generated from `LIST_METHODS` (v0.30.2, ADR 0063) rather than
            // hand-listed here, so the two can't drift the way this message
            // and the table's `join`/`joinOn`/`leftJoin`/`groupBy` once did.
            let kernel = kernel_vocabulary(LIST_METHODS);
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "the built-in `List[{}]` type has no method `{}` — the kernel is {kernel}",
                    elem.display(tys),
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

/// v0.91 (ADR 0115): the lazy query vocabulary names — the builders/terminals a
/// `store` map receiver lifts into a `Query` (vs the entry ops `put`/`get`/…).
/// `count` is the query terminal; the map's entry-count op is `size`.
pub(crate) fn is_query_op(name: &str) -> bool {
    matches!(
        name,
        "map"
            | "filter"
            | "flatMap"
            | "sortBy"
            | "take"
            | "skip"
            | "distinct"
            | "distinctBy"
            | "joinOn"
            | "leftJoin"
            | "join"
            | "groupBy"
            | "collect"
            | "first"
            | "firstOrElse"
            | "count"
            | "fold"
            | "any"
            | "all"
            | "sum"
            | "min"
            | "max"
            | "average"
            | "forEach"
            | "parTraverse"
            | "traverseAll"
            | "parTraverseAll"
            | "traverseTry"
            | "parTraverseTry"
    )
}

/// v0.91 (ADR 0115/0119, query-algebra slice 2): type a **lazy** storage-query
/// method on a `Query[T]` receiver (or a `store` field lifted into one). The
/// vocabulary mirrors the eager `List` one (ADR 0116), but **builders return
/// `Query[U]`** (still lazy, chainable) and **terminals return `Effect[T]`** (the
/// storage read folds into the agent's storage capability, ADR 0115 D5). `elem`
/// is the query's element type. Joins and `groupBy` arrive with slice 4.
pub(crate) fn check_query_kernel_method(
    method: &Ident,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let arity = |n: usize, ctx: &mut Ctx| {
        if args.len() != n {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`Query.{}` takes {n} argument{}, got {}",
                    method.name,
                    if n == 1 { "" } else { "s" },
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    // T3.6b: both wrappers take and return interned ids.
    let query = |t: TyId| tys.intern(Ty::Query(t));
    let eff = |t: TyId| tys.intern(Ty::Effect(t));
    match method.name.as_str() {
        // -- builders (return Query) --
        "map" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(&args[0], vec![elem], "the `Query.map` function", ctx)?;
            Some(query(ret))
        }
        "filter" => {
            if !arity(1, ctx) {
                return None;
            }
            let p = Ty::Fn {
                params: vec![elem],
                ret: tys.intern(Ty::Base(BaseType::Bool)),
            };
            check_arg(
                &args[0],
                tys.intern(p.clone()),
                "the `Query.filter` predicate",
                ctx,
            );
            Some(query(elem))
        }
        "flatMap" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret =
                check_kernel_fn_arg(&args[0], vec![elem], "the `Query.flatMap` function", ctx)?;
            match &*tys.get(ret) {
                Ty::Query(_) => Some(ret),
                _ => {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.combinator_return_mismatch",
                        args[0].span,
                        format!(
                            "the `Query.flatMap` function must return a `Query`, but returns `{}`",
                            ret.display(tys)
                        ),
                    ));
                    None
                }
            }
        }
        "sortBy" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(&args[0], vec![elem], "the `Query.sortBy` key", ctx)?;
            require_orderable(key, "Query.sortBy", args[0].span, ctx);
            Some(query(elem))
        }
        "take" | "skip" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(
                &args[0],
                tys.intern(Ty::Base(BaseType::Int)),
                &format!("the `Query.{}` count", method.name),
                ctx,
            );
            Some(query(elem))
        }
        "distinct" => {
            if !arity(0, ctx) {
                return None;
            }
            require_keyable(elem, "Query.distinct", span, ctx);
            Some(query(elem))
        }
        "distinctBy" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(&args[0], vec![elem], "the `Query.distinctBy` key", ctx)?;
            require_keyable(key, "Query.distinctBy", args[0].span, ctx);
            Some(query(elem))
        }
        // -- terminals (return Effect) --
        "collect" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(eff(tys.intern(Ty::List(elem))))
        }
        "first" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(eff(tys.intern(Ty::Option(elem))))
        }
        "firstOrElse" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(&args[0], elem, "the `Query.firstOrElse` fallback", ctx);
            Some(eff(elem))
        }
        "count" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(eff(tys.intern(Ty::Base(BaseType::Int))))
        }
        "fold" => {
            if !arity(2, ctx) {
                return None;
            }
            let acc = type_of(&args[0], None, ctx)?;
            let step = Ty::Fn {
                params: vec![acc, elem],
                ret: acc,
            };
            check_arg(
                &args[1],
                tys.intern(step.clone()),
                "the `Query.fold` step function",
                ctx,
            );
            Some(eff(acc))
        }
        "any" | "all" => {
            if !arity(1, ctx) {
                return None;
            }
            let p = Ty::Fn {
                params: vec![elem],
                ret: tys.intern(Ty::Base(BaseType::Bool)),
            };
            check_arg(
                &args[0],
                tys.intern(p.clone()),
                &format!("the `Query.{}` predicate", method.name),
                ctx,
            );
            Some(eff(tys.intern(Ty::Base(BaseType::Bool))))
        }
        "sum" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(&args[0], vec![elem], "the `Query.sum` key", ctx)?;
            require_numeric(key, "Query.sum", args[0].span, ctx);
            Some(eff(key))
        }
        "min" | "max" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(
                &args[0],
                vec![elem],
                &format!("the `Query.{}` key", method.name),
                ctx,
            )?;
            require_orderable(key, &format!("Query.{}", method.name), args[0].span, ctx);
            Some(eff(tys.intern(Ty::Option(key))))
        }
        "average" => {
            if !arity(1, ctx) {
                return None;
            }
            let key = check_kernel_fn_arg(&args[0], vec![elem], "the `Query.average` key", ctx)?;
            require_numeric(key, "Query.average", args[0].span, ctx);
            let result = match key.base(tys) {
                Some(BaseType::Duration) => Ty::Base(BaseType::Duration),
                _ => Ty::Base(BaseType::Float),
            };
            Some(eff(tys.intern(Ty::Option(tys.intern(result)))))
        }
        // v0.107 (slice 4): `forEach` runs the effectful function over each element
        // **in turn**; `parTraverse` runs them **concurrently** (the broadcast form —
        // a slow/half-dead connection must not head-of-line-block the rest). Same
        // type; the difference is sequential-vs-parallel lowering.
        "forEach" | "parTraverse" => {
            if !arity(1, ctx) {
                return None;
            }
            let f = Ty::Fn {
                params: vec![elem],
                ret: tys.intern(Ty::Effect(tys.intern(Ty::Unit))),
            };
            check_arg(
                &args[0],
                tys.intern(f.clone()),
                &format!("the `Query.{}` function", method.name),
                ctx,
            );
            Some(eff(tys.intern(Ty::Unit)))
        }
        // v0.149 (ADR 0173): the collect-all terminals over a lazy `Query`/a
        // lifted storage collection (the broadcast form for a held
        // `Map[K, Connection]`). `f: T -> Effect[Result[U, E]]`; every outcome is
        // gathered (no short-circuit) into `Effect[List[Result[U, E]]]`. The
        // `List.traverseAll`/`parTraverseAll` shape (ADR 0172) over the query
        // vocabulary, so the same fan-out reaches live connections.
        TRAVERSE_ALL | PAR_TRAVERSE_ALL => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(
                &args[0],
                vec![elem],
                &format!("the `Query.{}` function", method.name),
                ctx,
            )?;
            let result_ty = match &*tys.get(ret) {
                Ty::Effect(inner) => match &*tys.get(*inner) {
                    Ty::Result(_, _) => *inner,
                    _ => {
                        ctx.errors.push(CompileError::new(
                            "bynk.types.combinator_return_mismatch",
                            args[0].span,
                            format!(
                                "the `Query.{}` function must return `Effect[Result[U, E]]`, but returns `Effect[{}]`",
                                method.name,
                                inner.display(tys)
                            ),
                        ));
                        return None;
                    }
                },
                _ => {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.combinator_return_mismatch",
                        args[0].span,
                        format!(
                            "the `Query.{}` function must return `Effect[Result[U, E]]`, but returns `{}`",
                            method.name,
                            ret.display(tys)
                        ),
                    ));
                    return None;
                }
            };
            Some(eff(tys.intern(Ty::List(result_ty))))
        }
        // v0.150 (ADR 0174): the short-circuit collect terminals over a lazy
        // `Query`/a lifted storage collection — stop at the first `Err`, returning
        // `Effect[Result[List[U], E]]`. The `List.traverseTry`/`parTraverseTry`
        // shape over the query broadcast, so the fan-out reaches live connections.
        TRAVERSE_TRY | PAR_TRAVERSE_TRY => {
            if !arity(1, ctx) {
                return None;
            }
            let (ok_ty, err_ty) = check_try_fn_arg(&args[0], elem, &method.name, "Query", ctx)?;
            let list = tys.intern(Ty::List(ok_ty));
            Some(eff(tys.intern(Ty::Result(list, err_ty))))
        }
        // v0.94 (ADR 0116/0120): joins & grouping are lazy builders — they project
        // through `into` and stay chainable as `Query[V]`.
        "joinOn" => Some(query(check_equi_join(args, elem, span, true, false, ctx)?)),
        "leftJoin" => Some(query(check_equi_join(args, elem, span, true, true, ctx)?)),
        "join" => Some(query(check_pred_join(args, elem, span, true, ctx)?)),
        "groupBy" => Some(query(check_group_by(args, elem, span, true, ctx)?)),
        _ => {
            // Generated from `QUERY_METHODS` (T1.6′) rather than hand-listed
            // here, so the two can't drift the way this message once did —
            // it fell behind when `join`/`joinOn`/`leftJoin`/`groupBy`
            // (ADR 0116/0120) landed in the table but not in this string.
            let kernel = kernel_vocabulary(QUERY_METHODS);
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "the built-in `Query[{}]` type has no method `{}` — the kernel is {kernel}",
                    elem.display(tys),
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

/// v0.100: type a built-in `Stream[T]` kernel method (real-time track slice 0).
/// The vocabulary is deliberately minimal at v1 — the lazy builders `map` and a
/// bounded `take` return `Stream[U]`/`Stream[T]`, and the single terminal
/// `collect` drains the stream into `Effect[List[T]]` (the observation point a
/// test asserts on). A fuller algebra (`filter`/`scan`/fan-in) earns its own
/// slice + ADR, as the query algebra did. `elem` is the stream's element type.
pub(crate) fn check_stream_kernel_method(
    method: &Ident,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let arity = |n: usize, ctx: &mut Ctx| {
        if args.len() != n {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`Stream.{}` takes {n} argument{}, got {}",
                    method.name,
                    if n == 1 { "" } else { "s" },
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    let stream = |t: TyId| tys.intern(Ty::Stream(t));
    let eff = |t: TyId| tys.intern(Ty::Effect(t));
    match method.name.as_str() {
        // -- builders (return Stream) --
        "map" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(&args[0], vec![elem], "the `Stream.map` function", ctx)?;
            Some(stream(ret))
        }
        "take" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(
                &args[0],
                tys.intern(Ty::Base(BaseType::Int)),
                "the `Stream.take` count",
                ctx,
            );
            Some(stream(elem))
        }
        // -- terminal (returns Effect) --
        "collect" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(eff(tys.intern(Ty::List(elem))))
        }
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "the built-in `Stream[{}]` type has no method `{}` — builders are `map`/`take`, the terminal is `collect`",
                    elem.display(tys),
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

/// v0.100: the `Stream.of(xs)` static constructor — builds a deterministic
/// `Stream[T]` from a `List[T]` (the in-memory source v1 ships; live runtime
/// sources arrive with their consumers, e.g. the streaming-HTTP terminal). The
/// element type is inferred from the argument list. Mirrors the
/// `Duration.millis` / `Instant.fromEpochMillis` static-constructor shape.
pub(crate) fn check_stream_static(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if method.name != OF {
        // The resolver owns the unknown-static diagnostic; don't double up.
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`Stream.of` takes 1 argument, got {}", args.len()),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let arg_ty = type_of(&args[0], None, ctx)?;
    match &*tys.get(arg_ty) {
        Ty::List(elem) => Some(tys.intern(Ty::Stream(*elem))),
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.argument_mismatch",
                args[0].span,
                format!(
                    "`Stream.of` expects a `List[T]`, but the argument is `{}`",
                    arg_ty.display(tys)
                ),
            ));
            None
        }
    }
}

/// v0.102: type a held-resource operation on a `Connection[F]` (real-time track
/// slice 2). `send(f: F)` writes a frame and is **non-consuming** (the binding
/// stays owned); `close()` is **consuming** (ends the connection's life). Both
/// are effectful (`Effect[()]`). The *ownership* transitions these imply are
/// enforced by the linearity pass (§3 step 11); this only types the call.
/// `frame` is the connection's `F` (the server→client frame type).
pub(crate) fn check_connection_method(
    method: &Ident,
    args: &[Expr],
    frame: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let eff_unit = || Ty::Effect(tys.intern(Ty::Unit));
    match method.name.as_str() {
        "send" => {
            if args.len() != 1 {
                ctx.errors.push(CompileError::new(
                    "bynk.types.method_arity",
                    span,
                    format!("`Connection.send` takes 1 argument, got {}", args.len()),
                ));
                for a in args {
                    let _ = type_of(a, None, ctx);
                }
                return None;
            }
            // The channel is typed: the frame must match `F` (a wrong-shaped
            // frame is a compile error at the `.send` site, §2.9.1).
            check_arg(&args[0], frame, "the `Connection.send` frame", ctx);
            Some(tys.intern(eff_unit()))
        }
        "close" => {
            if !args.is_empty() {
                ctx.errors.push(CompileError::new(
                    "bynk.types.method_arity",
                    span,
                    format!("`Connection.close` takes no arguments, got {}", args.len()),
                ));
                for a in args {
                    let _ = type_of(a, None, ctx);
                }
                return None;
            }
            Some(tys.intern(eff_unit()))
        }
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "the built-in `Connection[{}]` type has no method `{}` — the operations are `send` and `close`",
                    frame.display(tys),
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

/// v0.21: type a built-in numeric kernel method (ADR 0041). Conversions are
/// value methods on the bare base types: `Int -> Float` is total
/// (`toFloat`); `Float -> Int` is one of four named, lossy roundings — there
/// is deliberately no ambiguous `toInt`. v0.22a (ADR 0048) extends the
/// kernel with `abs`/`min`/`max`/`clamp` on both numeric types and
/// `isNaN`/`isFinite` on `Float`.
pub(crate) fn check_numeric_kernel_method(
    method: &Ident,
    args: &[Expr],
    base: BaseType,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let b = Ty::Base(base);
    // (parameter types, return type)
    let sig: Option<(Vec<TyId>, Ty)> = match (base, method.name.as_str()) {
        (BaseType::Int, "toFloat") => Some((vec![], Ty::Base(BaseType::Float))),
        (BaseType::Float, "round" | "floor" | "ceil" | "truncate") => {
            Some((vec![], Ty::Base(BaseType::Int)))
        }
        (_, "abs") => Some((vec![], b.clone())),
        (_, "min" | "max") => Some((vec![tys.intern(b.clone())], b.clone())),
        (_, "clamp") => Some((
            vec![tys.intern(b.clone()), tys.intern(b.clone())],
            b.clone(),
        )),
        // v0.42 (ADR 0074): render a number as text — the missing direction
        // (`Int.parse` covers parsing). Total on both `Int` and `Float`.
        (_, "toString") => Some((vec![], Ty::Base(BaseType::String))),
        (BaseType::Float, "isNaN" | "isFinite") => Some((vec![], Ty::Base(BaseType::Bool))),
        // K6b (#2598): integer division companions and an exact power, each
        // faulting on a zero divisor or a result past the safe-integer range.
        (BaseType::Int, "div" | "rem" | "mod" | "pow") => {
            Some((vec![tys.intern(b.clone())], b.clone()))
        }
        // K6b (#2598): the `Float` functions over JS `Math`, host-defined
        // outside their domains as `Float` arithmetic is (ADR 0042).
        (
            BaseType::Float,
            "sqrt" | "exp" | "ln" | "log10" | "log2" | "sin" | "cos" | "tan" | "asin" | "acos"
            | "atan",
        ) => Some((vec![], b.clone())),
        (BaseType::Float, "pow" | "atan2") => Some((vec![tys.intern(b.clone())], b.clone())),
        _ => None,
    };
    let Some((params, ret)) = sig else {
        let kernel = match base {
            BaseType::Int => {
                "`toFloat`, `toString`, `abs`, `min`, `max`, `clamp`, `div`, `rem`, `mod`, `pow`"
            }
            _ => {
                "`round`, `floor`, `ceil`, `truncate`, `toString`, `abs`, `min`, `max`, \
                 `clamp`, `isNaN`, `isFinite`, `sqrt`, `pow`, `exp`, `ln`, `log10`, `log2`, \
                 `sin`, `cos`, `tan`, `asin`, `acos`, `atan`, `atan2`"
            }
        };
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "the built-in `{}` type has no method `{}` — the kernel is {kernel}",
                base.name(),
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`{}.{}` takes {} argument{}, got {}",
                base.name(),
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (a, p) in args.iter().zip(&params) {
        check_arg(
            a,
            *p,
            &format!("the `{}.{}` argument", base.name(), method.name),
            ctx,
        );
    }
    Some(tys.intern(ret))
}

/// v0.86 (ADR 0112): type a `Duration` kernel method. `Duration` arithmetic and
/// comparison are operators (D3/D4); the kernel is the explicit escape to raw
/// milliseconds (`toMillis`) plus `toString`.
pub(crate) fn check_duration_kernel_method(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let sig: Option<(Vec<TyId>, Ty)> = match method.name.as_str() {
        "toMillis" => Some((vec![], Ty::Base(BaseType::Int))),
        "toString" => Some((vec![], Ty::Base(BaseType::String))),
        _ => None,
    };
    let Some((params, ret)) = sig else {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "the built-in `Duration` type has no method `{}` — the kernel is `toMillis`, `toString`",
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Duration.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    Some(tys.intern(ret))
}

/// v0.90 (ADR 0114 D6): the `Instant` kernel — the explicit escape to raw epoch
/// milliseconds. Comparison/arithmetic are operators (D3); the static
/// `Instant.fromEpochMillis(n)` (the build-from-`Int` direction) is resolved
/// separately as a type static.
pub(crate) fn check_instant_kernel_method(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let sig: Option<(Vec<TyId>, Ty)> = match method.name.as_str() {
        "toEpochMillis" => Some((vec![], Ty::Base(BaseType::Int))),
        "toString" => Some((vec![], Ty::Base(BaseType::String))),
        // K3 (#2592): RFC 3339 text in UTC.
        "toIso" => Some((vec![], Ty::Base(BaseType::String))),
        _ => None,
    };
    let Some((params, ret)) = sig else {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "the built-in `Instant` type has no method `{}` — the kernel is `toEpochMillis`, `toString`, `toIso`",
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Instant.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    Some(tys.intern(ret))
}

/// v0.110 (ADR 0142 D3/D4): the `Bytes` kernel — `length` (octet count) plus
/// the String-interop bridge. `toBase64` is total; `decodeUtf8` is partial
/// (`None` on an invalid UTF-8 sequence). Equality is an operator (D4, content
/// compare); the statics `Bytes.fromUtf8`/`fromBase64`/`empty` (the
/// build-from-`String` direction) are resolved separately as type statics.
///
/// #2016 (standard-library K1): `concat`, `slice` (clamped like
/// `String.slice`), `compare` (unsigned lexicographic, `-1`/`0`/`1`; `Bytes`
/// stays unorderable), `toHex` and `toBase64Url`.
pub(crate) fn check_bytes_kernel_method(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let bytes = Ty::Base(BaseType::Bytes);
    let int = Ty::Base(BaseType::Int);
    let sig: Option<(Vec<TyId>, Ty)> = match method.name.as_str() {
        "length" => Some((vec![], int.clone())),
        "toBase64" | "toBase64Url" | "toHex" => Some((vec![], Ty::Base(BaseType::String))),
        "decodeUtf8" => Some((vec![], Ty::Option(tys.intern(Ty::Base(BaseType::String))))),
        "concat" => Some((vec![tys.intern(bytes.clone())], bytes)),
        "slice" => Some((
            vec![tys.intern(int.clone()), tys.intern(int.clone())],
            bytes,
        )),
        "compare" => Some((vec![tys.intern(bytes)], int)),
        _ => None,
    };
    let Some((params, ret)) = sig else {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "the built-in `Bytes` type has no method `{}` — the kernel is `length`, \
                 `concat`, `slice`, `compare`, `toHex`, `toBase64`, `toBase64Url`, `decodeUtf8`",
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Bytes.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (a, p) in args.iter().zip(&params) {
        check_arg(a, *p, &format!("the `Bytes.{}` argument", method.name), ctx);
    }
    Some(tys.intern(ret))
}

/// #2341 (decision A): type a method on an HTTP handler's `headers`
/// (`RequestHeaders`). `get(name)` reads a header case-insensitively, joining
/// repeated ones with `", "`; `bearer()` is the token of an
/// `Authorization: Bearer …` header; `clientIp()` is the client's address. Each
/// is `Option[String]`.
pub(crate) fn check_request_headers_method(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let string = tys.intern(Ty::Base(BaseType::String));
    let params: Vec<TyId> = match method.name.as_str() {
        "get" => vec![string],
        "bearer" | "clientIp" => vec![],
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "`RequestHeaders` has no method `{}` — it has `get(name)`, `bearer()` and `clientIp()`",
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`RequestHeaders.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (a, p) in args.iter().zip(&params) {
        check_arg(
            a,
            *p,
            &format!("the `RequestHeaders.{}` argument", method.name),
            ctx,
        );
    }
    Some(tys.intern(Ty::Option(string)))
}

/// #2380 (L5): type a method on an `Oidc` binder's `claims` (`TokenClaims`).
/// `get(name)` is the claim when it is a JSON string (`Option[String]`);
/// `has(name)` is whether the claim is present, of any type.
pub(crate) fn check_token_claims_method(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let string = tys.intern(Ty::Base(BaseType::String));
    let ret = match method.name.as_str() {
        "get" => Ty::Option(string),
        "has" => Ty::Base(BaseType::Bool),
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "`TokenClaims` has no method `{}` — it has `get(name)` and `has(name)`",
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
    };
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`TokenClaims.{}` takes 1 argument, got {}",
                method.name,
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    check_arg(
        &args[0],
        string,
        &format!("the `TokenClaims.{}` argument", method.name),
        ctx,
    );
    Some(tys.intern(ret))
}

/// v0.22a: type a built-in `String` kernel method (ADR 0046). `String` is
/// opaque (no char access), so its operations are compiler built-ins
/// lowering to TS string methods — the 0034/0037 hybrid posture. Semantics
/// are UTF-16 code units, except `chars()` (code points, normatively).
pub(crate) fn check_string_kernel_method(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let s = Ty::Base(BaseType::String);
    let int = Ty::Base(BaseType::Int);
    let boolean = Ty::Base(BaseType::Bool);
    let strings = Ty::List(tys.intern(s.clone()));
    // (parameter types, return type)
    let sig: Option<(Vec<TyId>, Ty)> = match method.name.as_str() {
        "length" => Some((vec![], int.clone())),
        "split" => Some((vec![tys.intern(s.clone())], strings.clone())),
        "trim" | "toUpper" | "toLower" => Some((vec![], s.clone())),
        "contains" | "startsWith" | "endsWith" => Some((vec![tys.intern(s.clone())], boolean)),
        "replace" => Some((
            vec![tys.intern(s.clone()), tys.intern(s.clone())],
            s.clone(),
        )),
        "slice" => Some((
            vec![tys.intern(int.clone()), tys.intern(int.clone())],
            s.clone(),
        )),
        "indexOf" => Some((vec![tys.intern(s.clone())], Ty::Option(tys.intern(int)))),
        "chars" => Some((vec![], strings)),
        "concat" => Some((vec![tys.intern(s.clone())], s.clone())),
        // K2 (#2594): percent-encoding over the UTF-8 bytes.
        "percentEncode" => Some((vec![], s.clone())),
        "percentDecode" => Some((vec![], Ty::Option(tys.intern(s.clone())))),
        _ => None,
    };
    let Some((params, ret)) = sig else {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_not_found",
            method.span,
            format!(
                "the built-in `String` type has no method `{}` — the kernel is \
                 `length`, `split`, `trim`, `contains`, `startsWith`, `endsWith`, \
                 `replace`, `slice`, `indexOf`, `toUpper`, `toLower`, `chars`, `concat`, \
                 `percentEncode`, `percentDecode`",
                method.name
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`String.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (a, p) in args.iter().zip(&params) {
        check_arg(
            a,
            *p,
            &format!("the `String.{}` argument", method.name),
            ctx,
        );
    }
    Some(tys.intern(ret))
}

/// v0.22a: type a function-valued kernel argument (the 0048 combinators —
/// `map`/`andThen`/`mapErr`). Parameter types are known from the receiver;
/// the return is read from the actual: an expected return carrying a
/// flexible variable lets a lambda type bottom-up (the v0.20a pass-2 rule),
/// and `unify` captures it here. Returns the function's return type.
fn check_kernel_fn_arg(arg: &Expr, params: Vec<TyId>, label: &str, ctx: &mut Ctx) -> Option<TyId> {
    let tys = ctx.tys;
    const RET_VAR: &str = "__kernel_ret";
    let expected = Ty::Fn {
        params: params.clone(),
        ret: tys.intern(Ty::Var(RET_VAR.to_string())),
    };
    let actual = type_of(arg, Some(tys.intern(expected.clone())), ctx)?;
    let mut subst: HashMap<String, TyId> = HashMap::new();
    if unify(tys.intern(expected), actual, &mut subst, tys)
        && let Some(ret) = subst.get(RET_VAR).cloned()
    {
        // `unify` is permissive ground-vs-ground; re-check the whole shape.
        let want = Ty::Fn { params, ret };
        if compatible(actual, tys.intern(want), tys) {
            return Some(ret);
        }
    }
    ctx.errors.push(CompileError::new(
        "bynk.types.argument_mismatch",
        arg.span,
        format!(
            "{label} expects a function over the receiver's value, but got `{}`",
            actual.display(tys)
        ),
    ));
    None
}

/// v0.150 (ADR 0174): infer a `traverseTry`/`parTraverseTry` function argument's
/// return type, which MUST be `Effect[Result[U, E]]`, and peel it to `(U, E)`.
/// `recv` is the collection's element type; `kind` is `"List"` or `"Query"` (for
/// the diagnostic). A non-`Result` effect is `bynk.types.combinator_return_mismatch`.
fn check_try_fn_arg(
    arg: &Expr,
    elem: TyId,
    method: &str,
    kind: &str,
    ctx: &mut Ctx,
) -> Option<(TyId, TyId)> {
    let tys = ctx.tys;
    let ret = check_kernel_fn_arg(
        arg,
        vec![elem],
        &format!("the `{kind}.{method}` function"),
        ctx,
    )?;
    match &*tys.get(ret) {
        Ty::Effect(inner) => match &*tys.get(*inner) {
            Ty::Result(ok, err) => Some((*ok, *err)),
            _ => {
                ctx.errors.push(CompileError::new(
                    "bynk.types.combinator_return_mismatch",
                    arg.span,
                    format!(
                        "the `{kind}.{method}` function must return `Effect[Result[U, E]]`, but returns `Effect[{}]`",
                        inner.display(tys)
                    ),
                ));
                None
            }
        },
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.combinator_return_mismatch",
                arg.span,
                format!(
                    "the `{kind}.{method}` function must return `Effect[Result[U, E]]`, but returns `{}`",
                    ret.display(tys)
                ),
            ));
            None
        }
    }
}

/// v0.22a: type a built-in `Option[T]` kernel method (ADR 0048) — the
/// combinators as value methods on the compiler-known receiver (collision-
/// free, unlike free functions imported by bare name).
pub(crate) fn check_option_kernel_method(
    method: &Ident,
    args: &[Expr],
    inner: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let arity = |n: usize, ctx: &mut Ctx| {
        if args.len() != n {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`Option.{}` takes {n} argument{}, got {}",
                    method.name,
                    if n == 1 { "" } else { "s" },
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    match method.name.as_str() {
        "map" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(&args[0], vec![inner], "the `Option.map` function", ctx)?;
            Some(tys.intern(Ty::Option(ret)))
        }
        "andThen" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret =
                check_kernel_fn_arg(&args[0], vec![inner], "the `Option.andThen` function", ctx)?;
            match &*tys.get(ret) {
                Ty::Option(_) => Some(ret),
                _ => {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.combinator_return_mismatch",
                        args[0].span,
                        format!(
                            "the `Option.andThen` function must return an `Option`, but returns `{}`",
                            ret.display(tys)
                        ),
                    ));
                    None
                }
            }
        }
        "getOrElse" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(&args[0], inner, "the `Option.getOrElse` fallback", ctx);
            Some(inner)
        }
        "isSome" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::Bool)))
        }
        "okOr" => {
            if !arity(1, ctx) {
                return None;
            }
            let err = type_of(&args[0], None, ctx)?;
            Some(tys.intern(Ty::Result(inner, err)))
        }
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "the built-in `Option[{}]` type has no method `{}` — the kernel is \
                     `map`, `andThen`, `getOrElse`, `isSome`, `okOr`",
                    inner.display(tys),
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

/// v0.22a: type a built-in `Result[T, E]` kernel method (ADR 0048).
pub(crate) fn check_result_kernel_method(
    method: &Ident,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let arity = |n: usize, ctx: &mut Ctx| {
        if args.len() != n {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`Result.{}` takes {n} argument{}, got {}",
                    method.name,
                    if n == 1 { "" } else { "s" },
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    match method.name.as_str() {
        "map" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(&args[0], vec![ok], "the `Result.map` function", ctx)?;
            Some(tys.intern(Ty::Result(ret, err)))
        }
        "andThen" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret =
                check_kernel_fn_arg(&args[0], vec![ok], "the `Result.andThen` function", ctx)?;
            match &*tys.get(ret) {
                Ty::Result(b, e2) => {
                    let (b, e2) = (*b, *e2);
                    if !compatible(e2, err, tys) && !compatible(err, e2, tys) {
                        ctx.errors.push(CompileError::new(
                            "bynk.types.combinator_return_mismatch",
                            args[0].span,
                            format!(
                                "the `Result.andThen` function's error type `{}` does not match the receiver's `{}`",
                                e2.display(tys),
                                err.display(tys)
                            ),
                        ));
                        return None;
                    }
                    Some(tys.intern(Ty::Result(b, err)))
                }
                _ => {
                    ctx.errors.push(CompileError::new(
                        "bynk.types.combinator_return_mismatch",
                        args[0].span,
                        format!(
                            "the `Result.andThen` function must return a `Result`, but returns `{}`",
                            ret.display(tys)
                        ),
                    ));
                    None
                }
            }
        }
        "mapErr" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret =
                check_kernel_fn_arg(&args[0], vec![err], "the `Result.mapErr` function", ctx)?;
            Some(tys.intern(Ty::Result(ok, ret)))
        }
        "getOrElse" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(&args[0], ok, "the `Result.getOrElse` fallback", ctx);
            Some(ok)
        }
        "isOk" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::Bool)))
        }
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "the built-in `Result[{}, {}]` type has no method `{}` — the kernel is \
                     `map`, `andThen`, `mapErr`, `getOrElse`, `isOk`",
                    ok.display(tys),
                    err.display(tys),
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

/// The `Effect[Result[T, E]]` combinators (design doc §2.8.3) — the
/// compiler-synthesised methods on the universal cross-context shape. Each is
/// syntactic sugar for a composition of Effect's `.map`/`.flatMap` with
/// Result's success/error split; the desugaring is the semantics (the emitter
/// inlines it). `ok`/`err` are the peeled `Result` parameters of the receiver's
/// `Effect[Result[ok, err]]`.
///
/// - `mapOk(f: T -> U)         : Effect[Result[U, E]]` — map the success value.
/// - `mapErr(f: E -> F)        : Effect[Result[T, F]]` — map the error.
/// - `flatMapOk(f: T -> Effect[Result[U, E]]) : Effect[Result[U, E]]` — chain a
///   further effectful-fallible step on success (same `E`).
/// - `flatMapErr(f: E -> Effect[Result[T, F]]): Effect[Result[T, F]]` — attempt
///   an effectful recovery on error (same `T`).
///
/// These produce an `Effect` value (they do not *run* it), so — unlike the
/// eager `List.forEach`/`traverseTry` iterators — they are not confined to
/// effectful contexts: a pure helper may transform an `Effect[Result[…]]` it was
/// handed and return it.
pub(crate) fn check_effect_result_kernel_method(
    method: &Ident,
    args: &[Expr],
    ok: TyId,
    err: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let arity = |n: usize, ctx: &mut Ctx| {
        if args.len() != n {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`Effect[Result].{}` takes {n} argument{}, got {}",
                    method.name,
                    if n == 1 { "" } else { "s" },
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    match method.name.as_str() {
        "mapOk" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(
                &args[0],
                vec![ok],
                "the `Effect[Result].mapOk` function",
                ctx,
            )?;
            Some(tys.intern(Ty::Effect(tys.intern(Ty::Result(ret, err)))))
        }
        "mapErr" => {
            if !arity(1, ctx) {
                return None;
            }
            let ret = check_kernel_fn_arg(
                &args[0],
                vec![err],
                "the `Effect[Result].mapErr` function",
                ctx,
            )?;
            Some(tys.intern(Ty::Effect(tys.intern(Ty::Result(ok, ret)))))
        }
        "flatMapOk" => {
            if !arity(1, ctx) {
                return None;
            }
            // `f: T -> Effect[Result[U, E]]` — peel the effectful `Result` and
            // require its error to match the receiver's `E` (the chain keeps a
            // single error type, exactly as `?` does).
            let (u, e2) = check_effect_result_fn_arg(&args[0], ok, &method.name, ctx)?;
            if !compatible(e2, err, tys) && !compatible(err, e2, tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.combinator_return_mismatch",
                    args[0].span,
                    format!(
                        "the `Effect[Result].flatMapOk` function's error type `{}` does not match the receiver's `{}`",
                        e2.display(tys),
                        err.display(tys)
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::Effect(tys.intern(Ty::Result(u, err)))))
        }
        "flatMapErr" => {
            if !arity(1, ctx) {
                return None;
            }
            // `f: E -> Effect[Result[T, F]]` — the recovery must produce the
            // receiver's success type `T`; its error `F` becomes the result's.
            let (t2, f) = check_effect_result_fn_arg(&args[0], err, &method.name, ctx)?;
            if !compatible(t2, ok, tys) && !compatible(ok, t2, tys) {
                ctx.errors.push(CompileError::new(
                    "bynk.types.combinator_return_mismatch",
                    args[0].span,
                    format!(
                        "the `Effect[Result].flatMapErr` recovery must produce the receiver's success type `{}`, but produces `{}`",
                        ok.display(tys),
                        t2.display(tys)
                    ),
                ));
                return None;
            }
            Some(tys.intern(Ty::Effect(tys.intern(Ty::Result(ok, f)))))
        }
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "the built-in `Effect[Result[{}, {}]]` type has no method `{}` — the kernel is \
                     `mapOk`, `mapErr`, `flatMapOk`, `flatMapErr`",
                    ok.display(tys),
                    err.display(tys),
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

/// Infer an `Effect[Result[T, E]]`-combinator function argument whose return
/// MUST be `Effect[Result[U, F]]`, peeling it to `(U, F)`. `param` is the
/// single parameter type the receiver dictates (the `Ok` type for `flatMapOk`,
/// the `Err` type for `flatMapErr`). A non-`Effect[Result]` return is
/// `bynk.types.combinator_return_mismatch`. Mirrors [`check_try_fn_arg`], with
/// the `Effect[Result]` diagnostic wording.
fn check_effect_result_fn_arg(
    arg: &Expr,
    param: TyId,
    method: &str,
    ctx: &mut Ctx,
) -> Option<(TyId, TyId)> {
    let tys = ctx.tys;
    let ret = check_kernel_fn_arg(
        arg,
        vec![param],
        &format!("the `Effect[Result].{method}` function"),
        ctx,
    )?;
    match &*tys.get(ret) {
        Ty::Effect(inner) => match &*tys.get(*inner) {
            Ty::Result(ok, err) => Some((*ok, *err)),
            _ => {
                ctx.errors.push(CompileError::new(
                    "bynk.types.combinator_return_mismatch",
                    arg.span,
                    format!(
                        "the `Effect[Result].{method}` function must return `Effect[Result[U, E]]`, but returns `Effect[{}]`",
                        inner.display(tys)
                    ),
                ));
                None
            }
        },
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.combinator_return_mismatch",
                arg.span,
                format!(
                    "the `Effect[Result].{method}` function must return `Effect[Result[U, E]]`, but returns `{}`",
                    ret.display(tys)
                ),
            ));
            None
        }
    }
}

/// v0.22b: whether a type can pass through the typed JSON codec — every
/// boundary-serialisable shape: bases, named types, and the built-in
/// generic containers over them. Functions, effects, `HttpResult`, the
/// error builtins, and type variables cannot.
/// v0.174 (#592): the shared diagnostic for a recursive generic record reaching
/// a boundary — a `Json.encode`/`decode` target here, a serialised position in
/// the emit-side boundary pass.
fn recursive_generic_error(name: &str, span: bynk_syntax::span::Span) -> CompileError {
    CompileError::new(
        "bynk.generics.recursive_generic_at_boundary",
        span,
        format!(
            "recursive generic record `{name}` cannot cross a boundary — it has no finite monomorphised codec"
        ),
    )
    .with_note(
        "a generic record that transitively contains itself is not yet boundary-serialisable; \
         use a concrete (non-generic) recursive type, or break the cycle",
    )
}

/// v0.174 (#592): the name of a recursive generic-record instantiation reachable
/// anywhere in `t` (as a codec target or nested inside one), or `None`. A
/// recursive generic record has no finite set of monomorphised codecs, so it
/// cannot cross a boundary yet — this reports it with a specific diagnostic
/// rather than the generic `json_uncodable`.
fn first_recursive_generic(
    t: TyId,
    types: &std::collections::HashMap<String, Arc<TypeDecl>>,
    tys: &Types,
) -> Option<String> {
    match &*tys.get(t) {
        Ty::Named { name, args, .. } => {
            if !args.is_empty() && generic_record_is_recursive(name, types) {
                return Some(name.clone());
            }
            args.iter()
                .find_map(|a| first_recursive_generic(*a, types, tys))
        }
        Ty::Result(a, b) | Ty::Map(a, b) => first_recursive_generic(*a, types, tys)
            .or_else(|| first_recursive_generic(*b, types, tys)),
        Ty::Option(a)
        | Ty::List(a)
        | Ty::Set(a)
        | Ty::Effect(a)
        | Ty::HttpResult(a)
        | Ty::HttpOutcome(a)
        | Ty::Query(a)
        | Ty::Stream(a)
        | Ty::Connection(a) => first_recursive_generic(*a, types, tys),
        Ty::Fn { params, ret } => params
            .iter()
            .find_map(|p| first_recursive_generic(*p, types, tys))
            .or_else(|| first_recursive_generic(*ret, types, tys)),
        _ => None,
    }
}

fn json_codable(t: TyId, tys: &Types) -> bool {
    match &*tys.get(t) {
        // R4.3: an already-diagnosed type doesn't also earn a
        // `bynk.types.json_uncodable` diagnostic — the resolution failure was
        // reported once, at its own site.
        Ty::Error => true,
        // v0.174 (#592): a generic record instantiation is boundary-serialisable
        // through its monomorphised codec (`serialise_Paginated_User`) exactly
        // when its type arguments are — a non-serialisable argument (a function,
        // a `Query`, …) is the reason the instantiation is uncodable, mirroring
        // the container arms below. (ADR 0183 Decision C's non-boundary rule was
        // the previous, blanket rejection.)
        Ty::Named { args, .. } if !args.is_empty() => args.iter().all(|a| json_codable(*a, tys)),
        Ty::Base(_) | Ty::Named { .. } | Ty::Unit => true,
        Ty::Result(a, b) => json_codable(*a, tys) && json_codable(*b, tys),
        Ty::Option(a) | Ty::List(a) | Ty::Set(a) => json_codable(*a, tys),
        Ty::Map(k, v) => json_codable(*k, tys) && json_codable(*v, tys),
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
        | Ty::Var(_)
        | Ty::Actor(_)
        | Ty::ActorSum(_) => false,
    }
}

/// v0.22b: type the `Json` codec statics (ADR 0045). `encode(v) -> String`
/// over any codable value (it throws on a non-finite `Float`, per 0040 —
/// documented, not typed); `decode[T](s) -> Result[T, JsonError]` with `T`
/// explicit (`Json.decode[Order](s)`) or inferred from an expected
/// `Result[T, JsonError]`.
pub(crate) fn check_json_static(
    method: &Ident,
    type_args: &[TypeRef],
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let arity1 = |ctx: &mut Ctx| {
        if args.len() != 1 {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`Json.{}` takes 1 argument, got {}",
                    method.name,
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    match method.name.as_str() {
        "encode" => {
            if !type_args.is_empty() {
                ctx.errors.push(CompileError::new(
                    "bynk.generics.type_arg_mismatch",
                    span,
                    "`Json.encode` takes no type arguments — its type comes from the value",
                ));
            }
            if !arity1(ctx) {
                return None;
            }
            let t = type_of(&args[0], None, ctx)?;
            if let Some(rec) = first_recursive_generic(t, &ctx.input.types, tys) {
                ctx.errors.push(recursive_generic_error(&rec, args[0].span));
                return None;
            }
            if !json_codable(t, tys) {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.json_uncodable",
                        args[0].span,
                        format!("`{}` cannot be encoded as JSON", t.display(tys)),
                    )
                    .with_note(
                        "the codec covers base types, named types, and the built-in \
                         containers over them — not functions, effects, or error builtins",
                    ),
                );
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::String)))
        }
        "decode" => {
            if !arity1(ctx) {
                return None;
            }
            check_arg(
                &args[0],
                tys.intern(Ty::Base(BaseType::String)),
                "the `Json.decode` input",
                ctx,
            );
            let t = match type_args {
                // #712: report (not silently swallow) an unknown target type —
                // `Json.decode[Typo]("{}")` previously compiled clean.
                [one] => resolve_expr_type_ref(one, ctx)?,
                [] => match expected.map(|e| tys.get(e)).as_deref() {
                    Some(Ty::Result(t, e)) if *e == tys.intern(Ty::JsonError) => *t,
                    _ => {
                        ctx.errors.push(
                            CompileError::new(
                                "bynk.generics.uninferable_type_arg",
                                span,
                                "cannot infer the target type of `Json.decode`",
                            )
                            .with_note(
                                "give it explicitly (`Json.decode[Order](s)`) or use the \
                                 result where a `Result[T, JsonError]` is expected",
                            ),
                        );
                        return None;
                    }
                },
                _ => {
                    ctx.errors.push(CompileError::new(
                        "bynk.generics.type_arg_mismatch",
                        span,
                        format!(
                            "`Json.decode` takes exactly one type argument, got {}",
                            type_args.len()
                        ),
                    ));
                    return None;
                }
            };
            if let Some(rec) = first_recursive_generic(t, &ctx.input.types, tys) {
                ctx.errors.push(recursive_generic_error(&rec, span));
                return None;
            }
            if !json_codable(t, tys) || t == tys.intern(Ty::Unit) {
                ctx.errors.push(
                    CompileError::new(
                        "bynk.types.json_uncodable",
                        span,
                        format!("`{}` cannot be decoded from JSON", t.display(tys)),
                    )
                    .with_note(
                        "the codec covers base types, named types, and the built-in \
                         containers over them — not functions, effects, or error builtins",
                    ),
                );
                return None;
            }
            Some(tys.intern(Ty::Result(t, tys.intern(Ty::JsonError))))
        }
        _ => {
            // The resolver owns the unknown-static diagnostic.
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

/// v0.22a: type the numeric parse statics — `Int.parse(s)` /
/// `Float.parse(s)` (`-> Option[T]`, ADR 0048). Parsing is full-string
/// (trailing garbage is `None`, unlike `parseFloat`); an out-of-safe-range
/// `Int` or non-finite `Float` is `None`.
pub(crate) fn check_numeric_parse_static(
    type_name: &Ident,
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // K6b (#2598): `Float.pi()` and `Float.e()`, the two constants.
    if type_name.name == FLOAT && matches!(method.name.as_str(), "pi" | "e") {
        if !args.is_empty() {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`Float.{}` takes no arguments, got {}",
                    method.name,
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
        return Some(tys.intern(Ty::Base(BaseType::Float)));
    }
    if method.name != "parse" {
        // The resolver owns the unknown-static diagnostic; don't double up.
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`{}.parse` takes 1 argument, got {}",
                type_name.name,
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    check_arg(
        &args[0],
        tys.intern(Ty::Base(BaseType::String)),
        &format!("the `{}.parse` argument", type_name.name),
        ctx,
    );
    let inner = if type_name.name == INT {
        BaseType::Int
    } else {
        BaseType::Float
    };
    Some(tys.intern(Ty::Option(tys.intern(Ty::Base(inner)))))
}

/// v0.86 (ADR 0112): type the `Duration.millis(n: Int) -> Duration` static
/// constructor — building a `Duration` from a runtime `Int` of milliseconds.
pub(crate) fn check_duration_static(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if method.name != "millis" {
        // The resolver owns the unknown-static diagnostic; don't double up.
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`Duration.millis` takes 1 argument, got {}", args.len()),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    check_arg(
        &args[0],
        tys.intern(Ty::Base(BaseType::Int)),
        "the `Duration.millis` argument",
        ctx,
    );
    Some(tys.intern(Ty::Base(BaseType::Duration)))
}

/// v0.90 (ADR 0114 D6): the `Instant.fromEpochMillis(n)` static constructor —
/// the way to build an `Instant` from a runtime `Int` (a wire/stored epoch
/// value); instants are otherwise minted only by `Clock.now()`. K3 (#2592):
/// `Instant.fromIso(s)` reads RFC 3339 text, `None` on anything else.
pub(crate) fn check_instant_static(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // (param, return) keyed on the static name; `None` = unknown (the resolver
    // owns the unknown-static diagnostic; don't double up).
    let sig: Option<(Ty, Ty)> = match method.name.as_str() {
        "fromEpochMillis" => Some((Ty::Base(BaseType::Int), Ty::Base(BaseType::Instant))),
        "fromIso" => Some((
            Ty::Base(BaseType::String),
            Ty::Option(tys.intern(Ty::Base(BaseType::Instant))),
        )),
        _ => None,
    };
    let Some((param, ret)) = sig else {
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Instant.{}` takes 1 argument, got {}",
                method.name,
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    check_arg(
        &args[0],
        tys.intern(param),
        &format!("the `Instant.{}` argument", method.name),
        ctx,
    );
    Some(tys.intern(ret))
}

/// v0.110 (ADR 0142 D2): the `Bytes` static constructors — the only way to
/// build a `Bytes` (no source literal). `fromUtf8(s)` is total (the UTF-8
/// encoding of a string); `fromBase64(s)` is partial (`None` on invalid
/// base64); `empty()` is the zero value (the empty octet sequence).
/// #2016: `fromHex(s)` and `fromBase64Url(s)` are partial in the same way.
pub(crate) fn check_bytes_static(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    // (params, return) keyed on the static name; `None` = unknown (the
    // resolver owns the unknown-static diagnostic; don't double up).
    let sig: Option<(Vec<TyId>, Ty)> = match method.name.as_str() {
        "fromUtf8" => Some((
            vec![tys.intern(Ty::Base(BaseType::String))],
            Ty::Base(BaseType::Bytes),
        )),
        "fromBase64" => Some((
            vec![tys.intern(Ty::Base(BaseType::String))],
            Ty::Option(tys.intern(Ty::Base(BaseType::Bytes))),
        )),
        "fromHex" | "fromBase64Url" => Some((
            vec![tys.intern(Ty::Base(BaseType::String))],
            Ty::Option(tys.intern(Ty::Base(BaseType::Bytes))),
        )),
        "empty" => Some((vec![], Ty::Base(BaseType::Bytes))),
        _ => None,
    };
    let Some((params, ret)) = sig else {
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`Bytes.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (a, p) in args.iter().zip(params.iter()) {
        check_arg(a, *p, &format!("the `Bytes.{}` argument", method.name), ctx);
    }
    Some(tys.intern(ret))
}

/// #2341 (L4): the `RequestHeaders` static constructors, the test-side way to
/// build the headers an HTTP handler reads. `of(lines)` takes `"name: value"`
/// lines (a line without a colon is ignored); `empty()` has no headers.
pub(crate) fn check_request_headers_static(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let params: Vec<TyId> = match method.name.as_str() {
        "of" => vec![tys.intern(Ty::List(tys.intern(Ty::Base(BaseType::String))))],
        "empty" => vec![],
        _ => {
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return None;
        }
    };
    if args.len() != params.len() {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!(
                "`RequestHeaders.{}` takes {} argument{}, got {}",
                method.name,
                params.len(),
                if params.len() == 1 { "" } else { "s" },
                args.len()
            ),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    for (a, p) in args.iter().zip(params.iter()) {
        check_arg(
            a,
            *p,
            &format!("the `RequestHeaders.{}` argument", method.name),
            ctx,
        );
    }
    Some(tys.intern(Ty::RequestHeaders))
}

/// #2380 (L5): `TokenClaims.of(lines)`, the test-side way to build the claims
/// an `Oidc` actor's handler reads: `"name: value"` lines, each a string claim.
pub(crate) fn check_token_claims_static(
    method: &Ident,
    args: &[Expr],
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if method.name != "of" {
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    if args.len() != 1 {
        ctx.errors.push(CompileError::new(
            "bynk.types.method_arity",
            span,
            format!("`TokenClaims.of` takes 1 argument, got {}", args.len()),
        ));
        for a in args {
            let _ = type_of(a, None, ctx);
        }
        return None;
    }
    let lines = tys.intern(Ty::List(tys.intern(Ty::Base(BaseType::String))));
    check_arg(&args[0], lines, "the `TokenClaims.of` argument", ctx);
    Some(tys.intern(Ty::TokenClaims))
}

/// v0.20b: type a built-in `Map[K, V]` kernel method.
pub(crate) fn check_map_kernel_method(
    method: &Ident,
    args: &[Expr],
    key: TyId,
    val: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    let arity = |n: usize, ctx: &mut Ctx| {
        if args.len() != n {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_arity",
                span,
                format!(
                    "`Map.{}` takes {n} argument{}, got {}",
                    method.name,
                    if n == 1 { "" } else { "s" },
                    args.len()
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            return false;
        }
        true
    };
    match method.name.as_str() {
        "length" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(tys.intern(Ty::Base(BaseType::Int)))
        }
        "keys" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(tys.intern(Ty::List(key)))
        }
        // v0.149 (ADR 0173): the `keys` sibling — the values in key order.
        "values" => {
            if !arity(0, ctx) {
                return None;
            }
            Some(tys.intern(Ty::List(val)))
        }
        "get" => {
            if !arity(1, ctx) {
                return None;
            }
            check_arg(&args[0], key, "the `Map.get` key", ctx);
            Some(tys.intern(Ty::Option(val)))
        }
        "insert" => {
            if !arity(2, ctx) {
                return None;
            }
            check_arg(&args[0], key, "the `Map.insert` key", ctx);
            check_arg(&args[1], val, "the `Map.insert` value", ctx);
            Some(tys.intern(Ty::Map(key, val)))
        }
        _ => {
            ctx.errors.push(CompileError::new(
                "bynk.types.method_not_found",
                method.span,
                format!(
                    "the built-in `Map[{}, {}]` type has no method `{}` — the kernel is `length`, `keys`, `values`, `get`, `insert`",
                    key.display(tys),
                    val.display(tys),
                    method.name
                ),
            ));
            for a in args {
                let _ = type_of(a, None, ctx);
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    /// Each kernel call is bound by an unannotated `let`, so its type is
    /// whatever the kernel's own dispatch decided, not something a declared
    /// type pushed onto it.
    const PROBE: &str = "commons demo\n\n\
fn probe(s: String, n: Int, xs: List[Int], m: Map[String, Int], o: Option[Int], r: Result[Int, String]) -> Int {
  let a = s.length()
  let b = s.split(\",\")
  let c = s.indexOf(\"x\")
  let d = xs.get(0)
  let e = xs.fold(0, (acc, x) => acc + x)
  let f = xs.prepend(1)
  let f2 = xs.append(4)
  let f3 = xs.concat([5, 6])
  let g = m.keys()
  let h = m.get(\"k\")
  let i = m.insert(\"k\", 1)
  let j = o.map((x) => x.toString())
  let k = o.okOr(\"none\")
  let l = r.mapErr((err) => err.length())
  let p = n.clamp(0, 10)
  let q = n.toFloat()
  0
}
";

    #[test]
    fn each_kernel_method_has_its_registered_result_type() {
        let a = analyse(&[("demo.bynk", PROBE)]);
        let cases = [
            ("s.length()", "Int"),
            ("s.split(\",\")", "List[String]"),
            ("s.indexOf(\"x\")", "Option[Int]"),
            ("xs.get(0)", "Option[Int]"),
            ("xs.fold(0, (acc, x) => acc + x)", "Int"),
            ("xs.prepend(1)", "List[Int]"),
            ("xs.append(4)", "List[Int]"),
            ("xs.concat([5, 6])", "List[Int]"),
            ("m.keys()", "List[String]"),
            ("m.get(\"k\")", "Option[Int]"),
            ("m.insert(\"k\", 1)", "Map[String, Int]"),
            ("o.map((x) => x.toString())", "Option[String]"),
            ("o.okOr(\"none\")", "Result[Int, String]"),
            ("r.mapErr((err) => err.length())", "Result[Int, Int]"),
            ("n.clamp(0, 10)", "Int"),
            ("n.toFloat()", "Float"),
        ];
        for (expr, want) in cases {
            assert_eq!(a.type_at("demo.bynk", expr), want, "type of `{expr}`");
        }
    }

    fn reports(body: &str, category: &str) {
        analyse(&[("demo.bynk", &format!("commons demo\n\n{body}"))]).assert_reports(category);
    }

    #[test]
    fn an_unknown_kernel_method_is_not_found() {
        reports(
            "fn f(s: String) -> Int { s.nope() }\n",
            "bynk.types.method_not_found",
        );
    }

    #[test]
    fn a_kernel_method_with_the_wrong_argument_count_is_an_arity_error() {
        reports(
            "fn f(s: String) -> String { s.trim(1) }\n",
            "bynk.types.method_arity",
        );
    }

    #[test]
    fn a_kernel_method_argument_of_the_wrong_type_is_a_mismatch() {
        reports(
            "fn f(s: String) -> Bool { s.contains(1) }\n",
            "bynk.types.type_mismatch",
        );
    }

    /// #1889: `append` checks its item against the receiver's element type.
    #[test]
    fn appending_an_item_of_the_wrong_type_is_a_mismatch() {
        reports(
            "fn f(xs: List[Int]) -> List[Int] { xs.append(\"s\") }\n",
            "bynk.types.type_mismatch",
        );
    }

    /// #1889: `concat` joins lists of one element type only.
    #[test]
    fn concatenating_a_list_of_another_element_type_is_a_mismatch() {
        reports(
            "fn f(xs: List[Int], ys: List[String]) -> List[Int] { xs.concat(ys) }\n",
            "bynk.types.type_mismatch",
        );
    }

    /// #1889: an empty receiver or argument takes its element type from the
    /// context, so a fold can start from `[]` and build in order.
    #[test]
    fn append_and_concat_infer_through_empty_lists() {
        let a = analyse(&[(
            "demo.bynk",
            "commons demo\n\n\
fn f(xs: List[Int]) -> List[Int] {
  let a: List[Int] = [].append(1)
  let b: List[Int] = [].concat([2])
  let c = xs.concat([])
  let d = xs.fold(a, (acc, x) => acc.append(x))
  let e = xs.fold(b, (acc, x) => acc.concat([x]))
  c.concat(d).concat(e)
}
",
        )]);
        a.assert_clean();
        assert_eq!(a.type_at("demo.bynk", "xs.concat([])"), "List[Int]");
    }

    /// #1889: inside a fold, the accumulator's element type fixes what
    /// `append` accepts, so appending a `String` to a `List[Int]` is caught.
    #[test]
    fn a_wrong_typed_append_inside_a_fold_is_a_mismatch() {
        reports(
            "fn f(xs: List[Int]) -> List[Int] { xs.fold([0], (acc, x) => acc.append(x.toString())) }\n",
            "bynk.types.type_mismatch",
        );
    }

    #[test]
    fn an_and_then_step_that_does_not_return_an_option_is_rejected() {
        reports(
            "fn f(o: Option[Int]) -> Option[Int] { o.andThen((x) => x) }\n",
            "bynk.types.combinator_return_mismatch",
        );
    }

    #[test]
    fn a_result_and_then_step_that_does_not_return_a_result_is_rejected() {
        reports(
            "fn f(r: Result[Int, String]) -> Result[Int, String] { r.andThen((x) => x) }\n",
            "bynk.types.combinator_return_mismatch",
        );
    }
}
