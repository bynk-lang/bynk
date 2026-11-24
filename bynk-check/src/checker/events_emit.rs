//! Events track, slice 0 (spine #936): owner-only emission.
//!
//! `Events.emit[E]` may only name an event `E` declared in *this* context, even though a
//! `consumes`-visible foreign event resolves here just as validly for every other purpose.
//! #4113: the check runs on `E` whether the call writes it or the checker infers it from the
//! argument, so inference cannot step around it.

use super::*;

/// Check `ty`, the type argument of `<cap>.<method>`, when that call is bynk's own
/// `Events.emit`: an event declared here is the owner and fine; a local non-event is
/// `bynk.event.emit_not_an_event`; a foreign type is `bynk.event.emit_outside_owner`.
///
/// First-party-gated like every other Events-special-cased site (mirrors #934's
/// `Idempotency` precedent): a bare `"Events"` name match would also fire for a third-party
/// capability that declares its own `Events` with an `emit` method.
pub(super) fn check_emit_owner(
    first_party: Option<crate::firstparty::FirstPartyCap>,
    method: &str,
    ty: TyId,
    span: Span,
    ctx: &mut Ctx,
) {
    if method != "emit" || first_party != Some(crate::firstparty::FirstPartyCap::Events) {
        return;
    }
    let tys = ctx.tys;
    let Ty::Named { name: ename, .. } = &*tys.get(ty) else {
        return;
    };
    if ctx.input.is_local_event(ename) {
        return;
    }
    if ctx.input.is_local_type(ename) {
        // `Events.emit[SomeLocalRecord]` once compiled clean, silently buffering an emission no
        // `from Events(...)` subscriber could match (`discover_event_subscribers` finds no owner
        // for a non-event name and drops it).
        ctx.errors.push(
            CompileError::new(
                "bynk.event.emit_not_an_event",
                span,
                format!(
                    "`{ename}` is not a declared `event` — `Events.emit` may only name an event type"
                ),
            )
            .with_note(
                "declare it with `event Name = { ... }`, or check that the type argument names the event you meant",
            ),
        );
    } else {
        ctx.errors.push(
            CompileError::new(
                "bynk.event.emit_outside_owner",
                span,
                format!(
                    "`{ename}` is not declared in this context — only the context that declares an event may emit it"
                ),
            )
            .with_note(
                "a foreign event is visible via `consumes` for subscription (`from Events(...)`), but only its owning context may `Events.emit` it",
            ),
        );
    }
}
