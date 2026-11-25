# 1005 — A case sets the DMARC verdict through its headers, at every tier

- **Status:** Accepted (v0.739)

**Context.** Part of #3888 (Decision D). At `unit` and `integration` a `Sender`-bound handler always read `DmarcNone`, while at `system` a case set the verdict through an `authentication-results` header, so the same case meant different things at different tiers.

**Decision.** (D) The harness derives `dmarc` from the case's `InboundEmail.headers` with the runtime's own `dmarcVerdict` rule (exported as `dmarcVerdictOf`, so the two cannot diverge). A case without the header still reads `DmarcNone`, so existing cases are unchanged. The check a `Sender` refinement makes on the verdict is emitted at the start of the handler, so a unit or integration case is refused exactly as the Worker refuses mail (`Reject("sender not authenticated")`, the body not run); the route-level refusal in the Worker's entry stays as the earlier, cheaper check. A binder-less `by <Sender refinement>` handler receives its identity under a synthetic name, because the check reads it.

**Consequences.** A case that set an `authentication-results` header at `unit` now sees what `system` already saw. Fixes a defect in #3888 B: a handler written `by s: Authenticated` did not receive `SenderIdentity`, because the binder was recognised only for the prelude `Sender`; it now does, pinned by the fixture reading `s.identity.dmarc`. Hover on a `Sender` refinement is a follow-up.
