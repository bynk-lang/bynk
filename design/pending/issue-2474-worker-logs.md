---
level: minor
changelog: "`bynk dev` rewrites stack frames in the workers' output to `.bynk` locations, and `vscode-bynk` shows the workers' logs in a filterable Bynk Logs view"
---

## ADR: dev-worker-logs-frames-in-the-driver
title: Worker log stack frames are rewritten to `.bynk` in the driver, from the emitter's source maps
summary: bynk dev passes wrangler's output through a line rewriter; the editor's Bynk Logs view reads it as log events

**Context.** A handler that throws under `bynk dev` prints stack frames into generated TypeScript, which cannot be clicked through to the `.bynk` line that failed. The emitter already writes a source-map v3 document beside every module (ADR 0103, 0569), but only the debugger used them. Whether to map in the editor or in the driver, and whether to start with the dev server's logs or with a deployed Worker's tail (#2474), were open.

**Decision.** The driver does the mapping. When `bynk dev` is not attached to a terminal (stdout or stderr is not one), or is asked to (`--rewrite-frames`, or `--log-events`, which implies it), it pipes each `wrangler dev`'s stdout and stderr through a line-at-a-time rewriter (`bynk/src/logs.rs`) that replaces the position in a stack frame with the `.bynk` one, found in the module's `.ts.map`, and leaves any other line as it came. Attached to a terminal and not asked, `bynk dev` leaves wrangler's output exactly as it was, so a terminal user keeps wrangler's colour and hotkeys; the piped paths (the editor's task, CI) carry `.bynk` frames, and `--rewrite-frames` gives a terminal user the same. Under `--format json --log-events` each line is also a `log` event (`context`, `level`, `text`); the flag is opt-in so the existing lifecycle stream is unchanged for scripts. The editor shows the events in a *Bynk Logs* output channel with a bounded in-memory buffer (5,000 lines), a level and context filter, and no persistence. A deployed Worker's tail (`bynk tail`) is a second slice, not built here.

**Consequences.** Wrangler turns its colour and interactive hotkeys off when its output is a pipe; they stay on the terminal path, which is unchanged. On the piped paths the driver restores colour with `FORCE_COLOR` when its own stderr is a terminal, but not the hotkeys. A frame the map does not cover, or a module without a map, is passed through unchanged. Level is inferred from wrangler's own tags, so it is a heuristic; per-request grouping is not attempted.
