# 0505 — "`bynk dev --format json` is a line-delimited lifecycle event stream"

- **Status:** Accepted (v0.354)

**Context.** The VS Code extension (#2094) needs to know when `bynk dev` is
serving, which context is on which port, and how each rebuild went. The only
source was the human serving report on stderr, whose wording is UX and changes,
and which prints nothing at all for a lone context on wrangler's default port.
CI scripts that start `bynk dev` and wait for it have the same need.

**Decision.** `bynk dev` takes `--format human|json` (default `human`). Under
`json` it writes one JSON object per line to stdout, tagged by `event`:

- `serving`: every `wrangler dev` has been started. `contexts` lists one
  `{ name, port, url, inspector_port }` per served context. `name` is the
  worker-directory form `--context` accepts. `port` is never null: a lone
  context on wrangler's default reports the port it gets (a `-- --port N`
  passthrough, else 8787). `inspector_port` is null when none was allocated.
- `rebuilding`, `rebuild_ok`, `rebuild_failed`: a watched change, and the
  rebuild's outcome. Diagnostics stay on stderr.
- `stopped`: the session ended, with the exit `code` `bynk dev` returns.

Keys are snake_case, as in `bynk deploy --format json` (the proposal sketched
`inspectorPort`; the driver's existing JSON surfaces decided it). The human
stream on stderr is unchanged under `json`, and `wrangler dev`'s own stdout
moves to stderr, so stdout carries only events. A pre-flight or build failure
before serving writes no events. Consumers must ignore unknown events and
fields: events and fields may be added, and existing ones keep their meaning.

**Consequences.** The event lines are pinned by unit tests in `bynk/src/dev.rs`
and mirrored in the extension's unit tests, so a change to either side fails a
test. `serving` marks the workers as started, not as answering; a consumer that
needs readiness polls each `url`, as the extension does.
