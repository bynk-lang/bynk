---
level: minor
changelog: In a bundle with a composition root, a context that requires a capability it declares (`given Warehouse`) but has no `provides Warehouse = …` for is now `bynk.capability.not_provided` at the `given`, as on Workers. `composeApp` builds every context's deps itself, so the gap reached `tsc` against `compose.ts` (TS2345) and was reported as a compiler bug (`bynk.codegen.tsc_failed`) by `bynk test` and `bynk build`, while `bynk check` passed. A bundle without a composition root still leaves the capability to the host
breaking: A bundle with a composition root (some context consumes another context's service or capability, emits events, or derives a capability) whose context requires a capability it declares but does not provide now fails `bynk check` with `bynk.capability.not_provided`; its `compose.ts` never type-checked. Add `provides Cap = …` to that context; a test `stub` still replaces it in the cases that stub it
---
