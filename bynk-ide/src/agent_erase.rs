//! #3931: tooling for `self.erase()` in an agent handler.
//!
//! Completion offers `erase` after `self.`, and hover says what the call does: it deletes the
//! instance's stored state when the handler commits. Both read the source, as the other editor
//! surfaces do, so the completion cannot tell a handler of an agent from a method: it is offered
//! where the document declares an agent, and the checker refuses it anywhere `self` is not the
//! agent's.

use bynk_syntax::span::Span;

use crate::completion::{Completion, CompletionKind};

/// The text of `erase`'s detail: what it yields.
const ERASE_DETAIL: &str = "erase() -> Effect[()] — deletes this instance's stored state at commit";

/// What `self.erase()` does, as hover words it.
const ERASE_DOC: &str = "```bynk\nerase() -> Effect[()]\n```\n\n\
Deletes everything stored for this instance when the handler commits: every field, every `Map`, \
`Set`, `Cache` and `Log` entry, every index posting and the schema markers. The next call to \
the key starts from the agent's initial state, as a key never seen before does.\n\n\
**The erasure is the commit.** The handler's other writes, before or after the call, are \
discarded, and no invariant is checked against the initial state. A handler that faults after \
calling it erases nothing. Events emitted in the handler are still released.\n\n\
Only in an agent `on call` handler. Call it with `do self.erase()`, or as the handler's last \
expression.";

/// `erase` as a member candidate, when `line_prefix` ends `self.` (or the start of a member name
/// after it) in a document that declares an agent.
pub fn erase_member_candidates(line_prefix: &str, doc_text: &str) -> Option<Vec<Completion>> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let head = line_prefix.trim_end_matches(is_ident).strip_suffix('.')?;
    let before = head.strip_suffix("self")?;
    if before.ends_with(is_ident) || !doc_text.contains("agent ") {
        return None;
    }
    Some(vec![Completion::item(
        "erase",
        CompletionKind::Member,
        Some(ERASE_DETAIL.to_string()),
    )])
}

/// Hover on the `erase` of `self.erase()`: what the call deletes and when. `None` when the cursor
/// is not on such a word, or the `self` before it is not a typed agent `self` (the checker gives
/// an agent's `self` a record type named `__<Agent>Self`).
pub fn describe_erase_at(
    text: &str,
    offset: usize,
    expr_types: &[(Span, bynk_check::checker::TyId)],
    tys: &bynk_check::checker::Types,
) -> Option<String> {
    let tokens = bynk_syntax::lexer::tokenize(text).ok()?;
    let tok = tokens.iter().find(|t| {
        t.span.start <= offset && offset < t.span.end && &text[t.span.start..t.span.end] == "erase"
    })?;
    let before = text[..tok.span.start].trim_end();
    let after = text[tok.span.end..].trim_start();
    if !before.ends_with('.') || !after.starts_with('(') {
        return None;
    }
    let dot = before.len() - 1;
    let on_self = text[..dot].trim_end().ends_with("self");
    if !on_self {
        return None;
    }
    let recv = bynk_check::expr_types::type_at_offset(expr_types, dot.checked_sub(1)?, tys)?;
    let display = recv.display(tys);
    display
        .strip_prefix("__")
        .and_then(|s| s.strip_suffix("Self"))
        .map(|_| ERASE_DOC.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erase_is_offered_after_self_dot_in_a_document_with_an_agent() {
        let doc = "agent Account { key id: String }";
        assert!(erase_member_candidates("    self.", doc).is_some());
        assert!(erase_member_candidates("    self.er", doc).is_some());
        assert!(erase_member_candidates("    do self.", doc).is_some());
    }

    #[test]
    fn erase_is_not_offered_elsewhere() {
        let doc = "agent Account { key id: String }";
        assert!(erase_member_candidates("    myself.", doc).is_none());
        assert!(erase_member_candidates("    other.", doc).is_none());
        assert!(erase_member_candidates("    self", doc).is_none());
        assert!(erase_member_candidates("    self.", "fn f(self) -> Int { 1 }").is_none());
    }

    #[test]
    fn hover_describes_erase_on_an_agents_self_only() {
        use bynk_check::checker::{NamedKind, Ty, Types};
        let text = "do self.erase()";
        let tys = &Types::new();
        let at_self = Span::new(3, 7);
        let agent = vec![(
            at_self,
            tys.intern(Ty::Named {
                name: "__AccountSelf".into(),
                kind: NamedKind::Record,
                args: Vec::new(),
            }),
        )];
        let on_erase = text.find("erase").unwrap() + 1;
        let hover = describe_erase_at(text, on_erase, &agent, tys).expect("erase hovers");
        assert!(hover.contains("erase() -> Effect[()]"));
        assert!(hover.contains("initial state"));
        // A method's `self` is no agent's: nothing is said.
        let method = vec![(
            at_self,
            tys.intern(Ty::Named {
                name: "Account".into(),
                kind: NamedKind::Record,
                args: Vec::new(),
            }),
        )];
        assert!(describe_erase_at(text, on_erase, &method, tys).is_none());
        // Not on the word, nor on a call of something else called `erase`.
        assert!(describe_erase_at(text, 0, &agent, tys).is_none());
        assert!(describe_erase_at("do other.erase()", 10, &agent, tys).is_none());
    }
}
