//! #3959: capability parameters and shared capabilities on a commons free
//! function (track `design/tracks/capability-parameters.md`).
//!
//! A commons selects first-party capabilities in its braced `consumes`
//! (`consumes bynk.cloudflare { Kv }`), contract only. A free function then
//! declares each one in its `given` clause by the rule "instantiable means
//! parameter, not instantiable means shared": `given Store: Kv, Logger`. The
//! call-site half, binding each parameter, is `checker::cap_bindings`.

use crate::checker::{self, CapabilityInfo, Types};
use crate::context_checks;
use crate::hints::HintSink;
use crate::index::RefSink;
use crate::locals::LocalsSink;
use crate::requirements::RequirementSink;
use crate::resolver;
use crate::symbols::UnitTable;
use bynk_syntax::CompileError;
use bynk_syntax::ast::{CapRef, CommonsItem, FnDecl};
use bynk_syntax::error::Applicability;
use bynk_syntax::span::Span;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// The `[unstable] allow` entry that opts a project into capability
/// parameters: the parameter form, the in-argument binding, a capability in a
/// commons' `consumes`, and a commons suite's suite-local instances (track
/// §12). Built in: no unit exports it.
pub const GATE: &str = "bynk.given.parameters";

/// `bynk.given.parameter_outside_commons`: `given Name: Kind` anywhere but a
/// commons free function (track Q6).
pub(crate) fn parameter_outside_commons(cap: &CapRef) -> CompileError {
    CompileError::new(
        "bynk.given.parameter_outside_commons",
        cap.span,
        format!(
            "`{}: {}` is a capability parameter, which only a commons function may declare",
            cap.key(),
            cap.bound.as_ref().map_or("", |b| b.name.as_str())
        ),
    )
    .with_note("a context function, a handler, a provider and a suite helper name the capabilities they use directly: write `given Sessions`; a helper over several instances lives in a commons, which the context `uses`")
}

/// The experimental-gate error for a capability-parameter form at `span`,
/// unless the project allows [`GATE`].
pub(crate) fn gate(policy: &crate::stability::StabilityPolicy, span: Span) -> Option<CompileError> {
    policy.check(GATE, span)
}

/// Type-check this commons file's free functions that declare `given`, with
/// the capabilities their clauses name in scope. A refused declaration
/// (`given_fn_refusal`) was reported by the plain function check.
pub fn check_commons_given_fns(
    typed: &mut checker::TypedCommons,
    table: &UnitTable,
    // The file's own `consumes`, which the synthetic commons does not carry.
    consumes: &[bynk_syntax::ast::ConsumesDecl],
    cross_context: &resolver::CrossContextInfo,
    uses_commons_type_names: &HashSet<String>,
    (refs, hints, locals, requirements): (
        &mut RefSink,
        &mut HintSink,
        &mut LocalsSink,
        &mut RequirementSink,
    ),
    tys: &Arc<Types>,
) -> Vec<CompileError> {
    let mut errors = unused_selected_capabilities(&typed.commons, consumes, cross_context);
    let fns: Vec<Arc<FnDecl>> = typed
        .commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Fn(f) if !f.given.is_empty() => Some(Arc::new(f.clone())),
            _ => None,
        })
        .collect();
    if fns.is_empty() {
        return errors;
    }
    let resolved = context_checks::declarations_resolved(
        typed,
        table,
        cross_context,
        false,
        uses_commons_type_names,
    )
    .with_is_commons(true);
    let selected = context_checks::flattened_capability_infos(typed, cross_context, tys);
    for f in fns {
        if checker::given_fn_refusal(&f, true).is_some() {
            continue;
        }
        let Some(capabilities) =
            commons_fn_capabilities(&f, &selected, cross_context, &resolved, &mut errors, refs)
        else {
            continue;
        };
        refs.set_owner(f.name.display());
        checker::check_context_fn(
            &f,
            checker::FnCaps {
                capabilities,
                declared_capabilities: selected.clone(),
            },
            &resolved,
            &mut typed.expr_types,
            &mut typed.callees,
            &mut errors,
            refs,
            hints,
            locals,
            requirements,
            tys,
        );
        refs.clear_owner();
    }
    errors
}

