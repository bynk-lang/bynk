//! #4035: hover and completion for the name an `expect <call> violates <name>` claim names.
//!
//! The name is an invariant or a transition of an agent, or a `@unique` constraint by its field
//! and key. The declarations are read from the project's sources, so a claim in a test file
//! completes against the agents of every unit, and hover shows the predicate that refused.

use std::collections::HashMap;
use std::path::PathBuf;

use bynk_syntax::ast::{AgentDecl, CommonsItem, ExprKind, SourceUnit};
use bynk_syntax::lexer::tokenize;
use bynk_syntax::parser::parse_unit_with_recovery;

use crate::completion::{Completion, CompletionKind};

/// One declaration a `violates` claim can name.
struct Refusal {
    agent: String,
    /// The name a fault carries: `non_negative`, or `field.index` for a `@unique` constraint.
    name: String,
    /// What it is, as hover shows it.
    text: String,
}

/// The `violates` claims' declarations in one source: its agents' invariants, transitions and
/// `@unique` constraints, in declaration order.
fn refusals_of(source: &str) -> Vec<Refusal> {
    let Ok(tokens) = tokenize(source) else {
        return Vec::new();
    };
    let (Some(unit), _) = parse_unit_with_recovery(&tokens, source) else {
        return Vec::new();
    };
    let items: &[CommonsItem] = match &unit {
        SourceUnit::Commons(c) => &c.items,
        SourceUnit::Context(c) => &c.items,
        SourceUnit::Adapter(a) => &a.items,
        SourceUnit::Suite(_) => &[],
    };
    let mut out = Vec::new();
    for item in items {
        if let CommonsItem::Agent(a) = item {
            agent_refusals(a, &mut out);
        }
    }
    out
}

fn agent_refusals(a: &AgentDecl, out: &mut Vec<Refusal>) {
    let agent = a.name.name.clone();
    for i in &a.invariants {
        out.push(Refusal {
            agent: agent.clone(),
            name: i.name.name.clone(),
            text: format!(
                "invariant {}: {}",
                i.name.name,
                bynk_fmt::expr_to_string(&i.predicate)
            ),
        });
    }
    for t in &a.transitions {
        out.push(Refusal {
            agent: agent.clone(),
            name: t.name.name.clone(),
            text: format!(
                "transition {}: {}",
                t.name.name,
                bynk_fmt::expr_to_string(&t.predicate)
            ),
        });
    }
    for f in &a.store_fields {
        for an in f.annotations.iter().filter(|an| an.name.name == "unique") {
            for arg in &an.args {
                if let ExprKind::Ident(by) = &arg.value.kind {
                    out.push(Refusal {
                        agent: agent.clone(),
                        name: format!("{}.{}", f.name.name, by.name),
                        text: format!(
                            "store {}: … @unique(by: {}): no two entries share a `{}`",
                            f.name.name, by.name, by.name
                        ),
                    });
                }
            }
        }
    }
}

/// Every source in scope: the live document, then the project's files.
fn sources<'a>(doc_text: &'a str, files: Option<&'a HashMap<PathBuf, String>>) -> Vec<&'a str> {
    let mut all = vec![doc_text];
    if let Some(files) = files {
        all.extend(files.values().map(String::as_str));
    }
    all
}

