# 0506 — The editor runs `bynk dev` as a `CustomExecution` task owned by a dev-server controller

- **Status:** Accepted (v0.354)

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
spawn. Until then, a rebuild's outcome is written without an end marker, so a
save during boot can't end the start-up cycle early. If the ports still don't
answer after 120 seconds, the controller writes a `ready, but not every port
answered` marker and shows a warning in the status bar instead of the spinner.
That way a `preLaunchTask` never waits forever on a port the probe can't
reach. Stop sends SIGTERM to the process group, which `bynk dev` traps to stop
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