/// The capabilities a commons function's `given` puts in scope, by deps key:
/// each parameter as its bound kind's contract under the parameter's name, and
/// each shared capability as itself. `None` when a declaration rule refuses an
/// entry, so the body is not checked against a half-built scope.
fn commons_fn_capabilities(
    f: &FnDecl,
    selected: &HashMap<String, CapabilityInfo>,
    cross_context: &resolver::CrossContextInfo,
    resolved: &resolver::ResolvedCommons,
    errors: &mut Vec<CompileError>,
    refs: &mut RefSink,
) -> Option<HashMap<String, CapabilityInfo>> {
    let before = errors.len();
    let policy = &resolved.stability.policy;
    let type_params: HashSet<&str> = f.type_params.iter().map(|t| t.name.name.as_str()).collect();
    let mut params_seen: HashSet<&str> = HashSet::new();
    let mut capabilities = HashMap::new();
    for g in &f.given {
        let unit = cross_context.flattened_caps.get(g.key());
        match &g.bound {
            Some(bound) => {
                if let Some(err) = gate(policy, g.span) {
                    errors.push(err);
                }
                if let Some(err) = name_conflict(g, selected, &type_params, &params_seen) {
                    errors.push(err);
                }
                params_seen.insert(g.key());
                let Some(kind_unit) = cross_context.flattened_caps.get(&bound.name) else {
                    errors.push(kind_not_selected(&bound.name, bound.span));
                    continue;
                };
                crate::symbols::record_capability_clause_ref(bound, cross_context, refs);
                if crate::firstparty::instantiable(kind_unit, &bound.name).is_none() {
                    errors.push(shared_as_parameter(g, &bound.name));
                    continue;
                }
                let Some(contract) = selected.get(&bound.name) else {
                    continue;
                };
                capabilities.insert(
                    g.key().to_string(),
                    CapabilityInfo {
                        name: g.key().to_string(),
                        ops: contract.ops.clone(),
                        derived_from: Some((kind_unit.clone(), bound.name.clone())),
                    },
                );
            }
            None => {
                if let Some(unit) = unit
                    && g.context.is_none()
                    && crate::firstparty::instantiable(unit, g.key()).is_some()
                {
                    errors.push(kind_needs_parameter(g));
                    continue;
                }
                if unit.is_none() && g.context.is_none() {
                    errors.push(
                        CompileError::new(
                            "bynk.given.unknown_capability",
                            g.span,
                            format!("capability `{}` is not selected in this commons", g.key()),
                        )
                        .with_note(format!(
                            "a commons names a first-party capability by selecting it in a braced `consumes`, as in `consumes bynk {{ {} }}`",
                            g.key()
                        )),
                    );
                    continue;
                }
                if let Some(info) =
                    crate::symbols::resolve_given_cap_ref(g, selected, cross_context, errors, refs)
                {
                    capabilities.insert(g.key().to_string(), info);
                }
            }
        }
    }
    (errors.len() == before).then_some(capabilities)
}

/// `bynk.given.parameter_name_conflict`: a parameter that shadows a
/// capability or kind the commons can see, a type parameter, or another
/// parameter. Inside the body `Store.get` must mean one thing.
fn name_conflict(
    g: &CapRef,
    selected: &HashMap<String, CapabilityInfo>,
    type_params: &HashSet<&str>,
    params_seen: &HashSet<&str>,
) -> Option<CompileError> {
    let name = g.key();
    let what = if params_seen.contains(name) {
        "another capability parameter of this function"
    } else if type_params.contains(name) {
        "a type parameter of this function"
    } else if selected.contains_key(name) {
        "a capability this commons selects"
    } else {
        return None;
    };
    Some(
        CompileError::new(
            "bynk.given.parameter_name_conflict",
            g.name.span,
            format!("the capability parameter `{name}` has the name of {what}"),
        )
        .with_note("a parameter may not shadow a visible capability or kind, a type parameter or another parameter, so that `Name.op(…)` in the body means one thing: rename it"),
    )
}

