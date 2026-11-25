---
level: patch
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: `self.op(…)` inside an agent handler is refused as `bynk.agents.self_operation`, naming the agent and the operation and showing `Agent(self.id).op(…)`, where it reported a field of the internal `__<Agent>Self` record (#4291)
---
