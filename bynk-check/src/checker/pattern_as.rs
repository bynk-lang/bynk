//! #3971: `pattern as name` binds the whole value a pattern matched.
//!
//! The binding is at the type of the position the pattern sits at: the sum for
//! a variant pattern, the record for a record pattern, the element for a list
//! element. `as` never narrows what its pattern matches, so exhaustiveness and
//! reachability read straight through it (`Pattern::peel_as`).

use bynk_syntax::ast::Pattern;
use bynk_syntax::error::{Applicability, CompileError};

/// `bynk.match.as_on_wildcard` for `_ as x`, or `None` for any other pattern.
/// `_` matches everything, so `x` alone binds the same value; the fix writes it.
pub(crate) fn as_on_wildcard(pattern: &Pattern) -> Option<CompileError> {
    let Pattern::As { inner, name, span } = pattern else {
        return None;
    };
    if !matches!(**inner, Pattern::Wildcard(_)) {
        return None;
    }
    Some(
        CompileError::new(
            "bynk.match.as_on_wildcard",
            *span,
            format!("`_ as {0}` is just `{0}`", name.name),
        )
        .with_note("`_` already matches every value, so a bare name binds the same thing")
        .with_suggestion(
            format!("write `{}`", name.name),
            vec![(*span, name.name.clone())],
            Applicability::MachineApplicable,
        ),
    )
}
