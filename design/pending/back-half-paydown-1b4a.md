---
level: patch
changelog: Internal restructuring: `bynk-emit` gets one definition of each small TypeScript node builder (`ident`, `str_lit`, `member`, `call` and the rest), in `ts_build`; no emitted output changes
probes: node_builder_defs
---
refactor: one definition for each shared node builder (1B.4a)

Part of #4051. Part of #3676.

No output changes.
