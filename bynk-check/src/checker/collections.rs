//! #3961: the `Map` and `Set` methods that bring them level with `List` (ADR
//! 0116): `remove`, `contains`, `getOrElse`, `isEmpty`, `entries`, `filter`,
//! `mapValues` and `fold` on a `Map`, `Map.of`, and the set algebra plus
//! `isEmpty`, `filter`, `map` and `fold` on a `Set`. Kept apart from
//! `kernels.rs` so that file does not grow.

use super::calls::check_arg;
use super::kernels::check_kernel_fn_arg;
use super::keying::{KeySite, check_key};
use super::*;
use crate::kernel_op::{MapOp, SetOp};

/// A wrong argument count: reports `bynk.types.method_arity` and types the
/// arguments for their own diagnostics. `true` when the count is right.
fn arity(what: &str, name: &str, n: usize, args: &[Expr], span: Span, ctx: &mut Ctx) -> bool {
    if args.len() == n {
        return true;
    }
    ctx.errors.push(CompileError::new(
        "bynk.types.method_arity",
        span,
        format!(
            "`{what}.{name}` takes {n} argument{}, got {}",
            if n == 1 { "" } else { "s" },
            args.len()
        ),
    ));
    for a in args {
        let _ = type_of(a, None, ctx);
    }
    false
}

/// The `Map` methods #3961 adds; `None` for one it does not own.
pub(super) fn check_map_more(
    method: &Ident,
    op: MapOp,
    args: &[Expr],
    key: TyId,
    val: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let tys = ctx.tys;
    let name = method.name.as_str();
    let n = match op {
        MapOp::IsEmpty | MapOp::Entries => 0,
        MapOp::Remove | MapOp::Contains | MapOp::Filter | MapOp::MapValues => 1,
        MapOp::GetOrElse | MapOp::Fold => 2,
        _ => return None,
    };
    if !arity("Map", name, n, args, span, ctx) {
        return Some(None);
    }
    let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
    // `?` inside returns from this closure only: a failed argument is reported
    // already and types as `None`, which is still one of our methods.
    let typed = |ctx: &mut Ctx| -> Option<TyId> {
        match op {
            MapOp::IsEmpty => Some(bool_ty),
            MapOp::Entries => Some(tys.intern(Ty::List(map_entry_ty(key, val, tys)))),
            MapOp::Remove => {
                check_arg(&args[0], key, "the `Map.remove` key", ctx);
                Some(tys.intern(Ty::Map(key, val)))
            }
            MapOp::Contains => {
                check_arg(&args[0], key, "the `Map.contains` key", ctx);
                Some(bool_ty)
            }
            MapOp::GetOrElse => {
                check_arg(&args[0], key, "the `Map.getOrElse` key", ctx);
                check_arg(&args[1], val, "the `Map.getOrElse` default", ctx);
                Some(val)
            }
            MapOp::Filter => {
                let p = Ty::Fn {
                    params: vec![key, val],
                    ret: bool_ty,
                };
                check_arg(&args[0], tys.intern(p), "the `Map.filter` predicate", ctx);
                Some(tys.intern(Ty::Map(key, val)))
            }
            MapOp::MapValues => {
                let u =
                    check_kernel_fn_arg(&args[0], vec![val], "the `Map.mapValues` function", ctx)?;
                Some(tys.intern(Ty::Map(key, u)))
            }
            MapOp::Fold => {
                let acc = type_of(&args[0], ctx.fold_init_expected.take(), ctx)?;
                let step = Ty::Fn {
                    params: vec![acc, key, val],
                    ret: acc,
                };
                check_arg(
                    &args[1],
                    tys.intern(step),
                    "the `Map.fold` step function",
                    ctx,
                );
                Some(acc)
            }
            _ => unreachable!("filtered above"),
        }
    };
    Some(typed(ctx))
}

/// The `Set` methods #3961 adds; `None` for one it does not own.
pub(super) fn check_set_more(
    method: &Ident,
    op: SetOp,
    args: &[Expr],
    elem: TyId,
    span: Span,
    ctx: &mut Ctx,
) -> Option<Option<TyId>> {
    let tys = ctx.tys;
    let name = method.name.as_str();
    let n = match op {
        SetOp::IsEmpty => 0,
        SetOp::Union
        | SetOp::Intersect
        | SetOp::Difference
        | SetOp::IsSubsetOf
        | SetOp::Filter
        | SetOp::Map => 1,
        SetOp::Fold => 2,
        _ => return None,
    };
    if !arity("Set", name, n, args, span, ctx) {
        return Some(None);
    }
    let set = tys.intern(Ty::Set(elem));
    let bool_ty = tys.intern(Ty::Base(BaseType::Bool));
    // `?` inside returns from this closure only: a failed argument is reported
    // already and types as `None`, which is still one of our methods.
    let typed = |ctx: &mut Ctx| -> Option<TyId> {
        match op {
            SetOp::IsEmpty => Some(bool_ty),
            SetOp::Union | SetOp::Intersect | SetOp::Difference => {
                check_arg(&args[0], set, &format!("the `Set.{name}` argument"), ctx);
                Some(set)
            }
            SetOp::IsSubsetOf => {
                check_arg(&args[0], set, "the `Set.isSubsetOf` argument", ctx);
                Some(bool_ty)
            }
            SetOp::Filter => {
                let p = Ty::Fn {
                    params: vec![elem],
                    ret: bool_ty,
                };
                check_arg(&args[0], tys.intern(p), "the `Set.filter` predicate", ctx);
                Some(set)
            }
            SetOp::Map => {
                let u = check_kernel_fn_arg(&args[0], vec![elem], "the `Set.map` function", ctx)?;
                check_key(ctx, u, args[0].span, &KeySite::SetElement);
                Some(tys.intern(Ty::Set(u)))
            }
            SetOp::Fold => {
                let acc = type_of(&args[0], ctx.fold_init_expected.take(), ctx)?;
                let step = Ty::Fn {
                    params: vec![acc, elem],
                    ret: acc,
                };
                check_arg(
                    &args[1],
                    tys.intern(step),
                    "the `Set.fold` step function",
                    ctx,
                );
                Some(acc)
            }
            _ => unreachable!("filtered above"),
        }
    };
    Some(typed(ctx))
}

