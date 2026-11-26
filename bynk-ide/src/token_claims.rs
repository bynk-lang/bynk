//! #3937: tooling for `<binder>.claims` on a verified actor's binder (`by u: User`).
//!
//! The binder of an `Oidc` or `Bearer` actor reads the verified token's claims as a `TokenClaims`.
//! Completion offers `claims` after such a binder, and hover on `claims` lists its two methods.
//! Both read the source, as the other editor surfaces do: completion cannot tell a `Bearer` actor's
//! binder from any other `by` binder, so it is offered after any single-actor binder and the
//! checker refuses it where the actor reads no token.

use bynk_syntax::span::Span;

use crate::completion::{Completion, CompletionKind};

/// The text of `claims`' detail: what it yields.
const CLAIMS_DETAIL: &str = "claims: TokenClaims — the verified token's claims";

/// What `<binder>.claims` is, as hover words it.
const CLAIMS_DOC: &str = "```bynk\nclaims: TokenClaims\n```\n\n\
The verified token's claims, read by the actor's seam before the handler runs: a token that fails \
verification is `401` and never reaches it. Available on the binder of an `Oidc` or a `Bearer` \
actor, and of a refinement of one.\n\n\
| Method | Returns | Reads |\n|---|---|---|\n\
| `get(name)` | `Option[String]` | the claim when it is a JSON string; `None` for any other type, or none |\n\
| `has(name)` | `Bool` | whether the claim is present, of any type |\n\n\
A claim says what the token asserts, not whose data the handler may touch: object-level checks stay \
in the handler. A `TokenClaims` cannot be a field, or cross a call, a message or an agent \
(`bynk.types.claims_at_boundary`): read the claim you need and keep that `String`.";

/// The receiver identifier of a member being completed: `u` for `…u.` or `…u.cl`.
fn receiver_ident(line_prefix: &str) -> Option<&str> {
    let is_ident = |c: char| c.is_alphanumeric() || c == '_';
    let head = line_prefix.trim_end_matches(is_ident).strip_suffix('.')?;
    let start = head
        .char_indices()
        .rfind(|&(_, c)| !is_ident(c))
        .map_or(0, |(i, c)| i + c.len_utf8());
    let name = &head[start..];
    (!name.is_empty() && name.starts_with(|c: char| c.is_ascii_lowercase())).then_some(name)
}

/// Whether `doc_text` binds `name` in a single-actor `by` clause: `by name: Actor`, not a sum
/// (`by name: A | B`), whose binder is the resolved arm.
fn binds_single_actor(doc_text: &str, name: &str) -> bool {
    let needle = format!("by {name}:");
    doc_text.match_indices(&needle).any(|(i, _)| {
        let rest = &doc_text[i + needle.len()..];
        let clause = rest.split(['{', '\n']).next().unwrap_or("");
        let clause = clause.split("given").next().unwrap_or(clause);
        !clause.contains('|') && !clause.trim().is_empty()
    })
}

/// `claims` as a member candidate, when `line_prefix` ends `<binder>.` (or the start of a member
/// name after it) for a binder this document's `by` clauses declare.
pub fn claims_member_candidates(line_prefix: &str, doc_text: &str) -> Option<Vec<Completion>> {
    let name = receiver_ident(line_prefix)?;
    binds_single_actor(doc_text, name).then(|| {
        vec![Completion::item(
            "claims",
            CompletionKind::Member,
            Some(CLAIMS_DETAIL.to_string()),
        )]
    })
}

/// Hover on the `claims` of `<binder>.claims`: what it is and its two methods. `None` when the
/// cursor is not on such a word, or the receiver is not a verified actor's binder.
pub fn describe_claims_at(
    text: &str,
    offset: usize,
    expr_types: &[(Span, bynk_check::checker::TyId)],
    tys: &bynk_check::checker::Types,
) -> Option<String> {
    let tokens = bynk_syntax::lexer::tokenize(text).ok()?;
    let tok = tokens.iter().find(|t| {
        t.span.start <= offset && offset < t.span.end && &text[t.span.start..t.span.end] == "claims"
    })?;
    let before = text[..tok.span.start].trim_end();
    if !before.ends_with('.') {
        return None;
    }
    let dot = before.len() - 1;
    let recv = bynk_check::expr_types::type_at_offset(expr_types, dot.checked_sub(1)?, tys)?;
    matches!(&*tys.get(recv), bynk_check::checker::Ty::Actor(_)).then(|| CLAIMS_DOC.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "service api from http {\n  on GET(\"/x\") () -> Effect[HttpResult[String]] by u: User given Clock {\n    Ok(\"\")\n  }\n  on GET(\"/y\") () -> Effect[HttpResult[String]] by who: User | Visitor {\n    Ok(\"\")\n  }\n}";

    #[test]
    fn claims_is_offered_after_a_single_actor_binder() {
        assert!(claims_member_candidates("    let t = u.", DOC).is_some());
        assert!(claims_member_candidates("    let t = u.cl", DOC).is_some());
    }

    #[test]
    fn claims_is_not_offered_elsewhere() {
        // A sum's binder is the resolved arm; an unbound name; a type.
        assert!(claims_member_candidates("    who.", DOC).is_none());
        assert!(claims_member_candidates("    other.", DOC).is_none());
        assert!(claims_member_candidates("    User.", DOC).is_none());
        assert!(claims_member_candidates("    u", DOC).is_none());
    }

    #[test]
    fn hover_describes_claims_on_an_actor_binder_only() {
        use bynk_check::checker::{NamedKind, Ty, Types};
        let text = "let t = u.claims";
        let tys = &Types::new();
        let unit = tys.intern(Ty::Unit);
        let on_actor = vec![(Span::new(8, 9), tys.intern(Ty::Actor(unit)))];
        let at = text.find("claims").unwrap() + 1;
        let hover = describe_claims_at(text, at, &on_actor, tys).expect("claims hovers");
        assert!(hover.contains("claims: TokenClaims"));
        assert!(hover.contains("`get(name)`") && hover.contains("`has(name)`"));
        // A record that has a field named `claims` is no actor binder.
        let on_record = vec![(
            Span::new(8, 9),
            tys.intern(Ty::Named {
                name: "Seen".into(),
                kind: NamedKind::Record,
                args: Vec::new(),
            }),
        )];
        assert!(describe_claims_at(text, at, &on_record, tys).is_none());
        assert!(describe_claims_at(text, 0, &on_actor, tys).is_none());
    }
}
