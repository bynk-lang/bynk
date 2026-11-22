---
level: patch
changelog: Internal restructuring: npm, pnpm, bun, yarn, `tsc`, `tsx`, `node` and `git` are spawned from one module; no lookup, message or fetched package changes
probes: tool_spawn_sites
---
refactor(driver): one module for npm, tsc, tsx, node and git spawning (1D.5b)

Part of #4003. Part of #3676.

No output changes. `bynk-driver/src/tools/` holds the version pins (`pins::typescript_spec()`, `typescript@<major>`, and `pins::TSX_SPEC`, unpinned), the `tsc` and `tsx` runner ladders (`Ladder::{tsc, tsx}`, with `tool_ladder`, `resolve_tool` and `start_failure` moved in), the `node` and `git` commands and `run_to_completion`, which installs use. `bynk test`'s `tsc`, `tsx` and `node` launches, `bynk test --inspect`'s `node`, `bynk check --emit`'s `tsc`, the npm/pnpm/bun/yarn installs, `--changed`'s `git` and `bynk fetch`'s `git` go through it, each with the lookup it had. The `tsc` ladder is written once, and `typescript@<major>` is formatted in one place; the stale `typescript@5` in `npm_install.rs` is gone. Wrangler spawning stays for 1D.3. `tool_spawn_sites` is a new gated growth probe: it read 20 before this slice and reads 12 after it (wrangler 2, process control 8, self re-execution 2, other 0).
