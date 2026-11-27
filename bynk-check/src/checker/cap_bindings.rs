//! #3959: binding a commons function's capability parameters at a call.
//!
//! A call binds every capability parameter of its callee explicitly, inside
//! the argument list after `given` (`cached(id, load, given Store = Sessions)`),
//! to a capability the caller holds. Nothing is inferred or defaulted, even
//! when the caller holds exactly one candidate (track §11): an unbound
//! parameter is `bynk.given.binding_missing`, and its fix is machine-applicable
//! only when there is exactly one candidate, so `bynk fix` never chooses
//! between instances.

use super::Ctx;
use bynk_syntax::ast::{CapBinding, CapRef, Expr, FnDecl, Ident};
use bynk_syntax::error::{Applicability, CompileError};
use bynk_syntax::span::Span;

/// The first-party `(unit, kind)` an instantiable kind name is, as a
/// capability parameter's bound names it (`Kv`). A bound may only name a kind
/// a first-party unit lists as instantiable (`bynk.given.shared_as_parameter`
/// otherwise), and those names are unique across the units.
pub(crate) fn bound_kind(kind: &str) -> Option<(&'static str, &'static str)> {
    bynk_std::UNITS.iter().find_map(|u| {
        u.instantiable
            .iter()
            .find(|i| i.capability == kind)
            .map(|i| (u.name, i.capability))
    })
}

/// The kind a capability the caller holds is of, as `(unit, kind)`: a derived
/// instance's base (`Sessions` from `capability Sessions = Kv`), a commons
/// caller's own parameter's bound, or the bare kind itself when it is a
/// flattened first-party instantiable (`given Kv`). `None` for anything else.
fn held_kind(key: &str, ctx: &Ctx) -> Option<(String, String)> {
    if let Some(info) = ctx.caps.capabilities.get(key)
        && let Some((unit, base)) = &info.derived_from
    {
        return Some((unit.clone(), base.clone()));
    }
    let unit = ctx.input.cross_context.flattened_caps.get(key)?;
    bynk_std::instantiable(unit, key).map(|_| (unit.clone(), key.to_string()))
}

/// The call's ordinary arguments, bindings and span, for the binding fix's
/// insertion point.
pub(crate) struct CallSite<'a> {
    pub args: &'a [Expr],
    pub bindings: &'a [CapBinding],
    pub span: Span,
}

/// Check a call's bindings against `callee`'s capability parameters, marking
/// each bound capability used in the caller. Shared capabilities are left to
/// the inclusion rule in `require_callee_given`.
pub(crate) fn check_call_bindings(name: &Ident, callee: &FnDecl, site: &CallSite, ctx: &mut Ctx) {
    let params: Vec<&CapRef> = callee.given.iter().filter(|g| g.is_parameter()).collect();
    // The in-argument binding is experimental (track §12).
    if let Some(first) = site.bindings.first()
        && let Some(err) = crate::cap_params::gate(&ctx.input.stability.policy, first.span)
    {
        ctx.errors.push(err);
    }
    let mut seen: Vec<&str> = Vec::new();
    for b in site.bindings {
        let param = b.param.name.as_str();
        if seen.contains(&param) {
            ctx.errors.push(
                CompileError::new(
                    "bynk.given.binding_duplicate",
                    b.span,
                    format!("`{}` binds `{param}` more than once", name.name),
                )
                .with_note("each capability parameter is bound exactly once: remove the repeat"),
            );
            continue;
        }
        seen.push(param);
        if let Some(shared) = callee
            .given
            .iter()
            .find(|g| !g.is_parameter() && g.key() == param)
        {
            ctx.errors.push(binding_shared(name, shared, b));
            continue;
        }
        let Some(p) = params.iter().find(|p| p.key() == param) else {
            ctx.errors.push(binding_unknown(name, callee, &params, b));
            continue;
        };
        check_binding_value(name, p, b, ctx);
    }
    for p in &params {
        if !seen.contains(&p.key()) {
            let (err, candidates) = binding_missing(name, p, site, ctx);
            // A candidate is not reported unused while a call is unbound:
            // that warning's machine-applicable removal would take away the
            // instance the binding's fix offers.
            ctx.caps.given_used.extend(candidates);
            ctx.errors.push(err);
        }
    }
}