/// The text typed after `violates ` when the cursor is in the claim's name: `""`, `Acc`,
/// `Account.`. `None` anywhere else.
fn typed_after_violates(line_prefix: &str) -> Option<&str> {
    let at = line_prefix.rfind("violates")?;
    if at > 0 && line_prefix[..at].ends_with(|c: char| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let rest = &line_prefix[at + "violates".len()..];
    let typed = rest.strip_prefix(char::is_whitespace)?.trim_start();
    typed
        .chars()
        .all(|c| c.is_alphanumeric() || c == '_' || c == '.')
        .then_some(typed)
}

/// Completions for the name after `violates`: the invariants, transitions and `@unique`
/// constraints the project declares, bare or qualified by their agent. `None` when the cursor is
/// not there.
pub fn name_candidates(
    line_prefix: &str,
    doc_text: &str,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<Vec<Completion>> {
    let typed = typed_after_violates(line_prefix)?;
    // The editor replaces the word at the cursor, so a label is what follows the last `.` typed.
    let dotted = typed.rfind('.').map_or(0, |d| d + 1);
    let (path, word) = typed.split_at(dotted);
    let mut out: Vec<Completion> = Vec::new();
    for source in sources(doc_text, files) {
        for r in refusals_of(source) {
            let qualified = format!("{}.{}", r.agent, r.name);
            for form in [r.name.as_str(), qualified.as_str()] {
                let Some(rest) = form.strip_prefix(path) else {
                    continue;
                };
                if !rest.starts_with(word) || rest.is_empty() {
                    continue;
                }
                if out.iter().any(|c| c.label == rest) {
                    continue;
                }
                out.push(Completion::item(
                    rest,
                    CompletionKind::Constant,
                    Some(format!("{} (agent `{}`)", r.text, r.agent)),
                ));
            }
        }
    }
    Some(out)
}

/// Hover for the name after `violates`: the declaration that refuses, with its predicate. `None`
/// when the cursor is not on such a name, or nothing declares it.
pub fn describe_at(
    text: &str,
    offset: usize,
    files: Option<&HashMap<PathBuf, String>>,
) -> Option<String> {
    let line_start = text[..offset.min(text.len())]
        .rfind('\n')
        .map_or(0, |n| n + 1);
    let line_end = text[offset.min(text.len())..]
        .find('\n')
        .map_or(text.len(), |n| offset + n);
    let line = &text[line_start..line_end];
    let at = line.rfind("violates")?;
    let name_start = line_start + at + "violates".len();
    let name = text[name_start..line_end].trim_start();
    let name_start = line_end - name.len();
    let name: String = name
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '_' || *c == '.')
        .collect();
    if name.is_empty() || offset < name_start || offset > name_start + name.len() {
        return None;
    }
    let mut found: Vec<String> = Vec::new();
    for source in sources(text, files) {
        for r in refusals_of(source) {
            if (r.name == name || format!("{}.{}", r.agent, r.name) == name)
                && !found.iter().any(|f| f.ends_with(&format!("`{}`", r.agent)))
            {
                found.push(format!(
                    "```bynk\n{}\n```\n\nRefuses a commit of agent `{}`.",
                    r.text, r.agent
                ));
            }
        }
    }
    (!found.is_empty()).then(|| found.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const AGENT: &str = "context demo.ledger\n\nagent Account {\n  key id: String\n  store balance: Cell[Int] = 0\n  store contacts: Map[String, Contact] @unique(by: email)\n  invariant non_negative: balance >= 0\n  transition grows: new.balance >= old.balance\n  on call noop() -> Effect[()] {\n    Effect.pure(())\n  }\n}\n";

    fn files() -> HashMap<PathBuf, String> {
        HashMap::from([(PathBuf::from("src/demo/ledger.bynk"), AGENT.to_string())])
    }

    fn labels(prefix: &str) -> Vec<String> {
        name_candidates(prefix, "", Some(&files()))
            .map(|c| c.into_iter().map(|c| c.label).collect())
            .unwrap_or_default()
    }

    #[test]
    fn completion_offers_invariants_transitions_and_unique_names() {
        let l = labels("    expect Account(\"a\").noop() violates ");
        for want in [
            "non_negative",
            "grows",
            "contacts.email",
            "Account.non_negative",
        ] {
            assert!(l.contains(&want.to_string()), "{want} in {l:?}");
        }
    }

    #[test]
    fn completion_narrows_by_the_typed_word_and_agent() {
        assert_eq!(labels("expect x violates non"), vec!["non_negative"]);
        assert!(labels("expect x violates Account.").contains(&"grows".to_string()));
        assert!(labels("expect x violates ").len() >= 4);
    }

    #[test]
    fn completion_is_silent_elsewhere() {
        assert!(name_candidates("expect x faults", "", Some(&files())).is_none());
        assert!(name_candidates("let violates", "", Some(&files())).is_none());
    }

    #[test]
    fn hover_shows_the_refusing_predicate() {
        let line = "    expect Account(\"a\").noop() violates non_negative\n";
        let at = line.find("non_negative").unwrap() + 3;
        let h = describe_at(line, at, Some(&files())).expect("hover");
        assert!(h.contains("invariant non_negative: balance >= 0"), "{h}");
        assert!(h.contains("Account"), "{h}");
        let off = line.find("expect").unwrap();
        assert!(describe_at(line, off, Some(&files())).is_none());
    }
}
