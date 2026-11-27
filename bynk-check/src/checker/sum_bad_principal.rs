//! #4385: a bad-credential principal on an actor sum. `by Forged`, `by Expired`, `by Stale` and
//! `by Revoked` present one member's refusal, so on a sum they name the member: `by Forged(Session)`.
//! The argument is an identifier resolved against the sum's members and never evaluated; a bad
//! credential carries no identity, so the slot is free, and it is told apart from an actor's identity
//! (`by User("ada")`, a value) by being a bare name. A bare `by Forged` still reaches the one member
//! that can refuse it; with two or more it is ambiguous and is refused.

use super::*;
use crate::actors::{BadCredential, sum_refusing_arms};

/// Check `by <Bad>[(<Arm>)]` against the sum `by`. `true` when this decided the principal (so the
/// caller stops); `false` leaves it to the single-actor rules, which a bare principal with at most
/// one refusing member keeps, byte for byte.
pub(super) fn check(
    p: &bynk_syntax::ast::CallSiteActor,
    bad: BadCredential,
    by: &bynk_syntax::ast::ByClause,
    ctx: &mut Ctx,
) -> bool {
    let name = p.actor.name.as_str();
    // The `Oidc`-only principals are not offered by member: a bare one reaches the sum's `Oidc`
    // member under the single-actor rules, and a named one is refused rather than read as an identity.
    if !matches!(
        bad,
        BadCredential::Forged
            | BadCredential::Expired
            | BadCredential::Stale
            | BadCredential::Revoked
    ) {
        if crate::actors::bad_principal_arm(p).is_none() {
            return false;
        }
        ctx.errors.push(
            CompileError::new(
                "bynk.test.actor_no_identity",
                p.span,
                format!(
                    "`{name}` is not offered by member on an actor sum, so it takes no member name"
                ),
            )
            .with_note(format!(
                "write `by {name}`: it is presented to the sum's `Oidc` member"
            )),
        );
        return true;
    }
    let refusing = sum_refusing_arms(by, bad, &ctx.test_actors);
    let Some(arm) = crate::actors::bad_principal_arm(p) else {
        if p.identity.is_none() && refusing.len() > 1 {
            ctx.errors.push(
                CompileError::new(
                    "bynk.test.bad_principal_needs_arm",
                    p.span,
                    format!(
                        "`by {name}` on this actor sum could refuse more than one member ({}), so it must name the one to test",
                        refusing.join(", ")
                    ),
                )
                .with_note(format!(
                    "write `by {name}({})`, naming the member whose refusal the case checks",
                    refusing[0]
                )),
            );
            return true;
        }
        return sole_refuser_names_itself(p, bad, &refusing, ctx);
    };
    if !by.actors.iter().any(|a| a.name == arm) {
        let members = by
            .actors
            .iter()
            .map(|a| a.name.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        ctx.errors.push(
            CompileError::new(
                "bynk.test.principal_identity_mismatch",
                p.span,
                format!("`by {name}({arm})` names `{arm}`, which is not a member of this sum (`{members}`)"),
            )
            .with_note(format!("name one of the members, e.g. `by {name}({})`", by.actors[0].name)),
        );
        return true;
    }
    if !refusing.iter().any(|m| m == arm) {
        let (code, what) = match bad {
            BadCredential::Forged => (
                "bynk.test.forged_needs_signature_route",
                "a `Signature`, `Bearer`, `Oidc` or `SignedCookie` seam",
            ),
            BadCredential::Expired => (
                "bynk.test.expired_needs_bearer_route",
                "a `Bearer`, `Oidc` or `SignedCookie` seam",
            ),
            BadCredential::Stale => (
                "bynk.test.stale_needs_timestamp",
                "a `Signature` seam that declares a `timestamp` and a `tolerance`",
            ),
            BadCredential::Revoked => (
                "bynk.test.revoked_needs_sessions_route",
                "a `SignedCookie` seam with `sessions = true`",
            ),
            BadCredential::WrongIssuer
            | BadCredential::WrongAudience
            | BadCredential::WrongAlgorithm
            | BadCredential::NoSubject
            | BadCredential::Malformed
            | BadCredential::Unknown => unreachable!("handled before the member lookup"),
        };
        let err = CompileError::new(
                code,
                p.span,
                format!("`by {name}({arm})` presents a credential that only {what} refuses, but the `{arm}` member has none"),
            )
            .with_note(format!(
                "name a member that can refuse it{}",
                match refusing.first() {
                    Some(m) => format!(", e.g. `by {name}({m})`"),
                    None => String::new(),
                }
            ));
        // An open member (`Visitor`) catches every bad credential, so no member refuses it: say that
        // rather than blaming the named member.
        ctx.errors.push(
            if crate::actors::sum_has_open_member(by, &ctx.test_actors) {
                err.with_note(super::calls::OPEN_SUM_NOTE)
            } else {
                err
            },
        );
    }
    true
}

/// A bare principal that exactly one member can refuse. A `Bearer` or `Signature` member is what a
/// bare principal always drove, so the single-actor rules keep it (`false`). Any other member (`Oidc`,
/// `SignedCookie`) has no such driver, so the checker records it as the arm and the case drives it
/// as if written `by <Bad>(<member>)` (`true`).
fn sole_refuser_names_itself(
    p: &bynk_syntax::ast::CallSiteActor,
    bad: BadCredential,
    refusing: &[String],
    ctx: &mut Ctx,
) -> bool {
    let [member] = refusing else {
        return false;
    };
    let scheme = ctx
        .test_actors
        .get(member)
        .and_then(|d| d.auth.as_ref())
        .map(|a| a.name.as_str());
    if !matches!(scheme, Some("Oidc" | "SignedCookie")) || p.identity.is_some() {
        return false;
    }
    ctx.callees.insert(
        p.id,
        Callee::Principal(crate::actors::CallSitePrincipal::BadCredentialArm(
            bad,
            member.clone(),
        )),
    );
    true
}
