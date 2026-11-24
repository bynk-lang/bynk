//! #3894: tooling for `<context>.<service>.outcome(args)`, the cross-context call whose failure
//! is a `CallOutcome` value.
//!
//! Completion offers `outcome` after a consumed context's service; signature help shows the
//! service's parameters under the `Effect[CallOutcome[R]]` it yields; hover says what each
//! variant means. All three read the source, as the other editor surfaces do: the consumed
//! context comes from this file's `consumes` clauses and its service from the project's files.

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_syntax::ast::{CommonsItem, HandlerKind, SourceUnit};
use bynk_syntax::span::Span;

use crate::completion::{Completion, CompletionKind, for_each_unit};
use crate::symbols::type_ref_str;

/// The text of `outcome`'s detail: what it yields.
const OUTCOME_DETAIL: &str = "outcome(…) -> Effect[CallOutcome[R]] — the call's failure as a value";

/// The consumed context `prefix` names in `doc_text`, by alias or qualified name.
fn consumed_context(prefix: &str, doc_text: &str) -> Option<String> {
    let tokens = bynk_syntax::lexer::tokenize(doc_text).ok()?;
    let unit = bynk_syntax::parser::parse_unit_with_recovery(&tokens, doc_text).0?;
    let consumes = match &unit {
        SourceUnit::Context(c) => &c.consumes,
        SourceUnit::Suite(s) => &s.consumes,
        _ => return None,
    };
    consumes.iter().find_map(|c| {
        let target = c.target.joined();
        let alias = c.alias.as_ref().map(|a| a.name.as_str());
        (alias == Some(prefix) || (c.alias.is_none() && c.selected.is_none() && target == prefix))
            .then_some(target)
    })
}

/// `outcome` as a member candidate, when `line_prefix` ends `<context>.<service>.` (or the
/// start of a member name after it) for a context this file consumes.
pub fn outcome_member_candidates(line_prefix: &str, doc_text: &str) -> Option<Vec<Completion>> {
    let (prefix, _service) = service_chain_before_dot(line_prefix)?;
    consumed_context(&prefix, doc_text)?;
    Some(vec![Completion::item(
        "outcome",
        CompletionKind::Member,
        Some(OUTCOME_DETAIL.to_string()),
    )])
}

/// The `(context prefix, service)` of the dotted chain ending `….<service>.<partial>` at the
/// end of `line`.
fn service_chain_before_dot(line: &str) -> Option<(String, String)> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let head = line.trim_end_matches(is_ident).strip_suffix('.')?;
    let chain_start = head
        .char_indices()
        .rfind(|&(_, c)| !is_ident(c) && c != '.')
        .map_or(0, |(i, c)| i + c.len_utf8());
    let chain = &head[chain_start..];
    let (prefix, service) = chain.rsplit_once('.')?;
    (!prefix.is_empty() && !service.is_empty() && prefix.split('.').all(|s| !s.is_empty()))
        .then(|| (prefix.to_string(), service.to_string()))
}

/// The signature of `<context>.<service>.outcome(…)`: the service's parameters, and its handler's
/// result under `CallOutcome`. `recv` is the dotted `<context>.<service>` before `.outcome`.
pub fn outcome_signature(
    recv: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let (prefix, service) = recv.rsplit_once('.')?;
    let context = consumed_context(prefix, doc_text)?;
    let mut label = None;
    for_each_unit(doc_text, files, |unit| {
        let SourceUnit::Context(c) = unit else { return };
        if label.is_some() || c.name.joined() != context {
            return;
        }
        for item in &c.items {
            let CommonsItem::Service(s) = item else {
                continue;
            };
            if s.name.name != service {
                continue;
            }
            let Some(h) = s
                .handlers
                .iter()
                .find(|h| matches!(h.kind, HandlerKind::Call))
            else {
                continue;
            };
            let params: Vec<String> = h
                .params
                .iter()
                .map(|p| format!("{}: {}", p.name.name, type_ref_str(&p.type_ref)))
                .collect();
            let result = match &h.return_type {
                bynk_syntax::ast::TypeRef::Effect(inner, _) => type_ref_str(inner),
                other => type_ref_str(other),
            };
            label = Some(format!(
                "outcome({}) -> Effect[CallOutcome[{result}]]",
                params.join(", ")
            ));
        }
    });
    label
}

/// Hover on a built-in member of an effect: the `within`, `race` or `detach` of an `Effect`, or
/// the `outcome` of a service call.
pub fn describe_member_at(
    text: &str,
    offset: usize,
    expr_types: &[(Span, bynk_check::checker::TyId)],
    tys: &bynk_check::checker::Types,
) -> Option<String> {
    crate::symbols::describe_effect_method_at(text, offset, expr_types, tys)
        .or_else(|| describe_outcome_at(text, offset, expr_types, tys))
}

