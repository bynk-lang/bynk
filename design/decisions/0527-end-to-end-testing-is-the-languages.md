# 0527 — End-to-end testing against a running system is the language's, as a run of the `system` suites

- **Status:** Accepted (v0.367)

**Context.** ADR 0207 placed the top rung, a deployed instance, outside the language ("e2e (Postman, external)"). So every project hand-rolled it in another language, with hand-signed tokens, untyped string matching and port guessing. `bynk-lang/shop` keeps a Bash scenario script for exactly this. The `system` drivers already build a concrete `Request` for a route and decode its `Response`; only the hop between them was in process. (End-to-end testing track §3.0, #2138.)

**Decision.** Running the `system` suites against a running system is the language's. The same case bodies run against a deployment by a choice of the run, `bynk test --against`, without rewriting them elsewhere. What stays external is what was always the point of "proper auth": a real identity provider's credential ceremony.

**Consequences.** ADR 0207's "e2e is external" line is superseded for running against a deployed instance. The in-process `system` tier stays the default and the gate.
