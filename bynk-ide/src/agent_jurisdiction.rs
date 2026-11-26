//! #3819: hover on an agent's `@jurisdiction("eu" | "fedramp")`: where its state lives, and what a
//! change costs.

use bynk_syntax::ast::{CommonsItem, SourceUnit};

/// Hover anywhere in an agent's `@jurisdiction( … )`: the annotation as the formatter writes it,
/// where every instance's state is kept, and why it cannot change after deploy. `None` when
/// `offset` is not inside one.
pub fn describe_jurisdiction_at(source: &str, offset: usize) -> Option<String> {
    let tokens = bynk_syntax::lexer::tokenize(source).ok()?;
    let unit = bynk_syntax::parser::parse_unit_with_recovery(&tokens, source).0?;
    let SourceUnit::Context(context) = &unit else {
        return None;
    };
    for item in &context.items {
        let CommonsItem::Agent(agent) = item else {
            continue;
        };
        for ann in &agent.annotations {
            if ann.name.name != bynk_check::agent_jurisdiction::JURISDICTION
                || !(ann.span.start <= offset && offset <= ann.span.end)
            {
                continue;
            }
            let applied = match bynk_check::agent_jurisdiction::jurisdiction_of(agent) {
                Some(j) => format!("every instance's state is kept in the **{j}** jurisdiction"),
                None => "this annotation is not valid, so it is not applied".to_string(),
            };
            return Some(describe(
                agent.name.name.as_str(),
                &bynk_fmt::annotation_to_string(ann),
                &applied,
            ));
        }
    }
    None
}

/// The hover text, shared with the agent's own hover.
pub(crate) fn describe(agent: &str, annotation: &str, applied: &str) -> String {
    format!(
        "```bynk\n{annotation}\n```\n\n\
         For `{agent}`: {applied}. The agent's Durable Object namespace is reached through \
         `namespace.jurisdiction(…)`, so every instance's id comes from the restricted \
         namespace.\n\n\
         It cannot change after deploy: the same name in another jurisdiction is a different id, \
         so a change would orphan every existing instance. `bynk deploy` records the \
         jurisdiction per class and refuses a change unless it is confirmed.\n\n\
         Locally, `bynk dev` (workerd) does not implement jurisdictions: it uses the unrestricted \
         namespace there and warns once."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "context shop.ledger\n\n@jurisdiction(\"eu\")\nagent Learner {\n  key id: String\n  store n: Cell[Int]\n\n  on call add() -> Effect[Int] {\n    n\n  }\n}\n";

    #[test]
    fn hover_describes_the_jurisdiction() {
        let at = SRC.find("jurisdiction").unwrap() + 2;
        let hover = describe_jurisdiction_at(SRC, at).expect("hover");
        assert!(hover.contains("@jurisdiction(\"eu\")"), "{hover}");
        assert!(hover.contains("**eu** jurisdiction"), "{hover}");
        assert!(describe_jurisdiction_at(SRC, SRC.find("agent").unwrap() + 2).is_none());
        assert!(describe_jurisdiction_at(SRC, 0).is_none());
    }

    #[test]
    fn an_invalid_jurisdiction_says_it_is_not_applied() {
        let src = SRC.replace("\"eu\"", "\"us\"");
        let at = src.find("jurisdiction").unwrap() + 2;
        let hover = describe_jurisdiction_at(&src, at).expect("hover");
        assert!(hover.contains("not valid"), "{hover}");
    }
}