/// `Map.of(entries: List[MapEntry[K, V]]) -> Map[K, V]`: a later entry for a key
/// replaces the value and keeps the first position. An inferred key type never
/// passes the resolver's `Map[K, V]` check, so the key rule is applied here.
pub(super) fn check_map_of(
    args: &[Expr],
    span: Span,
    expected: Option<TyId>,
    ctx: &mut Ctx,
) -> Option<TyId> {
    let tys = ctx.tys;
    if !arity("Map", "of", 1, args, span, ctx) {
        return None;
    }
    let expected_list = expected
        .and_then(|t| peel_to_map(t, tys))
        .filter(|(k, v)| !contains_var(*k, tys) && !contains_var(*v, tys))
        .map(|(k, v)| tys.intern(Ty::List(map_entry_ty(k, v, tys))));
    let list = type_of(&args[0], expected_list, ctx)?;
    let entry = peel_to_list(list, tys).and_then(|e| match &*tys.get(e) {
        Ty::Named { name, args, .. } if name == MAP_ENTRY && args.len() == 2 => {
            Some((args[0], args[1]))
        }
        _ => None,
    });
    let Some((key, val)) = entry else {
        ctx.errors.push(CompileError::new(
            "bynk.types.type_mismatch",
            args[0].span,
            format!(
                "the `Map.of` argument has type `{}`, but a `List[MapEntry[K, V]]` is required",
                list.display(tys)
            ),
        ));
        return None;
    };
    check_key(ctx, key, args[0].span, &KeySite::MapKey);
    Some(tys.intern(Ty::Map(key, val)))
}

/// The `bynk.map` and `bynk.set` free functions and the method form each rewrites
/// to: `(module, function, method, arity)`. One table, read by the warning below and by
/// the wasm `bynk_reference` catalogue so the two cannot drift.
pub const BYNK_COLLECTION_DEPRECATED: &[(&str, &str, &str, usize)] = &[
    ("bynk.map", "values", "values", 1),
    ("bynk.map", "contains", "contains", 2),
    ("bynk.map", "getOr", "getOrElse", 3),
    ("bynk.set", "union", "union", 2),
    ("bynk.set", "intersect", "intersect", 2),
    ("bynk.set", "difference", "difference", 2),
    ("bynk.set", "isSubsetOf", "isSubsetOf", 2),
    ("bynk.set", "filterSet", "filter", 2),
    ("bynk.set", "mapSet", "map", 2),
];

/// A call to a deprecated `bynk.map`/`bynk.set` function: a non-failing warning
/// (ADR 0117) with a machine-applicable rewrite to the method form,
/// `union(a, b)` to `a.union(b)` and `getOr(m, k, d)` to `m.getOrElse(k, d)`.
pub(super) fn warn_deprecated_collection_fn(
    name: &Ident,
    args: &[Expr],
    call_span: Span,
    ctx: &mut Ctx,
) {
    let Some(module) = ctx.input.imported_from.get(&name.name).cloned() else {
        return;
    };
    let Some(&(_, _, method, arity)) = BYNK_COLLECTION_DEPRECATED
        .iter()
        .find(|(m, f, _, _)| *m == module && *f == name.name)
    else {
        return;
    };
    let code = if module == "bynk.map" {
        "bynk.map.deprecated_function"
    } else {
        "bynk.set.deprecated_function"
    };
    let mut err = CompileError::new(
        code,
        name.span,
        format!(
            "`{module}.{}` is deprecated: use the `{}` method `.{method}(…)`",
            name.name,
            if module == "bynk.map" { "Map" } else { "Set" }
        ),
    )
    .with_note(format!(
        "the `{module}` free functions are superseded by the `{}` methods (#3961); the method form reads left to right and chains",
        if module == "bynk.map" { "Map" } else { "Set" }
    ));
    // The rewrite needs the function's own shape; a wrong-arity call is reported
    // elsewhere and gets no fix.
    if args.len() == arity {
        let mut edits = vec![(
            Span::new(name.span.start, args[0].span.start),
            String::new(),
        )];
        edits.push(if args.len() == 1 {
            (
                Span::new(args[0].span.end, call_span.end),
                format!(".{method}()"),
            )
        } else {
            (
                Span::new(args[0].span.end, args[1].span.start),
                format!(".{method}("),
            )
        });
        err = err.with_suggestion(
            format!("rewrite to the `.{method}(…)` method form"),
            edits,
            Applicability::MachineApplicable,
        );
    }
    ctx.errors.push(err);
}
