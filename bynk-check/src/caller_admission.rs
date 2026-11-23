//! Which contexts may call a service, decided across the whole project (#3890).
//!
//! A refinement over `Caller` (`actor Admin = Caller where IsContext(shop.admin)`)
//! names the calling contexts a service admits. The compiler sees every call
//! edge in the project, so a call the callee's actor does not admit is refused
//! here, at the call site, before deploy. The callee still checks the proven
//! caller at run time (wire format check 6), because signing keys are per
//! context pair and a stale caller may still be deployed.
//!
//! Two reports: `bynk.actor.unknown_context` (a name in `IsContext(…)` that is
//! no context of the project) and `bynk.actor.caller_not_admitted` (a call from
//! a context no member of the callee's `by` clause admits).

use std::collections::{BTreeMap, HashMap};

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{
    ActorDecl, BinOp, CommonsItem, Expr, ExprKind, Handler, HandlerKind, SourceUnit, UnaryOp,
    expr_children, statement_exprs,
};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::actors::{caller_refinement, dotted_name};
use crate::checker::Callee;
use crate::project_model::{ErrorSink, UnitCallees};
use crate::symbols::UnitTable;

/// The context names an `IsContext(…)` predicate mentions, with where each is written.
fn mentioned_names(e: &Expr, out: &mut Vec<(String, Span)>) {
    match &e.kind {
        ExprKind::Paren(inner) | ExprKind::UnaryOp(UnaryOp::Not, inner) => {
            mentioned_names(inner, out)
        }
        ExprKind::BinOp(BinOp::And | BinOp::Or, l, r) => {
            mentioned_names(l, out);
            mentioned_names(r, out);
        }
        ExprKind::Call { name, args, .. } if name.name == "IsContext" => {
            out.extend(
                args.iter()
                    .filter_map(|a| dotted_name(a).map(|n| (n, a.span))),
            );
        }
        _ => {}
    }
}

/// Whether the actor named `name` in `table` admits a call from `from`.
/// A foreign actor never admits a context; a refinement over `Caller` admits
/// the contexts its predicate does; any other actor (`Caller` itself, or a
/// name the checker reports) admits every context.
fn actor_admits(name: &str, table: &UnitTable, from: &str) -> bool {
    match table.actors.get(name) {
        Some(decl) => match caller_refinement(decl, &table.actors) {
            Some(pred) => pred.admits(from),
            None => {
                decl.auth
                    .as_ref()
                    .and_then(|a| crate::actors::Scheme::from_name(&a.name))
                    != Some(crate::actors::Scheme::Foreign)
            }
        },
        None => true,
    }
}