/// A bound naming a kind the commons does not select.
fn kind_not_selected(kind: &str, span: Span) -> CompileError {
    CompileError::new(
        "bynk.given.unknown_capability",
        span,
        format!("the kind `{kind}` is not selected in this commons"),
    )
    .with_note(format!(
        "a commons names a first-party capability kind by selecting it: add `{kind}` to the braced `consumes` of the unit that exports it, as in `consumes bynk.cloudflare {{ {kind} }}`"
    ))
}

/// `bynk.given.shared_as_parameter`: a kind with no instances used as a bound.
fn shared_as_parameter(g: &CapRef, kind: &str) -> CompileError {
    CompileError::new(
        "bynk.given.shared_as_parameter",
        g.span,
        format!("`{kind}` has no instances, so it cannot bound a capability parameter"),
    )
    .with_note(format!(
        "a caller has exactly one `{kind}`, so a binding would add a name and no choice: name it bare as a shared capability, `given {kind}`"
    ))
    .with_suggestion(
        format!("make `{kind}` a shared capability"),
        vec![(g.span, kind.to_string())],
        Applicability::MachineApplicable,
    )
}

/// `bynk.given.kind_needs_parameter`: an instantiable kind named bare in a
/// commons. A library never silently means "the caller's default instance";
/// the fix makes it a parameter, which every caller then binds.
fn kind_needs_parameter(g: &CapRef) -> CompileError {
    let kind = g.key();
    CompileError::new(
        "bynk.given.kind_needs_parameter",
        g.span,
        format!("`{kind}` has instances, so a commons names it as a capability parameter"),
    )
    .with_note(format!(
        "declare a parameter bounded by the kind, `given Store: {kind}`, and each caller binds it to one of its own instances (or its bare `{kind}`) with `given Store = …`"
    ))
    .with_suggestion(
        format!("make `{kind}` a capability parameter"),
        vec![(g.span, format!("Store: {kind}"))],
        Applicability::MaybeIncorrect,
    )
}

/// `bynk.commons.capability_unused`: a capability this commons file selects
/// in its `consumes` and no function's `given` names, bare or as a bound. A
/// selection brings in a contract and nothing else, so an unused one only
/// misleads a reader about what the library reaches.
fn unused_selected_capabilities(
    commons: &bynk_syntax::ast::Commons,
    consumes: &[bynk_syntax::ast::ConsumesDecl],
    cross_context: &resolver::CrossContextInfo,
) -> Vec<CompileError> {
    let named: HashSet<&str> = commons
        .items
        .iter()
        .filter_map(|i| match i {
            CommonsItem::Fn(f) => Some(f),
            _ => None,
        })
        .flat_map(|f| &f.given)
        .map(|g| g.bound.as_ref().map_or(g.key(), |b| b.name.as_str()))
        .collect();
    consumes
        .iter()
        .flat_map(|c| c.selected.iter().flatten().map(move |item| (c, item)))
        .filter(|(_, item)| {
            cross_context.flattened_caps.contains_key(&item.name.name)
                && !named.contains(item.name.name.as_str())
        })
        .map(|(c, item)| {
            CompileError::new(
                "bynk.commons.capability_unused",
                item.name.span,
                format!(
                    "`{}` is selected from `{}`, but no function here names it in `given`",
                    item.name.name,
                    c.target.joined()
                ),
            )
            .with_note("remove it from the `consumes` list, or name it in a function's `given`, as a shared capability or the bound of a capability parameter")
        })
        .collect()
}
