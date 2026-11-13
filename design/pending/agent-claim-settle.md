---
level: patch
changelog: A `claim` in an agent handler is settled like a service handler's, released when the handler fails and confirmed otherwise, after the state commit and events flush
probes: emit_abi_shapes, ts_writes
---