/// The right-hand side of one binding: a capability the caller holds, of the
/// parameter's kind.
fn check_binding_value(name: &Ident, param: &CapRef, b: &CapBinding, ctx: &mut Ctx) {
    let key = b.value.key().to_string();
    if b.value.is_cross_context() || !ctx.caps.given_remaining.contains(&key) {
        if ctx.in_test_body {
            return;
        }
        ctx.errors.push(
            CompileError::new(
                "bynk.given.missing_for_call",
                b.value.span,
                format!(
                    "`{}` binds `{}` to `{}`, which the enclosing body does not hold",
                    name.name,
                    param.key(),
                    b.value.display_name()
                ),
            )
            .with_note(format!(
                "a binding's right-hand side is a capability in the caller's own `given`: add `{}` to it, or bind a capability the caller holds",
                b.value.display_name()
            )),
        );
        return;
    }
    ctx.caps.given_used.insert(key.clone());
    let want = param.bound.as_ref().map(|k| k.name.as_str()).unwrap_or("");
    let got = held_kind(&key, ctx);
    let matches = got
        .as_ref()
        .is_some_and(|(unit, kind)| Some((unit.as_str(), kind.as_str())) == bound_kind(want));
    if !matches {
        let what = match &got {
            Some((_, kind)) => format!("a `{kind}`"),
            None => "not an instance of a capability kind".to_string(),
        };
        ctx.errors.push(
            CompileError::new(
                "bynk.given.binding_kind_mismatch",
                b.value.span,
                format!(
                    "`{}` is {what}, but `{}` takes a `{want}`",
                    b.value.display_name(),
                    param.key()
                ),
            )
            .with_label(param.span, format!("`{}` is declared here", param.key()))
            .with_note(format!(
                "bind an instance of `{want}` (one declared `capability Name = {want}`), or the bare `{want}` where the kind allows one; the kind must match exactly"
            )),
        );
    }
}

fn binding_shared(name: &Ident, shared: &CapRef, b: &CapBinding) -> CompileError {
    CompileError::new(
        "bynk.given.binding_shared",
        b.span,
        format!(
            "`{}` is a shared capability of `{}`, so it is not bound",
            shared.key(),
            name.name
        ),
    )
    .with_note(format!(
        "a shared capability means the caller's one `{0}`, which the caller's own `given` supplies: remove `{0} = …` from the bindings",
        shared.key()
    ))
}

fn binding_unknown(
    name: &Ident,
    callee: &FnDecl,
    params: &[&CapRef],
    b: &CapBinding,
) -> CompileError {
    let err = CompileError::new(
        "bynk.given.binding_unknown",
        b.param.span,
        format!(
            "`{}` has no capability parameter named `{}`",
            name.name, b.param.name
        ),
    )
    .with_label(
        callee.name.ident().span,
        format!("`{}` is declared here", name.name),
    );
    if params.is_empty() {
        err.with_note("it declares no capability parameters, so there is nothing to bind: remove `given …` from the arguments")
    } else {
        let names: Vec<&str> = params.iter().map(|p| p.key()).collect();
        err.with_note(format!(
            "its capability parameters are {}",
            names.join(", ")
        ))
    }
}

