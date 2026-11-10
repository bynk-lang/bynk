---
level: patch
probes: ts_writes
changelog: "The emitter builds an agent's state record in `emit_agent_state_record` and writes its `loadState` and `commitState` methods together in `emit_agent_load_commit_state`, as part of splitting `emit_agent` (#3167)"
---
