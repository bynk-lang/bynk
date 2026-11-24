# 0909 — The virtual-time settle helper accepts any deps object

- **Status:** Accepted (v0.655.2)

**Context.** `__bynkSettleVirtual` typed its deps as `{ __exec?: … }`, a weak type, so a deps object without `__exec` (a unit-tier case stubbing only a capability) failed `tsc --strict` with TS2559 (#4189).

**Decision.** Widen the parameter to `object & { __exec?: … }`. Nothing else changes in the emitted call.

**Consequences.** Every emitted test module embedding the helper changes by that one line.