/// `bynk.given.binding_missing`, with one fix per capability of the
/// parameter's kind the caller holds. The fix is machine-applicable only when
/// there is exactly one candidate (track §11): with two, `bynk fix` leaves the
/// call alone, because choosing an instance is the author's decision.
fn binding_missing(
    name: &Ident,
    param: &CapRef,
    site: &CallSite,
    ctx: &Ctx,
) -> (CompileError, Vec<String>) {
    let want = param.bound.as_ref().map(|k| k.name.as_str()).unwrap_or("");
    let kind = bound_kind(want);
    let candidates: Vec<&str> = ctx
        .caps
        .given_entries
        .iter()
        .map(|(k, _)| k.as_str())
        .filter(|k| held_kind(k, ctx).is_some_and(|(u, c)| Some((u.as_str(), c.as_str())) == kind))
        .fold(Vec::new(), |mut acc, k| {
            if !acc.contains(&k) {
                acc.push(k);
            }
            acc
        });
    let mut err = CompileError::new(
        "bynk.given.binding_missing",
        name.span,
        format!(
            "`{}` leaves its capability parameter `{}: {want}` unbound",
            name.name,
            param.key()
        ),
    )
    .with_label(param.span, format!("`{}` is declared here", param.key()))
    .with_note(format!(
        "every call binds each capability parameter explicitly, and nothing is inferred: add `given {} = <a {want} the caller holds>` after the arguments",
        param.key()
    ));
    let applicability = if candidates.len() == 1 {
        Applicability::MachineApplicable
    } else {
        Applicability::MaybeIncorrect
    };
    for c in &candidates {
        let (span, text) = binding_insertion(site, param.key(), c);
        err = err.with_suggestion(
            format!("bind `{}` to `{c}`", param.key()),
            vec![(span, text)],
            applicability,
        );
    }
    (err, candidates.into_iter().map(str::to_string).collect())
}

/// Where and what to insert to bind `param` to `value` at `site`: after the
/// last binding, else after the last argument, else just inside the `)`.
fn binding_insertion(site: &CallSite, param: &str, value: &str) -> (Span, String) {
    if let Some(last) = site.bindings.last() {
        let at = last.span.end;
        return (
            Span::new_in(site.span.file, at, at),
            format!(", {param} = {value}"),
        );
    }
    if let Some(last) = site.args.last() {
        let at = last.span.end;
        return (
            Span::new_in(site.span.file, at, at),
            format!(", given {param} = {value}"),
        );
    }
    let at = site.span.end.saturating_sub(1);
    (
        Span::new_in(site.span.file, at, at),
        format!("given {param} = {value}"),
    )
}

/// Bindings on a call that is not a call to a function (a variant
/// constructor, a record's), which declares no capability parameters.
pub(crate) fn binding_on_a_non_function(name: &Ident, b: &CapBinding) -> CompileError {
    CompileError::new(
        "bynk.given.binding_unknown",
        b.param.span,
        format!(
            "`{}` is not a function, so it has no capability parameter `{}`",
            name.name, b.param.name
        ),
    )
    .with_note("only a call to a commons function with capability parameters takes bindings: remove `given …` from the arguments")
}

/// `bynk.given.parameterised_fn_value` (track Q8): a function with capability
/// parameters named as a value. A binding must be chosen, and a lambda that
/// binds says which explicitly; partial application of bindings is not a
/// value form.
pub(crate) fn parameterised_fn_value(name: &Ident, callee: &FnDecl) -> CompileError {
    let params: Vec<String> = callee
        .given
        .iter()
        .filter(|g| g.is_parameter())
        .map(|g| format!("{} = …", g.key()))
        .collect();
    CompileError::new(
        "bynk.given.parameterised_fn_value",
        name.span,
        format!(
            "`{}` takes capability parameters, so it cannot be named as a value",
            name.name
        ),
    )
    .with_label(callee.name.ident().span, format!("`{}` is declared here", name.name))
    .with_note(format!(
        "every call binds its parameters, and a value would leave them unchosen: write a lambda that binds them, as in `{}`",
        binding_lambda(&name.name, callee, &params)
    ))
}

/// The lambda `parameterised_fn_value`'s note suggests, over the callee's own
/// parameters: `(key, value) => put(key, value, given Inner = …)`.
fn binding_lambda(name: &str, callee: &FnDecl, bindings: &[String]) -> String {
    let args: Vec<&str> = callee.params.iter().map(|p| p.name.name.as_str()).collect();
    let args = args.join(", ");
    let sep = if args.is_empty() { "" } else { ", " };
    format!(
        "({args}) => {name}({args}{sep}given {})",
        bindings.join(", ")
    )
}
