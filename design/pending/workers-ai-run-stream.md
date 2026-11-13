---
level: minor
changelog: `Ai.runStream(model, input)` streams a Workers AI model's own events as a `Stream[String]`, ready to serve with `Streaming`; a refusal before the stream is an `Err`, a failure after it faults the stream, and a client that hangs up cancels the model
probes: emit_abi_shapes, ts_writes
---
