---
level: patch
changelog: A provider that calls an agent handler must list the handler's `given` capabilities, transitively, in its own `given` (`bynk.provider.agent_call_undeclared_capability`). Before, `bynk check` passed and `tsc` failed later (#2402)
---
