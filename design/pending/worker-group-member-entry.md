---
level: patch
changelog: A Worker entry can be emitted as one member of a `[deploy.workers]` group, marking its router's own "not mine" answers so a group's dispatcher can try the next member (#3562, G2b step 2c-4a); no output changes until groups are switched on
probes: emit_diagnostics
---

A group is one Worker over several contexts, each with its own `fetch`. A
member's entry answers only what it owns: it tests that an internal
`/_bynk/call`, `/_bynk/event` or `/_bynk/migrate` target is its own **before**
reading the body or verifying a signature, and it marks the router's own
misses (the final `404`, the synthesised `405`/`204` with `Allow`, an unknown
admin target) in a module-level `WeakSet`, read through the exported `isMiss`.
A response a handler returned, including a handler's own `404`, is never
marked. The mark is not a header, so it cannot reach a client. The layout
decides membership (`WorkerLayout::is_member`), and it is identity until 2c-4b,
so every existing build is byte-identical.