/// Hover on the `outcome` of `<context>.<service>.outcome(args)` or `Agent(key).op.outcome(args)`:
/// the call's type and what each variant means. The expression it is in has type
/// `Effect[CallOutcome[…]]`, which keeps a service named `outcome`, or a user type's own method,
/// from being described as it.
pub fn describe_outcome_at(
    text: &str,
    offset: usize,
    expr_types: &[(Span, bynk_check::checker::TyId)],
    tys: &bynk_check::checker::Types,
) -> Option<String> {
    let tokens = bynk_syntax::lexer::tokenize(text).ok()?;
    let tok = tokens.iter().find(|t| {
        t.span.start <= offset
            && offset < t.span.end
            && &text[t.span.start..t.span.end] == "outcome"
    })?;
    let before = text[..tok.span.start].trim_end();
    let after = text[tok.span.end..].trim_start();
    if !before.ends_with('.') || !after.starts_with('(') {
        return None;
    }
    let ty = bynk_check::expr_types::type_at_offset(expr_types, offset, tys)?;
    let display = ty.display(tys);
    let handled = display
        .strip_prefix("Effect[CallOutcome[")?
        .strip_suffix("]]")?;
    Some(format!(
        "```bynk\noutcome(…) -> Effect[CallOutcome[{handled}]]\n```\n\n\
         The same call as without `.outcome`, but a call that fails is a value, not a fault in \
         this handler. `CallOutcome` says whether the callee's handler ran:\n\n\
         - `Handled(result)`: it ran and answered (a `Result`'s `Err` is an answer).\n\
         - `Rejected(reason)`: it was refused before it ran (`TooLarge`, `NotAuthenticated`, \
         `UnsupportedWire`, `NoSuchService`, `NotAdmitted`, `StaleContract`, `Undecodable`). \
         Never for an agent call.\n\
         - `Faulted`: it ran and faulted; a faulting handler commits nothing. For an agent \
         call, an invariant or rehydration violation is one too.\n\
         - `Unreachable(status)`: it is not known whether it ran: a failed fetch, an \
         unexpected status, or (an agent call) a queue that stayed busy. Retry with an \
         idempotency key.\n\n\
         `o.handled()` is the `Result[R, CallFailure]` for `?` and `embeds`. A failure turned \
         into a value is still logged once."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORDERS: &str = "context shop.orders\n\nconsumes shop.pay as Pay\nconsumes shop.stock\n";

    fn pay_files() -> HashMap<PathBuf, String> {
        HashMap::from([(
            PathBuf::from("/proj/src/shop/pay.bynk"),
            "context shop.pay\n\nservice authorise {\n  on call(total: Int, who: String) -> Effect[Result[String, String]] by Caller {\n    Effect.pure(Ok(who))\n  }\n}\n".to_string(),
        )])
    }

    #[test]
    fn outcome_is_offered_after_a_consumed_service_by_alias_or_qualified_name() {
        for prefix in [
            "  let o <- Pay.authorise.",
            "  let o <- Pay.authorise.out",
            "  let o <- shop.stock.reserve.",
        ] {
            let items = outcome_member_candidates(prefix, ORDERS).expect(prefix);
            assert_eq!(items.len(), 1);
            assert_eq!(items[0].label, "outcome");
        }
    }

    #[test]
    fn outcome_is_not_offered_elsewhere() {
        // An unconsumed prefix, a bare context prefix, and a local value's field.
        for prefix in [
            "  let o <- Other.authorise.",
            "  let o <- Pay.",
            "  let o <- order.total.",
            "  authorise.",
        ] {
            assert!(
                outcome_member_candidates(prefix, ORDERS).is_none(),
                "{prefix}"
            );
        }
    }

    #[test]
    fn the_signature_shows_the_services_parameters_and_the_wrapped_result() {
        let files = pay_files();
        let label = outcome_signature("Pay.authorise", ORDERS, Some(&files)).expect("signature");
        assert_eq!(
            label,
            "outcome(total: Int, who: String) -> Effect[CallOutcome[Result[String, String]]]"
        );
        let qualified = "context shop.orders\n\nconsumes shop.pay\n";
        assert_eq!(
            outcome_signature("shop.pay.authorise", qualified, Some(&files)).as_deref(),
            Some(label.as_str())
        );
        assert!(outcome_signature("Pay.missing", ORDERS, Some(&files)).is_none());
        assert!(outcome_signature("Other.authorise", ORDERS, Some(&files)).is_none());
    }

    #[test]
    fn hover_describes_the_variants_only_on_a_call_outcome() {
        use bynk_check::checker::{NamedKind, Ty, Types};
        use bynk_syntax::ast::BaseType;
        let tys = Types::new();
        let int = tys.intern(Ty::Base(BaseType::Int));
        let outcome = tys.intern(Ty::Named {
            name: "CallOutcome".to_string(),
            kind: NamedKind::Sum,
            args: vec![int],
        });
        let effect = tys.intern(Ty::Effect(outcome));
        let src = "let o <- Pay.authorise.outcome(1)\n";
        let at = src.find("outcome").unwrap() + 1;
        let call = [(Span::new(8, src.len() - 1), effect)];
        let hover = describe_outcome_at(src, at, &call, &tys).expect("outcome hovers");
        assert!(
            hover.contains("outcome(…) -> Effect[CallOutcome[Int]]"),
            "{hover}"
        );
        for variant in ["Handled", "Rejected", "Faulted", "Unreachable"] {
            assert!(hover.contains(variant), "{hover}");
        }
        // A service of that name, whose call is not a `CallOutcome`.
        let plain = [(Span::new(8, src.len() - 1), tys.intern(Ty::Effect(int)))];
        assert!(describe_outcome_at(src, at, &plain, &tys).is_none());
        assert!(describe_outcome_at(src, 0, &call, &tys).is_none());
    }
}
