//! #3932: hover on an agent's `@busyAfter(d)`: the wait it sets, and what a call does at the end
//! of it.

use bynk_syntax::ast::{CommonsItem, SourceUnit};

/// Hover anywhere in an agent's `@busyAfter( … )`: the annotation as the formatter writes it,
/// the wait it sets for a call queued behind the instance's running handler, and what happens
/// when the wait ends. `None` when `offset` is not inside one.
pub fn describe_busy_after_at(source: &str, offset: usize) -> Option<String> {
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
            if ann.name.name != "busyAfter" || !(ann.span.start <= offset && offset <= ann.span.end)
            {
                continue;
            }
            let wait = match bynk_check::agent_busy::busy_after_ms(agent) {
                Some(ms) => format!("**{} s** here", ms as f64 / 1000.0),
                None => "30 s by default (this annotation is not valid, so it is not applied)"
                    .to_string(),
            };
            return Some(format!(
                "```bynk\n{}\n```\n\n\
                 How long a call to `{}` waits behind the handlers ahead of it on one instance: \
                 {wait}. An instance runs one handler at a time, so a hot one queues. The timer \
                 measures queue wait only, never a handler's own run time.\n\n\
                 When it ends the call fails as `AgentBusy`: an HTTP route answers `503` with \
                 `Retry-After: 1` (logged as busy, not reported as a fault), a cross-context call \
                 is `Unreachable`, and in a queue consumer it is a fault that retries the message. \
                 It can lower the 30-second default (from 1 s), never raise it.",
                bynk_fmt::annotation_to_string(ann),
                agent.name.name
            ));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: &str = "context shop.basket\n\n@busyAfter(5.seconds)\nagent Basket {\n  key id: String\n  store n: Cell[Int]\n\n  on call add() -> Effect[Int] {\n    n\n  }\n}\n";

    #[test]
    fn hover_names_the_effective_wait_inside_the_annotation_only() {
        let at = SRC.find("busyAfter").unwrap() + 2;
        let hover = describe_busy_after_at(SRC, at).expect("hover");
        assert!(hover.contains("@busyAfter(5.seconds)"), "{hover}");
        assert!(hover.contains("**5 s** here"), "{hover}");
        assert!(hover.contains("`Basket`"), "{hover}");
        assert!(hover.contains("503"), "{hover}");
        assert!(describe_busy_after_at(SRC, SRC.find("agent").unwrap() + 2).is_none());
        assert!(describe_busy_after_at(SRC, 0).is_none());
    }

    #[test]
    fn an_invalid_wait_says_it_is_not_applied() {
        let src = SRC.replace("5.seconds", "60.seconds");
        let at = src.find("busyAfter").unwrap() + 2;
        let hover = describe_busy_after_at(&src, at).expect("hover");
        assert!(hover.contains("30 s by default"), "{hover}");
    }
}