/// The refinement predicates a `by` clause names, for the "admitted contexts" note.
fn admitted_note(
    by_names: &[&str],
    table: &UnitTable,
    contexts: &[&str],
    caller: &str,
) -> (String, String) {
    let admitted: Vec<&str> = contexts
        .iter()
        .copied()
        .filter(|c| by_names.iter().any(|n| actor_admits(n, table, c)))
        .collect();
    let actor = by_names.join(" | ");
    let list = if admitted.is_empty() {
        "no context".to_string()
    } else {
        admitted
            .iter()
            .map(|c| format!("`{c}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let _ = caller;
    (actor, list)
}

/// Refuse a name that is no context, and a call no `by` member admits.
pub fn phase_caller_admission(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    unit_tables: &HashMap<String, UnitTable>,
    unit_callees: &UnitCallees,
    errors: &mut ErrorSink,
) {
    let contexts: Vec<&str> = groups
        .keys()
        .filter(|n| kinds.get(*n) == Some(&UnitKind::Context))
        .map(String::as_str)
        .collect();
    for (ctx_name, indices) in groups {
        if kinds.get(ctx_name) != Some(&UnitKind::Context) {
            continue;
        }
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            let file = parsed[i].identity_path();
            for item in &ctx.items {
                match item {
                    CommonsItem::Actor(a) => check_actor_names(a, &contexts, &file, errors),
                    CommonsItem::Service(s) => {
                        for h in &s.handlers {
                            check_handler_calls(
                                h,
                                ctx_name,
                                &contexts,
                                unit_tables,
                                unit_callees,
                                &file,
                                errors,
                            );
                        }
                    }
                    CommonsItem::Agent(a) => {
                        for h in &a.handlers {
                            check_handler_calls(
                                h,
                                ctx_name,
                                &contexts,
                                unit_tables,
                                unit_callees,
                                &file,
                                errors,
                            );
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

fn check_actor_names(
    actor: &ActorDecl,
    contexts: &[&str],
    file: &std::path::Path,
    errors: &mut ErrorSink,
) {
    let Some(r) = &actor.refinement else {
        return;
    };
    let mut names = Vec::new();
    mentioned_names(&r.predicate, &mut names);
    for (name, span) in names {
        if contexts.contains(&name.as_str()) {
            continue;
        }
        let mut err = CompileError::new(
            "bynk.actor.unknown_context",
            span,
            format!("`{name}` is not a context of this project"),
        )
        .with_note("`IsContext(…)` names the project's own contexts, written as qualified names");
        if let Some(near) = bynk_project::paths::did_you_mean(&name, contexts.iter().copied()) {
            err = err.with_note(format!("did you mean `{near}`?"));
        }
        errors.push_for(Some(file), err);
    }
}

fn check_handler_calls(
    handler: &Handler,
    caller: &str,
    contexts: &[&str],
    unit_tables: &HashMap<String, UnitTable>,
    unit_callees: &UnitCallees,
    file: &std::path::Path,
    errors: &mut ErrorSink,
) {
    let Some(callees) = unit_callees.get(caller) else {
        return;
    };
    let mut stack: Vec<&Expr> = Vec::new();
    for s in &handler.body.statements {
        statement_exprs(s, &mut stack);
    }
    stack.push(&handler.body.tail);
    while let Some(e) = stack.pop() {
        if let Some(Callee::Cross { unit, service }) = callees.get(&e.id)
            && let Some(table) = unit_tables.get(unit)
            && let Some(svc) = table.services.get(service)
            && let Some(h) = svc.handlers.iter().find(|h| h.kind == HandlerKind::Call)
            && let Some(by) = h.by_clause.as_ref().or(svc.default_by.as_ref())
        {
            let names: Vec<&str> = by.actors.iter().map(|a| a.name.as_str()).collect();
            if !names.iter().any(|n| actor_admits(n, table, caller)) {
                let (actor, admitted) = admitted_note(&names, table, contexts, caller);
                errors.push_for(
                    Some(file),
                    CompileError::new(
                        "bynk.actor.caller_not_admitted",
                        e.span,
                        format!("`{unit}.{service}` does not admit calls from `{caller}`"),
                    )
                    .with_note(format!(
                        "its handler is guarded by `by {actor}`, which admits {admitted}"
                    )),
                );
            }
        }
        stack.extend(expr_children(e));
    }
}

#[cfg(test)]
mod tests {
    use crate::testkit::analyse;

    const BILLING: &str = "context shop.billing\n\n\
actor Admin = Caller where IsContext(shop.admin)\n\
actor NotLegacy = Caller where !IsContext(shop.legacy)\n\n\
service refunds {\n  on call(id: String) -> Effect[Int] by Admin {\n    Effect.pure(1)\n  }\n}\n\n\
service status {\n  on call(id: String) -> Effect[Int] by NotLegacy {\n    Effect.pure(2)\n  }\n}\n";

    fn caller(name: &str, call: &str) -> (String, String) {
        (
            format!("shop/{name}.bynk"),
            format!(
                "context shop.{name}\n\nconsumes shop.billing\n\n\
service go {{\n  on call(id: String) -> Effect[Int] by Caller {{\n    {call}\n  }}\n}}\n"
            ),
        )
    }

    fn project(callers: &[(&str, &str)]) -> crate::testkit::Analysed {
        let mut files: Vec<(String, String)> =
            vec![("shop/billing.bynk".to_string(), BILLING.to_string())];
        for name in ["admin", "legacy", "orders"] {
            let call = callers
                .iter()
                .find(|(n, _)| *n == name)
                .map_or("Effect.pure(0)", |(_, c)| *c);
            files.push(caller(name, call));
        }
        let refs: Vec<(&str, &str)> = files
            .iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect();
        analyse(&refs)
    }

    #[test]
    fn an_admitted_context_calls_clean() {
        project(&[("admin", "shop.billing.refunds(id)")]).assert_clean();
    }

    #[test]
    fn a_context_the_predicate_does_not_name_is_refused_at_the_call() {
        let a = project(&[("orders", "shop.billing.refunds(id)")]);
        assert_eq!(a.error_categories(), ["bynk.actor.caller_not_admitted"]);
    }

    #[test]
    fn a_deny_list_refuses_only_the_named_context() {
        project(&[("orders", "shop.billing.status(id)")]).assert_clean();
        let a = project(&[("legacy", "shop.billing.status(id)")]);
        assert_eq!(a.error_categories(), ["bynk.actor.caller_not_admitted"]);
    }

    #[test]
    fn a_name_that_is_no_context_is_refused() {
        // Neither `shop.admin` nor `shop.legacy` exists here, so each is refused.
        let a = analyse(&[("shop/billing.bynk", BILLING)]);
        assert_eq!(
            a.error_categories(),
            ["bynk.actor.unknown_context", "bynk.actor.unknown_context"]
        );
    }
}
