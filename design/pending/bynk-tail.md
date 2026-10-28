---
level: minor
changelog: "`bynk tail` streams a deployed Worker's logs with stack frames rewritten to `.bynk` locations"
---

## ADR: bynk-tail-rewrites-frames
title: `bynk tail` streams a deployed Worker's logs with `.bynk` frames
summary: The second slice of ADR 0622, reusing the dev rewriter over `wrangler tail`

**Context.** ADR 0622 rewrote stack frames in `bynk dev`'s output to `.bynk` locations and left a deployed Worker's tail as a second slice (#2546).

**Decision.** `bynk tail [PATH] [--context NAME] [--env NAME] [-- <wrangler args>]` compiles the project into `.bynk/tail/` (nothing is deployed, provisioned or locked), resolves the Worker's deployed name as `bynk deploy` does (`--env` qualified), runs `wrangler tail <name>` from that Worker's directory, and pipes both of its streams through the `bynk dev` line rewriter and its source maps. One Worker is tailed per invocation, so a project with several contexts needs `--context`. Output is human text only; a `log`-event stream is not added here.

**Consequences.** Mapping needs the tail's frames to name a module the build has a map for. A frame that names the deployed bundle, or a module with no map, passes through unchanged, as in `bynk dev`. The mapped source is the current checkout, so tailing a Worker deployed from different sources can point at the wrong line.
