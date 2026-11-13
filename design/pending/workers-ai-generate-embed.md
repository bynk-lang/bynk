---
level: minor
changelog: `Ai.generate` and `Ai.embed` are typed Workers AI operations for chat generation and text embeddings, returning concrete records and vectors, with a `BadReply` error for a reply that is not the shape promised
probes: emit_abi_shapes, ts_writes
---
