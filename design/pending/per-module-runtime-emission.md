---
level: patch
changelog: A project emits only the runtime modules it reaches, as `__bynk_runtime/<module>` files, instead of one `runtime.ts` (#2441, ADR 0615)
probes: ts_writes
---
