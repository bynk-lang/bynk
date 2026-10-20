//! Short source-text renderings of AST fragments, shared by the agent state
//! model ([`crate::agent_state`]) and `bynk-ide`'s sequence diagram and effect
//! summary: a branch's outcome, a match arm's pattern, and a parenthesis peel.
//! #2220 moved them here from `bynk-ide`'s `sequence.rs` with the state model.

use bynk_syntax::ast::*;

/// A branch's rendered outcome — the tail value the handler yields on that
/// path (`Ok(view)`), which a renderer shows as a note over the entry
/// lifeline. `None` when the tail carries no distinguishable reply: a unit
/// `()` (an else-less `if`'s synthesised branch, or an explicit `()` tail), or
/// control flow (`if`/`match`/block) that is already rendered as its own
/// nested structure — repeating it as a note would only add noise.
pub fn branch_outcome(tail: &Expr) -> Option<String> {
    let inner = peel_paren(tail);
    match &inner.kind {
        ExprKind::UnitLit | ExprKind::If { .. } | ExprKind::Match { .. } | ExprKind::Block(_) => {
            None
        }
        _ => Some(bynk_fmt::expr_to_string(inner)),
    }
}

pub fn peel_paren(expr: &Expr) -> &Expr {
    match &expr.kind {
        ExprKind::Paren(inner) => peel_paren(inner),
        _ => expr,
    }
}

/// A match arm's branch label — a short rendering of its pattern, since
/// there is no dedicated pattern-to-source printer to reuse.
pub fn pattern_summary(pattern: &Pattern) -> String {
    match pattern {
        Pattern::Wildcard(_) => "_".to_string(),
        Pattern::Binding(b) => b.name.clone(),
        Pattern::Literal { value, .. } => value.describe(),
        Pattern::Variant {
            type_name, variant, ..
        } => match type_name {
            Some(t) => format!("{}.{}", t.name, variant.name),
            None => variant.name.clone(),
        },
        Pattern::Refined { inner, .. } => pattern_summary(inner),
        Pattern::Or(patterns, _) => patterns
            .iter()
            .map(pattern_summary)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}
