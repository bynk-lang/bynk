---
level: minor
changelog: "`bynk dev --format json` writes one lifecycle event per line (`serving` with each context's ports, `rebuilding`, `rebuild_ok`, `rebuild_failed`, `stopped`); VS Code gains **Bynk: Start/Stop/Restart Dev Server**, a `bynk: dev` task and a status-bar item with each context's URL"
---

## ADR: bynk-dev-event-stream
title: "`bynk dev --format json` is a line-delimited lifecycle event stream"
summary: A stable, pinned event stream for editors and CI instead of parsing the human serving report

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

## ADR: vscode-dev-server-controller
title: The editor runs `bynk dev` as a `CustomExecution` task owned by a dev-server controller
summary: A task for the terminal, stop/restart and preLaunchTask; a pseudoterminal so the controller can read the event stream

**Context.** #2094 asks for one command that starts `bynk dev`, a status bar
that shows what is serving, and a `bynk: dev` task that works as a
`preLaunchTask`. The proposal recommended a `vscode.Task` with a
`ShellExecution` (DECISION B). A shell task gives the extension no access to the
process's output, though, so the controller could not read the
`--format json` events it needs for the status bar.

**Decision.** The `bynk: dev` task (type `bynk`, `command: "dev"`, optional
`contexts` and `basePort`) uses a `CustomExecution`. Its pseudoterminal is
driven by a single dev-server controller that spawns `bynk dev --format json` in
its own process group, turns event lines into state, and passes everything else
to the terminal unchanged. The controller writes one `[bynk dev] …` marker line
per lifecycle step. The `$bynk-dev` problem matcher's background patterns key on
those lines, and `ready` is written only once every served port accepts a
connection, so a `preLaunchTask` waits for the workers rather than for the
spawn. Stop sends SIGTERM to the process group, which `bynk dev` traps to stop
every `wrangler` and `workerd`. A failing rebuild runs `bynk check . --format
short` and fills a `bynk-dev` diagnostic collection, which the next good
rebuild clears.

A plain dev server and a `mode: "dev"` debug session bind the same ports, so
they are mutually exclusive: starting that debug session while the dev server
runs asks "Stop the running dev server and debug?" (DECISION C).

**Consequences.** The user still gets a readable terminal, the standard task
stop and restart, and `preLaunchTask` composition, which is what DECISION B was
for. The spawning, settings and process-group handling live in a shared CLI
runner (`vscode-bynk/src/cli.ts`) that later editor commands driving `bynk`
reuse. The proposal's `bynk stop` recovery command is not offered: the driver
has no such command, and `bynk dev` already sweeps leftover `workerd`
processes when it stops.
