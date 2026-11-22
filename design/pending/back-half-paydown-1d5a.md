---
level: patch
changelog: Internal restructuring: the `BYNK_*` names the toolchain shares with generated TypeScript have one Rust definition; no name or emitted output changes
probes: env_protocol_literals
---
refactor: one table for the cross-language `BYNK_*` protocol (1D.5a)

Part of #4003. Part of #3676.

No output changes. `bynk-project/src/env_protocol.rs` defines the 20 protocol keys (24 names a reader spells) with, for each, whether it is a process variable, a Worker variable, a binding or a secret, whether it is a name or a prefix family, the Rust that sets it and the TypeScript that reads it. The Rust writers use its constants: the bare literals in `test_runner.rs`, `emit.rs` and `wrangler.rs` are gone, and the older constant paths (`wire_format::EDGE_FROM_PREFIX`, `dev_ack::DEV_VAR`, `CONTAINERS_UNAVAILABLE_VAR`, `FILTER_ENV` and the rest) are `pub use`s of the table. `xtask/tests/env_protocol.rs` holds the hand-written TypeScript readers to the table, replacing the two ad hoc checks. `env_protocol_literals` is a new gated growth probe: it read 36 before this slice and reads 4 after it, the named residual of `tests_emit/test_main.rs` (3, until 1B.2) and `deploy/config.rs` (1, until 1C.2).
