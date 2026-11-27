---
title: "CLI (`bynk` driver)"
---
The **`bynk`** driver is the developer front-end — it **links the compiler
pipeline in-process** and orchestrates the Node toolchain. A fresh
`cargo install bynk` is self-contained: it compiles, checks, formats and tests
with nothing else installed. This page is the argument and exit-code reference
for its subcommands.

The everyday commands — [`bynk check`](#bynk-check), [`bynk compile`](#bynk-compile), [`bynk build`](#bynk-build),
[`bynk fmt`](#bynk-fmt), and [`bynk test`](#bynk-test) — are what the retired
`bynkc` binary used to provide, with the same flags, output and exit codes.

## Build progress

`bynk build`, `bynk dev` and `bynk deploy` print a line as each context compiles,
dependencies first, when stderr is a terminal:

```text
 Compiling shop.billing
  Finished shop.billing
 Compiling shop.orders
  Finished shop.orders
```

A context with errors prints `Compiling` and no `Finished`; its diagnostics
follow. `--quiet` (`-q`, accepted before or after the command) turns the lines
off. They are never printed when stderr is a pipe or a file, so a script or CI
log sees the same output as before. `bynk dev` reports its first build only: a
rebuild while watching stays quiet. `bynk check` and `bynk compile` print no
progress.

## Colour

`--color WHEN` (or `--colour WHEN`, accepted before or after the command) says
when `bynk` colours its output:

| `WHEN` | Colour |
| ------ | ------ |
| `auto` (default) | When the stream is a terminal and `NO_COLOR` is unset or empty. `FORCE_COLOR` or `CLICOLOR_FORCE`, set to anything but empty or `0`, colours it without a terminal (for `less -R` or a CI log that renders ANSI); `NO_COLOR` wins over both. |
| `always` | Always, into a pipe or a file too, whatever the environment says. |
| `never` | Never, whatever the environment says. |

Each colour has one meaning: **blue** for information, **green** for success,
**red** for an error or a failure, **amber** for a warning, and **bold** for a
heading. What is coloured:

- `bynk test`: a passing case's ✓ green, a failing case red, suite headings
  bold, and the closing tally.
- `--coverage`: each file's bar and percentage green from 80%, amber from 50%
  and red below, with its uncovered lines red.
- `--mutate`: each mutant's progress line by its outcome (killed or timed out
  green, survived red, not covered by any suite amber, invalid blue), and in the
  report the counts, the `SURVIVED` and `NO SUITE` labels, and the score, banded
  as coverage is.
- `bynk revert`: each Worker's heading bold, each check's `ok` green or
  `refused` red, and the verdict.
- The `error:`, `warning:` and `note:` label of a message on stderr, such as
  `bynk test: warning: no tests found`, and its coded form, `error[…]:`. Each
  message of the driver's own carries one after the name of who is speaking:
  `error:` when the command then fails (`bynk: error: not inside a Bynk
  project …`), `warning:` when it carries on in a degraded state, and `note:`
  when the message asks you to act. Progress and information carry none.
- `bynk doctor`'s marks, rich diagnostics, the
  [build progress](#build-progress) lines, and `--help`.

Machine-readable output (`--format json` and `ndjson`, and `bynk check`'s and
`bynk doctor`'s `--format short`) is never coloured. Under `auto` nothing
written to a pipe or a file is coloured either, so a script reading
`bynk revert`'s plan sees plain text. The choice is passed on to the `wrangler`
that `bynk dev` and `bynk tail` start, as `NO_COLOR=1` under `never` and
`FORCE_COLOR=1` under `always`.

## Installing npm packages

A generated build can depend on npm packages: `postal-mime` for inbound email,
`postgres` or `mysql2` for a database, an approved adapter's own. `bynk test`,
`bynk dev` and `bynk deploy` install them into the directory they generate
(`out`, `.bynk/dev`, `.bynk/deploy`), before wrangler runs, and only when a listed
package is missing. A warm re-run installs nothing and needs no network. Lifecycle
scripts are always off.

The package manager is the one you already use. The first match wins:

1. `BYNK_PACKAGE_MANAGER`, then [`[build] package_manager`](/reference/tooling/manifest/#build)
   in `bynk.toml` (`npm`, `pnpm`, `bun` or `yarn`).
2. The `packageManager` field of the project root's `package.json`.
3. A lockfile at the project root: `pnpm-lock.yaml`, `bun.lock` or `bun.lockb`,
   `yarn.lock`, `package-lock.json`.
4. `npm_config_user_agent`, set when bynk runs under `pnpm exec`, `npx`, `bunx` or
   `yarn`.
5. The first of `npm`, `pnpm` and `bun` on `PATH`.

| Tool | Command |
| --- | --- |
| npm | `npm install --no-audit --no-fund --ignore-scripts` |
| pnpm | `pnpm install --ignore-scripts --ignore-workspace` |
| bun | `bun install --ignore-scripts` |
| Yarn | `yarn install --ignore-scripts --non-interactive` (Yarn classic only; Yarn Berry is refused, because its Plug'n'Play layout leaves no `node_modules`) |

The same detection chooses the tool for bynk's *own* tools (`tsc`, `tsx` and
`wrangler`). When one is not installed, bynk fetches it with the detected tool's
runner, and `bynk doctor` and the error messages give that tool's install
command, not `npx` and `npm install -g`:

| Tool | Runs a package's binary | Installs globally | Adds to the project |
| --- | --- | --- | --- |
| npm | `npx --yes -p <pkg> <bin>` | `npm install -g <pkg>` | `npm install --save-dev <pkg>` |
| pnpm | `pnpm dlx --package <pkg> <bin>` | `pnpm add -g <pkg>` | `pnpm add -D <pkg>` |
| bun | `bunx --package <pkg> <bin>` | `bun add -g <pkg>` | `bun add -d <pkg>` |
| Yarn classic | none: `npx` if it is on `PATH` | `yarn global add <pkg>` | `yarn add -D <pkg>` |

Yarn classic has no runner, so a Yarn project falls back to `npx` when it is on
`PATH`, and otherwise gets the install command alone. A detected tool that is not
installed falls back to `npx` the same way. With nothing detected, bynk uses npm,
as it always has.

The generated directory has no lockfile to respect: whichever tool installs writes
its own there, in a directory bynk regenerates. [`bynk.lock`](/book/guides/projects-build-and-deployment/layout/)
stays the source of truth, and an adapter's npm packages must still be approved
with `bynk approve` before anything installs them. [`bynk doctor`](#bynk-doctor)
reports which package manager it detected and which rule chose it.

`bynk test`, `bynk dev` and `bynk deploy` take `--no-install` (or `BYNK_NO_INSTALL=1`, for CI),
for projects that provision `node_modules` themselves. A missing package then
stops the build before wrangler, naming the detected command, for example
`pnpm install --ignore-scripts --ignore-workspace`, and the directory to run it
in. `--dry-run` never installs: it reports what it would install, and with which
tool. A failed install stops with the tool's error, the command and the
directory, so it can be run by hand.

## Wrangler telemetry

`bynk dev`, `bynk deploy` and the other commands that run Wrangler leave its own
settings as they are, so Wrangler may report anonymous usage data to Cloudflare as it
does when you run it yourself. To turn that off for them, set
`WRANGLER_SEND_METRICS=false` in the environment (or `send_metrics = false` in a
Wrangler configuration); Wrangler reads it in the process Bynk starts, which inherits
it. Bynk's own test suite sets the variable for every Wrangler it starts.

## Exit codes and diagnostics

A diagnostic's **severity** decides whether it fails a build. An **`Error`**
rejects the program: `bynk compile` and `bynk check` exit non-zero and produce no
output. A **`Warning`** is surfaced but does **not** fail the build: these commands
still **succeed (exit 0)** and emit their output, with warnings reported alongside.
The build-failure gate counts error-severity diagnostics only. See the normative
rule in the [specification](/reference/spec/diagnostics/) and the
[diagnostic index](/reference/diagnostics/) (warning-severity codes are
marked *(warning)*).

```text
bynk <command> [options]
```

| Command | What it does |
|---|---|
| [`bynk doctor`](#bynk-doctor) | Check whether your machine is ready to compile, test, and deploy. |
| [`bynk new`](#bynk-new) | Scaffold a new, runnable project. |
| [`bynk dev`](#bynk-dev) | Build the project and serve it locally with `wrangler dev`. |
| [`bynk deploy`](#bynk-deploy) | Provision each context's Cloudflare resources and deploy every Worker, in dependency order. |
| [`bynk check`](#bynk-check) | Type-check a file or project without writing compiled output. |
| [`bynk build`](#bynk-build) | Build a project into its output directory with the settings in `bynk.toml`'s `[build]`; no flags needed. |
| [`bynk compile`](#bynk-compile) | Compile a project to TypeScript in a directory, optionally from unsaved editor buffers. |
| [`bynk client`](#bynk-client) | Write a typed TypeScript client for the project's `from http` services (experimental). |
| [`bynk openapi`](#bynk-openapi) | Write an OpenAPI 3.1 document for the project's `from http` services. |
| [`bynk token`](#bynk-token) | Mint a credential for the project's own `Bearer` actor, to call a guarded route by hand. |
| [`bynk fmt`](#bynk-fmt) | Format `.bynk` source files in place. |
| [`bynk test`](#bynk-test) | Discover and run a project's tests. |
| [`bynk fetch`](#bynk-fetch) | Resolve the project's dependencies, fetch git ones into the store, and write `bynk.lock`. |
| [`bynk approve`](#bynk-approve) | Approve the npm packages dependencies' adapters bring into the build. |
| [`bynk update`](#bynk-update) | Re-resolve git dependencies' revisions and rewrite `bynk.lock`. |
| [`bynk revert`](#bynk-revert) | Make a Worker's previous deployment live again without a rebuild, after checking what makes that unsafe. |
| [`bynk migrate`](#bynk-migrate) | Show where each agent class's stored state stands, and sweep deployed instances so old `migrate` steps can retire. |
| [`bynk add`](#bynk-add) | Add a registry dependency to `bynk.toml`, then resolve and lock it. |
| [`bynk search`](#bynk-search) | Look a package up on its registry: every version, and the newest usable one. |
| [`bynk deps`](#bynk-deps) | List the project's dependencies and what `bynk.lock` resolved each to. |
| [`bynk publish`](#bynk-publish) | Check, pack and upload the package to its registry's publish API. |
| [`bynk yank`](#bynk-yank) | Yank (or un-yank) a published version through its registry's API. |
| [`bynk login`](#bynk-login) | Sign in to a registry's accounts with GitHub. |
| [`bynk logout`](#bynk-logout) | Sign out of a registry's accounts. |
| [`bynk org`](#bynk-org) | Claim and block an organisation, manage its members, and mint and revoke its publish tokens. |
| [`bynk registry`](#bynk-registry) | Build and maintain a package registry in a directory, and serve it with a publish API. |
| [`bynk explain`](#bynk-explain) | Explain a diagnostic code — what the rule is, why it exists, how to fix it. |
| [`bynk book`](#bynk-book) | Print the Bynk Book as this toolchain documents it. |
| [`bynk mcp`](#bynk-mcp) | Serve the toolchain's read-only answers to an AI agent over the Model Context Protocol (stdio). |

---

## `bynk new`

Scaffold a new project: a complete, runnable single-context HTTP service you can
serve immediately with [`bynk dev`](#bynk-dev). See the guide [Start a new
project](/book/guides/projects-build-and-deployment/start-a-project/) for a worked
walkthrough.

```text
bynk new <PATH> [--name NAME]
```

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | *(required)* | Directory to create for the new project (e.g. `hello` or `./hello`). Parent directories are created. |
| `--name NAME` | `PATH`'s final component | Project name / context identifier. Must be a legal Bynk identifier (a letter followed by letters, digits, or underscores — no dashes or dots). |

**What it writes**

```text
<PATH>/
├── bynk.toml            # [project] name/version + optional [paths] include/exclude
├── .gitignore           # /out, /.bynk, .wrangler/, node_modules/
├── .gitattributes       # *.bynk text eol=lf
├── src/
│   └── <name>.bynk      # context <name> — a GET "/" HTTP service
└── tests/
    └── <name>.test.bynk # one case that drives that route and checks its answer
```

**Behaviour** — `bynk new` is pure, offline file-writing: it shells nothing,
compiles nothing, and reads no network, so it works before Node or
`wrangler` are installed.

1. Derive the project name from `PATH`'s final component (or `--name`) and
   validate it as a legal Bynk identifier — both `[project] name` and the
   starter's context use it.
2. Refuse to clobber: if the target exists and is non-empty, fail before writing
   anything. An empty directory is fine; VCS/OS cruft (`.git`, `.gitignore`,
   `.gitattributes`, `.DS_Store`, …) doesn't count as non-empty.
3. Write the scaffold and print next steps (`cd <path>`, then `bynk test` and
   `bynk dev`). `bynk test` passes on the project as generated.

**Exit code** — `0` on a written scaffold. A non-empty target or a name that
isn't a legal identifier exits non-zero, **touching nothing**.

**Notes**

- `bynk new` never overwrites a file it didn't create, and never runs `git init`
  or writes outside the project — the scaffold drops cleanly into an existing
  repository.
- The `.gitignore` covers only `/.bynk`, the build directory
  [`bynk dev`](#bynk-dev) writes (compiled workers and local wrangler state).
- The `.gitattributes` keeps `.bynk` files LF on every platform, the line
  ending [`bynk fmt`](#bynk-fmt) writes. Neither file is written over one that
  already exists.

---

## `bynk dev`

Build the project and serve it locally — one step in place of the manual
`bynk compile` + `cd` + `wrangler dev` recipe. See the guide [Run your project
locally](/book/guides/projects-build-and-deployment/run-locally/) for a worked
walkthrough.

```text
bynk dev [PATH] [--context NAME]... [--base-port N] [--inspect] [--inspect-port N] [--env NAME] [--local-worker NAME]... [--format human|json] [--log-events] [--trace-events] [--rewrite-frames] [--ready-json FILE] [--no-install] [-- <wrangler args>]
bynk dev inspect agents|agent …   # read a running session; see Inspecting agents
```

`bynk dev` serves one project, so it refuses to run at a
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces):
run it inside a member.

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | `.` | A directory inside the project. The root is found by walking up for `bynk.toml`. |
| `--context NAME` | every context | Which context to serve. **Repeatable**; omit to serve them all with their service bindings wired. Accepts the dotted name (`a.b`) or its worker-directory form (`a-b`). A member of a [`[deploy.workers.<key>]`](/reference/tooling/manifest/#worker-groups) group serves the whole group, which is one Worker on one port, and the command says so; the group's key is accepted too. |
| `--base-port N` | `8787` | First port of the per-context allocation: context *i* is served on `N + i`, in the order listed. |
| `--inspect` | off | Serve with the V8 inspector enabled (`wrangler dev --inspector-port`) so a JavaScript debugger can attach. Breakpoints set in `.bynk` sources resolve through the emitted source maps, composed into the worker bundle. Prints an inspector URL per context on start. |
| `--inspect-port N` | `9229` under `--inspect` or with several contexts; a free port otherwise | First inspector port, allocated per context exactly as `--base-port` is, and always honoured, for a single context too. wrangler always starts an inspector, so left on its default every session would race for 9229. Several contexts, or `--inspect`, start from 9229 unless you pass a port; a single context without `--inspect` is given a free port, so sessions started together do not collide. A port something already holds is reported by `bynk dev` before wrangler starts. A range that overlaps the `--base-port` range is an error. |
| `--no-install` | off | Do not install the build's npm packages; stop, naming the detected install command, when one is missing. Also `BYNK_NO_INSTALL=1`. See [Installing npm packages](#installing-npm-packages). |
| `--env NAME` | `default` | The environment whose `[env.<name>.vars]` values the project's [`env(…)` constants](/reference/constants/#values-that-differ-per-environment) take for the build `dev` runs, so a local run can use staging's values. With `-- --remote`, it also selects which `bynk.deploy.lock` section the remote resource ids are read from. |
| `--local-worker NAME` | none | A Worker outside the project, named by a [`Service` instance](/book/guides/effects-and-capabilities/call-a-foreign-worker/) (`@worker("NAME")`), to reach in your own locally running `wrangler dev` of it instead of the stand-in `bynk dev` binds by default. **Repeatable.** A name no served context binds is warned about. |
| `--format human\|json` | `human` | `human` is the readable stream on stderr. `json` adds one JSON lifecycle event per line on stdout (see [Lifecycle events](#lifecycle-events)), and moves `wrangler dev`'s own stdout to stderr so stdout carries only events. |
| `--log-events` | off | With `--format json`, also write each line a worker prints as a `log` event (see [Worker logs](#worker-logs)). |
| `--trace-events` | off | Write one `request` event per request a worker serves: its route, status, duration and the steps it took (see [Request traces](#request-traces)). Implies `--format json` and `--rewrite-frames`. |
| `--rewrite-frames` | off | Rewrite stack frames to `.bynk` locations even on a terminal (see [Worker logs](#worker-logs)). Implied by `--log-events` and `--trace-events`. |
| `--ready-json FILE` | none | Once every served context answers an HTTP probe, write `{ "<worker>": "<url>" }` to `FILE` (see [Readiness](#readiness)). A stale `FILE` is removed at start, and `FILE` is removed when the session stops. |
| `-- <wrangler args>` | — | Everything after `--` is forwarded to `wrangler dev` verbatim (e.g. `-- --var K:V`). Ports are the driver's to allocate: passing `-- --port` or `-- --inspector-port` against an allocation is an error naming the flag that owns it. A single context with no `--inspect` or `--inspect-port` leaves `-- --inspector-port N` yours. |

**Behaviour**

1. Locate the project root and read `[paths] include`.
2. Pre-flight the `deploy` capability (Node, `wrangler`) exactly as
   [`doctor`](#bynk-doctor) does; a missing required tool fails here, before any
   build, with doctor's remedy text.
3. Compile to the managed **`.bynk/dev/`** build directory (gitignored
   automatically; the `workers/` tree is cleared before each build).
4. Select the workers: every context, or the `--context` subset. An unknown
   name fails and lists the available contexts.
5. Run **one `wrangler dev` per context**, each from inside its own worker
   directory, in local mode (Miniflare) — **no namespace provisioning is
   needed**. Each is served from `wrangler.dev.toml`, a copy of its generated
   `wrangler.toml` with the development bindings applied, replaced whole after
   each build so it never carries the deploy's names. The processes discover each
   other through wrangler's dev registry and wire the generated `[[services]]`
   bindings between themselves, so a cross-context call resolves locally.
   A context that binds a Worker outside the project (a `Service` instance)
   gets a local stand-in for it in its own `wrangler dev`, passed as a second
   `-c` config and written under `.bynk/dev/stand-ins/`. It runs as
   `bynk-stand-in-<context>-<worker>`, so it never answers another local
   session's binding to the real Worker, and the context's `wrangler.dev.toml`
   binds the stand-in in place of the real Worker. It
   answers every request with `503` and a body naming itself, and is listed on
   start.
   `--local-worker NAME` drops it, so the dev registry connects the binding to
   your own `wrangler dev` of that Worker. `-- --remote` runs no stand-ins, and
   a `-- -c`/`--config` while stand-ins run is an error.
   When `wrangler` resolves through a runner (`npx`, `pnpm dlx` or `bunx`) and
   several contexts are served, it is installed once (for npm,
   `npx --yes wrangler@4 --version`) before they start, so their first-run
   installs do not race in the runner's cache.
6. Watch the `.bynk` sources; a save rebuilds in place and the affected workers
   hot-reload. Any worker exiting stops the rest — a survivor's bindings would
   point at a context that is gone. So does a SIGINT or SIGTERM sent to `bynk
   dev` itself (on Linux and macOS): it stops every `wrangler` and the `workerd`
   processes they started before it exits, so a CI step or test harness that
   stops it with `kill` leaves no server holding a port. A second Ctrl-C while
   they are stopping exits at once.

**Exit code** — On a successful hand-off, `bynk dev` exits with the exit code of
the first `wrangler` to stop (a clean Ctrl-C stop is a `0`). Stopped by a
SIGTERM, it exits `143`. A pre-flight or build failure exits non-zero before
serving.

**Notes**

- `bynk dev` provisions nothing. For `bynk dev -- --remote`, a prior
  [`bynk deploy`](#bynk-deploy) must have recorded every KV namespace id the
  Worker binds (the bare `KV` and each derived instance) in `bynk.deploy.lock`.
- `wrangler` is resolved with the same provenance ordering as `doctor`
  (project-local `node_modules/.bin` → `PATH` → your package manager's runner:
  `npx`, `pnpm dlx` or `bunx`). A runner resolution is surfaced as a notice
  naming it — it downloads on first use.

### Lifecycle events

Under `--format json`, `bynk dev` writes one JSON object per line to stdout,
tagged by `event`. This is the stable scriptable surface; the VS Code extension
reads it to show what is serving. Ignore fields and events you don't know:
new ones may be added, and existing ones keep their meaning.

| `event` | When | Fields |
|---|---|---|
| `serving` | Every `wrangler dev` has been started. A worker may still be booting, so poll its `url` for readiness. | `contexts`: one `{ "name", "port", "url", "inspector_port" }` per served context. `name` is the worker-directory form `--context` accepts. `inspector_port` is `null` when none was allocated. |
| `rebuilding` | A source change was seen and a rebuild began. | — |
| `rebuild_ok` | The rebuild succeeded; the workers hot-reload. | — |
| `rebuild_failed` | The rebuild failed. Its diagnostics are on stderr, and the last good build keeps serving. | — |
| `stopped` | The session ended. | `code`: the exit code `bynk dev` returns. |
| `request` | A worker served a request. Only under `--trace-events`. | See [Request traces](#request-traces). |
| `log` | A worker printed a line. Only under `--log-events`. | `context`: the worker directory that printed it. `level`: `info`, `warn` or `error`. `text`: the line, without colour, with any stack frame already at its `.bynk` location. |

```json
{"event":"serving","contexts":[{"name":"orders","port":8787,"url":"http://localhost:8787","inspector_port":9229},{"name":"payment","port":8788,"url":"http://localhost:8788","inspector_port":9230}]}
{"event":"rebuilding"}
{"event":"rebuild_failed"}
{"event":"rebuilding"}
{"event":"rebuild_ok"}
{"event":"stopped","code":0}
```

A pre-flight or build failure before serving writes no events; the exit code
and stderr say what went wrong.

### Worker logs

`bynk dev` reads each `wrangler dev`'s output a line at a time, and rewrites a
stack frame that names generated TypeScript to the `.bynk` line it came from,
through the source map the compiler writes beside the module (`<module>.ts.map`).
A handler that throws prints

```text
Error: boom
    at http_GET (/work/shop/src/shop.bynk:12:3)
```

in place of a position in `.bynk/dev/workers/shop/handlers.ts`, and a terminal
or editor that links `path:line:col` opens the `.bynk` source. A line that is
not a frame, a module without a map, and a position the map does not cover are
left as wrangler printed them. The path is absolute, so it links from anywhere.

On a terminal, wrangler's output stays attached to it, untouched, so its colour
and interactive hotkeys keep working; pass `--rewrite-frames` to rewrite there
too. Everywhere else (an editor task, CI, a redirect, either stream not a
terminal) and under `--log-events`, the output passes through `bynk`, which
wrangler sees as a pipe and so turns its colour and hotkeys off. `bynk dev` asks
for colour back with `FORCE_COLOR=1` when it would colour its own stderr (see
[Colour](#colour)).

With `--format json --log-events`, each line is also a `log` event (see
[Lifecycle events](#lifecycle-events)), classified by wrangler's own tags: a
line with `[ERROR]`, `✘`, `Uncaught` or an `…Error:` headline is `error`,
`[WARNING]` or `▲` is `warn`, and the rest `info`. A frame takes the level of
the line it continues. The VS Code extension reads these for its
[Bynk Logs](/reference/tooling/vscode-bynk/) view.

### Request traces

`--trace-events` builds the workers with per-request recording (a dev build
only: `bynk deploy` and `bynk compile` never record) and writes one `request`
event for each request a worker serves, so an editor can show what a request
did. Draw it over the handler's sequence diagram with the `span` of each step.

```json
{"event":"request","context":"shop-orders","id":"5d5e…","parent":null,"route":"POST /orders","status":201,"duration_ms":12.4,"steps":[{"kind":"call","target":"shop.payment.authorise","span":{"file":"src/shop/orders.bynk","line":23,"col":14},"start_ms":1.0,"ms":3.1,"outcome":"ok","request":"9a41…"},{"kind":"agent","target":"Ledger.tally","span":{"file":"src/shop/orders.bynk","line":27,"col":22},"start_ms":1.2,"ms":1.2,"outcome":"ok"}],"truncated":false,"fault":null}
```

| Field | Meaning |
|---|---|
| `context` | The worker directory that served the request, as in `serving`. |
| `id` | This request's id. |
| `parent` | `null` for a request from outside. For a cross-context call from another worker, the `id` of the calling request: the caller's `call` step carries this request's `id` as its `request`, so a client can join the two events into one call tree. |
| `route` | `<METHOD> <path>`, the path as requested. |
| `status` | The response status. A fault answers `500`. |
| `duration_ms` | From the request arriving to the response being ready. |
| `steps` | What the request did, in the order the calls began (below). |
| `truncated` | `true` when more steps happened than are listed. At most 200 steps are kept. |
| `fault` | `null`, or `{ "message", "frames" }`: the error's first line, and its stack frames, with frames into generated code at their `.bynk` location. |

A step is `{ "kind", "target", "span", "start_ms", "ms", "outcome" }`:

- `kind` is `capability` (an op on a capability in `given`), `call` (a call to
  another context) or `agent` (a method call on an agent).
- `target` is what was called: `Db.query`, `shop.payment.authorise`,
  `Ledger.tally`.
- `span` is the `.bynk` call site, `{ "file", "line", "col" }` (project-relative
  path, 1-based line and column), or `null`.
- `start_ms` is when the call began, in milliseconds after the request began: the
  same clock as `duration_ms`, so `[start_ms, start_ms + ms)` is the step's place on
  the request's timeline. Two steps whose intervals overlap ran concurrently (a
  `foldEff`, calls made in parallel); the gap between one step's end and the next's
  start is the handler's own time. A client can draw a true timing diagram from it,
  with the critical path. The steps are listed in the order they began, so
  `start_ms` never decreases down the list, and a step that settles last still lists
  where it began. A trace from a runtime built before this field has no `start_ms`;
  draw those steps by duration alone.
- `ms` is how long the call took.
- `request` (on a `call` step only) is the `id` of the callee's own `request`
  event.
- `outcome` is `ok`, `fault` (it threw or was rejected), or `pending` (it had not
  settled when the response was ready).

Timings are what the runtime measured. Where the platform coarsens its timers (the
Workers runtime advances its clock at I/O, not while code runs), steps can share a
`start_ms` and a step can report `ms` of `0`; the trace does not invent precision, so
read equal offsets as "no measurable difference" and keep the list order. A callee's
steps are measured from the callee's own start, not the caller's: the clocks of two
Workers cannot be compared, so a client places the callee's timeline at the
`start_ms` of the caller's `call` step that names it.

A step carries **no argument values**: only kinds, targets, spans, timings and
outcomes. The record closes when the response is ready, so work that continues
afterwards (`waitUntil`, a streamed body) is not in it. A call to another
context is one step here; the callee's own request is a separate `request`
event, from its own worker, whose `parent` is the caller's `id`. Run every
context under `bynk dev --trace-events` to see both sides: a callee served
without it prints no event, and the caller's step still names the `request` it
sent.

`bynk dev` takes a worker's output line as a trace when it carries the
`__bynk_trace:` prefix followed by valid trace JSON, so a line of your own that
happens to do so is read as one.

The recorder finds the request's record through `AsyncLocalStorage`, so a traced
build adds the `nodejs_als` compatibility flag to each worker's `wrangler.toml`.
It does not add it when `[workers] compatibility_flags` already lists
`nodejs_als` or `nodejs_compat`. Recording only observes: a call returns and
throws what it would have.

### Readiness

`--ready-json FILE` is the readiness signal for a test runner or a CI script,
so neither has to loop on a warm-up request. Once every served context answers
a probe (a `HEAD /`; any HTTP status counts), `bynk dev` writes one JSON object
mapping each context's worker-directory name to its URL, and prints `ready` on
stderr:

```json
{
  "orders": "http://localhost:8787",
  "payment": "http://localhost:8788"
}
```

The file is written once, atomically (to a temporary file, then renamed), so a
reader that sees it can parse it. It is removed when `bynk dev` starts, so a file
from an earlier session is never read as ready, and again when the session
stops. If the file can't be written, `bynk dev` says so once and keeps
serving; the write isn't retried. Each context is probed until it first
answers. The probe is plain HTTP on `localhost`, so a context served over TLS
(`-- --local-protocol https`) or away from localhost (`-- --ip`) never answers
it: `bynk dev` warns at start when `--ready-json` is combined with either, and,
after 30 seconds without readiness, notes once which contexts haven't answered.
Readiness means each Worker answers a request. The dev-registry service
bindings between contexts connect shortly after start, so a cross-context call
in the first moments may still be refused.

### Inspecting agents

```text
bynk dev inspect agents [PATH] [--context NAME] [--class AGENT]
bynk dev inspect agent <CLASS> <KEY> [--path PATH] [--context NAME] [--field FIELD [--after CURSOR]] [--limit N]
```

While `bynk dev` runs, `bynk dev inspect` lists the agent instances the session
has seen and reads one's stored state. Both print JSON on stdout, and fail with
a message on stderr and a non-zero exit when there is no running session for
the project, or the instance is unknown.

A running session writes `.bynk/dev/session.json`, naming each served context
and its URL, and removes it when it stops; `inspect` reads it to find the
session. Each request is signed under the local call key as the caller
`bynk.dev`, as an internal call between contexts is, so a Worker answers only a
caller holding that key.

**Dev builds only.** `bynk dev` builds each Worker with the inspection in; a
build for `bynk deploy` has no registry and no `/_bynk/inspect/` route (it
answers `404`), and is byte-identical to a build without this feature. Nothing
is written by inspecting: the read does not migrate a record, run a handler or
commit.

**`inspect agents`** lists the instances. An instance is listed once a handler
has addressed it in this session; one that exists in local storage from an
earlier session appears when it is next addressed.

```json
{
  "agents": [
    { "context": "shop.orders", "class": "Cart", "key": "u1", "id": "29bacc06…" }
  ],
  "errors": []
}
```

- `key` is the key in its wire form: a string key as a string, any other key as
  its JSON. `inspect agent` matches a string key by its text and any other key by
  its JSON, so if one class holds both a string key `"7"` and a numeric key `7`,
  typing `7` reaches whichever is listed first.
- `id` is the Durable Object id.
- `errors` names a context that could not be reached, as
  `{ "context": …, "error": … }`; the others still answer.

**`inspect agent CLASS KEY`** reads one instance. A string key is given as it is,
any other key as JSON.

```json
{
  "context": "shop.orders",
  "class": "Cart",
  "key": "u1",
  "id": "29bacc06…",
  "exists": true,
  "readable": true,
  "version": 1,
  "current": 1,
  "layout": 2,
  "migrationPending": false,
  "cells": { "total": 4 },
  "collections": {
    "lines": {
      "kind": "map",
      "size": 2,
      "entries": [ { "key": "apple", "value": { "i": 0, "v": 2 } } ],
      "next": "m/lines/apple"
    }
  }
}
```

- `cells` holds the agent's `Cell` fields as stored, in wire form.
- `collections` has one entry per `Map`, `Set`, `Cache` and `Log` field.
  `entries` is one page (`--limit`, 50 by default and at most 500), in storage
  key order, and `next` is the cursor of the following page or `null`. `--field
  NAME --after CURSOR` reads that field's next page alone. A `Map` or `Cache`
  entry is `{ "key", "value" }` with the stored value (`i` is its insertion
  sequence; a `Cache` entry also carries its expiry), a `Set` element is
  `{ "key", "value" }`, and a `Log` entry is the stored `{ "t", "v" }`. `size`
  is the entry count, or `null` when none is recorded.
- `version` is the schema version of the stored record and `current` the build's
  (`null` when the project has no schema lock). `layout` is the storage layout.
  `migrationPending` is `true` when the record is behind the build and a
  migration step covers it: it is shown as stored, not migrated.
- `history` is the instance's recent transitions, oldest first, at most the last
  50, each `{ "handler", "old", "new", "at" }`: the handler that committed it, the
  state before and after, and the time in milliseconds. `old` and `new` hold the
  `cells` in wire form and each collection's `size` (`{ "cells": {…},
  "collections": { "lines": 2 } }`), not its entries; `old` is `null` for the
  instance's first recorded commit. A transition a handler or an invariant
  refuses is never committed, so it has no entry. A commit made by a migration
  has `handler: null`. The buffer also drops its oldest entries when it would pass
  about 96 KiB, so an instance with large cells keeps fewer than 50. `history` is `[]` for an
  instance nothing has committed to in this build.
- `exists` is `false`, and nothing else is returned, for an instance with no
  stored state yet.
- `readable` is `false` for a record this build cannot read, with `fault` naming
  the kind (`NewerLayout`, `NewerSchema` or `NoMigrationPath`) and no state.

---

## `bynk tail`

```text
bynk tail [PATH] [--context NAME] [--env NAME] [-- <wrangler args>]
```

Streams a deployed Worker's logs (`wrangler tail`) with stack frames rewritten
to `.bynk` locations, exactly as [`bynk dev`](#worker-logs) does for the local
server. The project is compiled into `.bynk/tail/` so the compiler's source
maps are available; nothing is deployed, provisioned or written to the lock.

| Argument | Default | Meaning |
|---|---|---|
| `--context NAME` | the only context | Which context's Worker to tail. Required when the project has several. |
| `--env NAME` | `default` | Which `bynk deploy --env` environment's Worker to tail. |
| `-- <wrangler args>` | — | Forwarded to `wrangler tail` (e.g. `-- --status error`). `--format pretty` is added unless you pass `--format`, because wrangler prints JSON when its output is piped. |

Wrangler's pretty output may print an uncaught exception without its stack; the
frames that can be mapped are then those in a handler's own `console.*` output.
A frame that names the deployed bundle rather than a module, or a module the
build has no map for, is printed as wrangler gave it. The frames are mapped
through the current checkout, so tail a Worker deployed from the same sources.

## `bynk deploy`

Provision the Cloudflare resources each context declares — KV namespaces, queues,
and Durable Object migrations — set its secrets, and deploy every context's
Worker, in Service-Binding dependency order. See [Deploy to
Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/)
for the workflow, and [Deploy from
CI](/book/guides/projects-build-and-deployment/deploy-from-ci/) to run it from
GitHub Actions.

```text
bynk deploy [PATH] [--context NAME | --worker NAME] [--dry-run] [--no-install] [--strict-environments] [--format short|json] [--yes]
            [--secrets-file PATH] [--secret NAME]... [--force] [-- <wrangler args>]
```

`bynk deploy` deploys one project, so it refuses to run at a
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces):
run it inside a member. Each member keeps its own deploy state.

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | `.` | A directory inside the project. The root is found by walking up for `bynk.toml`. |
| `--context NAME` | every context | Deploy this context alone, assuming the contexts it consumes are already live. A dependency that has never been deployed is named and refused rather than pushed into — as is one that is live but no longer provides the contract this context was compiled against, and a build of this context that no longer provides what a live caller was compiled against (`bynk.deploy.contract_skew`; see [Contract skew](/book/guides/projects-build-and-deployment/contract-skew/)). Accepts the dotted name (`a.b`) or its worker-directory form (`a-b`). |
| `--worker NAME` | every Worker | Deploy one Worker by its directory name under `workers/`: a [`[deploy.workers.<key>]`](/reference/tooling/manifest/#worker-groups) group's key, or a context's worker-directory form (`a-b`). A group is one Worker, so it deploys as a unit; naming one of its members with `--context` is refused, with the Worker that holds it. Like `--context`, it assumes the Workers it binds to are already live. Not with `--context`. |
| `--dry-run`, `--plan` | off | Print the per-context plan and resolved order, then exit without changing Cloudflare or `bynk.deploy.lock`. With `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` set it reads the account to check what it would create (see below); without them it works offline. |
| `--no-install` | off | Do not install the build's npm packages; stop, naming the detected install command, when one is missing. Also `BYNK_NO_INSTALL=1`. See [Installing npm packages](#installing-npm-packages). |
| `--strict-environments` | off | Refuse a deploy to a named `--env` that shares a value with `default`: an `env(…)` constant taking a `[vars]` or default value instead of the environment's own, or a `Service @worker` or `Email @to` / `@allowedRecipients` equal to `default`'s. The plan always lists each `env(…)` constant with its value and where it came from, and each shared value as `shared with default: …` (`shared_with_default` in `--format json`); setting the variable under `[env.<name>.vars]` records the sharing as deliberate. If `default` cannot be built, so the comparison cannot be made, the plan says so and this flag refuses. |
| `--format FORMAT` | `short` | Plan output: line-oriented `short` or machine-readable `json`. |
| `--yes` | off | Skip the confirmation required before a resource is created or a Worker is published. Required for non-interactive calls. |
| `--secrets-file PATH` | — | Read secret **names and values** from a dotenv-style `NAME=value` file. Never committed, never persisted: values move to `wrangler secret put` and are dropped. |
| `--secret NAME` | — | Set this named secret, taking its **value** from the environment (or a prompt). Repeatable. Needed only for a `bynk.Secrets` name — an actor's declared `auth` secret needs no flag. |
| `--force` | off | Overwrite a secret that is already set. The default sets only the missing ones, so a re-deploy does not cut a fresh Cloudflare secret version for every secret every time. |
| `--accept-compatible-skew` | off | With `--context`: deploy a callee ahead of live callers still on an older contract, when every such change is compatible for them. The older contracts are recorded in `bynk.contracts.lock` (commit it) and the callee is rebuilt to answer them. A breaking change, and a caller ahead of its live callee, are refused as before. See [Contract skew](/book/guides/projects-build-and-deployment/contract-skew/#deploying-a-callee-ahead-of-its-callers). |
| `--accept-fresh-agent CONTEXT` | none | Let a context that hosts agents start with fresh state on a different Worker script than it was last deployed to: into or out of a [`[deploy.workers.<key>]`](/reference/tooling/manifest/#worker-groups) group, between groups, or after a group is renamed or re-keyed. An agent's state belongs to its script, so without this such a move is refused (`bynk.deploy.agent_move_refused`) before anything is pushed. With it the old state stays in the old script, the run warns and names that script, and the plan then reports it as holding agent data so no one deletes it as empty. Repeatable; each context is named, never a blanket. A move out of a group that stays, and a group renamed in place (`name =` changed, key kept), cannot be accepted. |
| `-- <wrangler args>` | — | Everything after `--` is forwarded to `wrangler deploy` verbatim, for every context deployed. |

**Behaviour** — the command pre-flights Node and Wrangler, compiles into
`.bynk/deploy/`, reads what each context declares from its generated
`wrangler.toml`, **topologically sorts the Service-Binding graph so every binding
target is uploaded before the Worker that binds to it**, prints a per-context
plan carrying that order, checks Wrangler authentication, and then provisions,
materialises, sets secrets, and deploys each context in turn. The order is a
correctness requirement, not a nicety: Cloudflare resolves a Service Binding at
upload and rejects a Worker whose target does not yet exist. A `consumes` cycle
cannot arise — the compiler rejects one before emit.

A Service Binding to a Worker **outside the project** (a [`Service`
instance](/book/guides/effects-and-capabilities/call-a-foreign-worker/)) is
planned as `service <BINDING> <worker> (foreign Worker; not provisioned)` (in
JSON, the context's `foreign_services`). It is never created, ordered against or
environment-suffixed. After authenticating and before confirming, `deploy` asks
the account whether each such Worker exists (`wrangler deployments list --name`)
and **warns, never refuses**, when one does not: Cloudflare then rejects the
upload itself. `--dry-run` does not ask.

**The JSON plan.** With `--format json`, `deploy` writes one JSON document to stdout, so a
client reads why a run stopped from a field and never from the stderr line. Two arrays are
always present beside `environment`: `refusals`, why the run cannot go ahead (the exit
status is then non-zero), and `warnings`, what it says about itself but goes ahead with (a
refused run has none). A run refused at a gate in the table below stops before its plan is
derived and prints `environment`, `refusals`, `warnings` and the upload `order`; a run that
gets as far as the plan prints the whole plan (`orphans`, `order`, `contexts`, …) with the
arrays beside it. The human `bynk: …` line is still written to stderr.

**A refusal that is not in the table yet leaves no entry** (the account checks and confirmations of a run that applies, which `--plan` never reaches, still fall here): the run exits non-zero and writes
its reason to stderr alone, and stdout can be empty, or hold a plan whose `refusals` is `[]`
(a refusal made after the plan is printed). A client treats a non-zero exit with no entry in
`refusals` as a refusal it cannot explain, and reads stderr.

Each entry is `{"code", "message", …}`: a stable `code`, the readable `message`, the facts
of its subject beside them, and `override`, the flag that overrides a refusal, where one
does. A client matches on `code` and treats a code it does not know as an unexplained
refusal or warning, never as a pass.

| Code | In | Subject fields |
|---|---|---|
| `bynk.deploy.agent_move_refused` | `refusals` | `reason` (`not_accepted`, `renamed_in_place` or `old_script_stays`), `context`, `agents`, `from` and `to` (the scripts). `override` is `--accept-fresh-agent <context>` for `not_accepted`; the other two cannot be accepted. |
| `bynk.deploy.accept_fresh_agent_unused` | `refusals` | `name`: an `--accept-fresh-agent` name that matches no move of this run. |
| `bynk.deploy.agent_starts_fresh` | `warnings` | `context`, `agents`, `from`, `to`: an accepted move, whose old script keeps the data. |
| `bynk.deploy.env_passthrough_conflict` | `refusals` | `environment`, `flag`: a second `--env` or `--environment` after `--`. |
| `bynk.deploy.project_undeployable` | `refusals` | none: the `[deploy.workers]` table does not read, or the build profile checks contracts. |
| `bynk.deploy.preflight_failed` | `refusals` | `capabilities`: the doctor's own JSON, each capability with its `rows` (`label`, `level`, `detail`, `remedy`). |
| `bynk.deploy.context_not_found` | `refusals` | `requested`: the `--context` or `--worker` name no build produced. |
| `bynk.deploy.no_workers_built` | `refusals` | none: the build produced no Workers (the project defines no contexts). |
| `bynk.deploy.workers_unreadable` | `refusals` | none: a built Worker's declarations could not be read. |
| `bynk.deploy.email_claim_refused` | `refusals` | `environment`: `[deploy.email]` could not be applied. |
| `bynk.deploy.secrets_source_unreadable` | `refusals` | none: `--secrets-file` did not read. |
| `bynk.deploy.ledger_unreadable` | `refusals` | `path`: `bynk.deploy.lock` did not read. |
| `bynk.deploy.dependency_not_deployed` | `refusals` | `worker`, `dependencies`: a `--context` Worker binds to Workers never deployed. |
| `bynk.deploy.contracts_manifest_unreadable` | `refusals` | `worker`: its contracts manifest did not read. |
| `bynk.deploy.contracts_lock_unwritable` | `refusals` | `path`: the older contracts `--accept-compatible-skew` records could not be written. |
| `bynk.deploy.rebuild_failed` | `refusals` | `path`: the rebuild after recording them failed. |
| `bynk.deploy.upload_order_cycle` | `refusals` | `cycle`: the Workers that bind to each other in a cycle. |
| `bynk.deploy.contract_skew` | `refusals` | `side` (`caller`, `callee` or `orphaned_caller`), `worker`, and `skews`: each a `dependency`, `service`, `expected` and `live` hash for a `caller`, or a `caller`, `dependency`, `service`, `expected` and `provided` hash for the other sides. |
| `bynk.deploy.foreign_contract_skew` | `refusals` | `pins`: each a `context`, `service`, `actor`, the `pinned` hash and what was `built` (`hash`, `not_callable`, `undeclared` or `no_context`) with its `built_hash`. |
| `bynk.deploy.pins_unreadable` | `refusals` | `path`: `bynk.foreign.toml`, the foreign callers' contract pins, did not read. |
| `bynk.deploy.account_name_conflict` | `refusals` | none: two instances would take the same name on the account. |
| `bynk.deploy.secrets_file_passthrough` | `refusals` | none: a passthrough `--secrets-file` on a first deploy. |
| `bynk.deploy.strict_environments_shared` | `refusals` | `environment`, `shared`: the names it shares with `default`. |
| `bynk.deploy.strict_environments_unchecked` | `refusals` | `environment`: `default` could not be built, so nothing was compared. |
| `bynk.deploy.hyperdrive_settings_unreadable` | `refusals` | none: `[deploy.hyperdrive]` did not read. |
| `bynk.deploy.hyperdrive_unresolved` | `refusals` | none: a Hyperdrive binding did not resolve. |
| `bynk.deploy.adoption_unreadable` | `refusals` | `table` (`vectorize` or `containers`): its `bynk.toml` entries did not read. |
| `bynk.deploy.adoption_unmatched` | `refusals` | `table`, `capability`: an adoption entry that names no capability of the project. |
| `bynk.deploy.adoption_partial` | `warnings` | `table`, `capability`, `environment`: the entry adopts nothing for this environment, so the deploy creates or builds one. |
| `bynk.deploy.account_note` | `warnings` | none: what checking the plan against the account found, as the message says it. |
| `bynk.deploy.email_forward_destination` | `warnings` | none: a `Forward` destination Cloudflare may refuse or that could not be checked. |
| `bynk.deploy.recreate_needs_yes` | `refusals` | none: a changed `@jurisdiction` would make a new R2 bucket, which `--yes` accepts. |

**Secrets** — `deploy` sets three kinds, and the plan marks which is which:

- **`declared`** — an `actor`'s `auth` secret (`Bearer(secret = "…")`,
  `Signature(secret = "…")`). Supply only its value; you never name it. A
  declared secret with no value is a **hard error**, because deploying without it
  would answer every request `401`.
- **`read`** — a literal `bynk.Secrets` name (`Secrets.get("X")`). The compiler
  sees it, but `get` returns `Option`, so a missing value is a **warning**, not
  an error: the program already handles `None`.
- **`supplied`** — anything you name with `--secrets-file` or `--secret` that the
  compiler did not find itself.

A `Secrets.get` call with a **computed** name cannot be planned. The compiler
warns (`bynk.secrets.computed_name`), and the plan says so rather than
under-reporting. Neither can a read inside an adapter's TypeScript binding: a
Worker that composes an adapter provider `given Secrets` is marked incomplete
too. Literal names in `provides` bodies — including providers reached through
`given` — are collected like those in handlers. Either way the plan says so —
`secrets incomplete <worker>`, or `secrets_complete: false` on the context in
`--format json`. **A short `read` list with `secrets_complete:
false` is not a census** — see [the deploy
guide](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/).

Values are read from `--secrets-file` first, then the environment, then a prompt
when a terminal is attached; without one, a missing value is an error naming the
secret rather than a silent blank. The environment supplies **values only** — it
is never scanned for names. A secret value never reaches `bynk.deploy.lock`,
generated config, or the plan, in any format.

Per context, the plan carries one line per resource it declares:

| Plan line | Meaning |
|---|---|
| `kv create\|reuse <namespace>` | A KV namespace is created, or its recorded id reused. A derived instance's line ends with its binding (`kv create shop-cart-sessions (KV_SESSIONS)`). In `--format json` the context's `"kv"` is a list of `{"binding", "action", "namespace"}`, empty for a context with no KV. |
| `orphan kv <worker> [<binding>]` | A recorded KV namespace the build no longer declares: a removed context's, or a derived instance its context dropped or renamed. Deleted only by `--prune`, behind its own confirmation. In `--format json`, `"orphans": {"kv": [{"worker", "binding"?}]}`, with no `binding` for the bare namespace. |
| `queue create\|reuse <name>` | A queue is provisioned before the push, which `wrangler deploy` will not do for you. `reuse` forecasts that it already exists; existence is checked against Cloudflare at provision time regardless, so a queue deleted out-of-band is restored. |
| `durable object <Class> (<storage>; advisory — Cloudflare reconciles it)` | One line per Durable Object class (each agent, and a context's events fan-out) the push declares in the config's `exports` map. **Advisory**: Cloudflare compares the declared classes with the Worker's namespaces on every deploy and creates what's missing, and `bynk` keeps no record of them, so this states what the push will declare, never which namespaces already exist. In `--format json` it is `"durable_objects": [{"class": …, "storage": "sqlite", "reconciled_by": "Cloudflare"}]`. |
| `deploy\|redeploy <worker>` | The Worker is pushed; `redeploy` when the ledger has pushed it before. |

**Checked against the account.** The plan's `create` comes from
`bynk.deploy.lock`. When the plan would create something and
`CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` are set, `deploy` (and
`--dry-run`) also lists what the account already holds of those kinds. An R2
bucket, D1 database, Vectorize index or queue of the same name is planned as
`reuse`, since the deploy uses it rather than making another. A queue the
context only sends to becomes `create` or `reuse` instead of `create or reuse`.
A KV namespace of the same title is a warning: Cloudflare refuses a second
namespace of one title, so the deploy cannot create it. The token needs read
access to the kinds listed. Without the variables, the plan stays the ledger's
and a note says so. A listing that fails is a warning, never a failure.

**Credentials for account resources.** The push runs through wrangler, so
`wrangler login` is enough for it. Creating or deleting a KV namespace, and
creating an R2 bucket or a D1 database, goes through Cloudflare's API, as
Hyperdrive and the linked kinds already do. That needs `CLOUDFLARE_API_TOKEN` and
`CLOUDFLARE_ACCOUNT_ID`, the token with "Workers KV Storage: Edit", "Workers R2
Storage: Edit" or "D1: Edit" for the kinds it touches. When a step needs them,
`deploy` checks for both after the plan and before the confirmation, and
refuses, with nothing created, naming the missing variable and the permission.
A run that needs nothing asks nothing: a redeploy whose namespaces, buckets and
databases are all recorded works with `wrangler login` alone. D1 migrations
still run through wrangler.
In that case the check that a recorded namespace still exists on the account is
skipped, and the plan notes it.

Each context's Cloudflare ids and its deployed state are recorded in the
committed, secret-free `bynk.deploy.lock` at the project root, written as each
resource lands. A recorded id is reused on later deploys. CI refuses to create an
unrecorded namespace, the bare one or any derived instance's; provision it
locally and commit the lock file first. That
refusal covers KV alone — a queue is addressed by the name in your source, so CI
creating one strands nothing.

A run is **resumable, not transactional**: a failure stops the run, keeps and
records what already landed (there is no rollback), and names what did not. A
re-run re-pushes in the same order — it does not skip contexts already live, so
a changed context always ships; the plan reports those as `redeploy`.

**Exit code** — `0` on a successful plan or deploy, and on a clean Ctrl-C stop.
A Wrangler failure exits with **Wrangler's own exit code** — the first one to
fail, since the run stops there — so a CI job reads the same code it would from
`wrangler deploy` directly. The driver's own failures exit `1`: missing tools or
authentication, declined confirmation, compilation failures, an unrecorded CI
resource, a `--context` whose dependency was never deployed, and a `--context`
whose live dependency has drifted from the contract it was compiled against
(`bynk.deploy.contract_skew`) — deploying that would ship a caller its callee
rejects on every call — or whose new build no longer provides what a live
caller was compiled against (the same code, from the callee's side), and a build that breaks a declared foreign caller's pin
in `bynk.foreign.toml` (`bynk.deploy.foreign_contract_skew`; see
[TypeScript client](/reference/typescript-client/#pins)).

---

## `bynk check`

Type-check a `.bynk` file or project without writing compiled output. Runs the
compiler pipeline **in-process**.

A project with dependencies is resolved first, as for every command that
compiles one. The first `bynk check` creates `bynk.lock` (at the workspace root
for a workspace member) and prints `bynk: wrote bynk.lock` on stderr; a later
one writes it again only when the dependency graph changed, and otherwise
leaves the project tree as it was. A git dependency missing from the store is
fetched into it. With `--locked` it fails instead of writing the lock, as
`cargo check --locked` does for `Cargo.lock`, and with `--offline` it fails
instead of fetching.

```text
bynk check [INPUT] [--format rich|short|json|ndjson] [--fix[=CODE,…]] [--emit] [--each] [--env NAME] [--locked] [--offline]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | A `.bynk` file, or a project root directory (a `bynk.toml` or `src/` subdir selects project mode; otherwise the directory is itself the source tree). A directory with no `bynk.toml` or `src/` of its own, inside a project's `[paths] include` trees, is checked as that project, so `bynk check src` reads the project's manifest, as `bynk check` does; a `rich` run notes which project it checked. A directory outside those trees is checked as itself. `--fix` edits only the directory named. |
| `--format` | `rich` | `rich` is the source-context rendering; `short` emits one terse `path:line:col: severity[category]: message` line per diagnostic, for the VS Code problem-matcher, CI, and scripts. `json` and `ndjson` write the diagnostics as data on stdout: see [Machine-readable diagnostics](#machine-readable-diagnostics). In `rich` and `short`, `path` is the file as you'd type it from the working directory (the input you passed, joined with the file's place in it), the same path `fmt` reports. `rich` is coloured as [`--color`](#colour) says: by default only when stderr is a terminal and `NO_COLOR` is unset or empty; `FORCE_COLOR` or `CLICOLOR_FORCE` set to anything but empty or `0` colours it without a terminal (for `less -R` or a CI log); `0` doesn't turn colour off on a terminal, `NO_COLOR` does, and it wins over both. `rich` cuts a source line longer than 400 bytes to a window around its labels, marked `…`. |

| `--fix[=CODE,…]` | off | Apply the machine-applicable fix suggestions to the sources, in place, before checking. With `=CODE` (comma-separated, or repeated), only those diagnostic codes' fixes. See [Applying fixes](#applying-fixes). |
| `--emit` | off | After a passing check, generate the Workers TypeScript into a temporary directory and type-check it with `tsc`. See [Type-checking the generated code](#type-checking-the-generated-code). Needs a project directory, and `tsc` (with Node.js). |
| `--each` | off | Check each project directly inside a directory that has no `bynk.toml`. See [A directory of projects](#a-directory-of-projects). |
| `--env NAME` | `default` | The environment whose `[env.<name>.vars]` values the project's [`env(…)` constants](/reference/constants/#values-that-differ-per-environment) take: its own table, then `[vars]`, then each constant's default. |

**Exit code** — `0` when the input type-checks (warnings are surfaced but do not
fail the build, per the [diagnostics rule](#exit-codes-and-diagnostics));
non-zero on any error-severity diagnostic.

**At a workspace root** — `INPUT` naming a
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces)
checks every member, in path order. `rich` heads each member's report with a
`── packages/money ──` line; `short` adds no headers, and its paths are relative
to the working directory, so they stay unambiguous across members. Every member
is checked even when an earlier one fails, and the exit code is non-zero if any
member fails.

### Machine-readable diagnostics

`--format json` writes one JSON document to **stdout**, and `--format ndjson`
writes the same diagnostics one per line, followed by a summary line. Stderr
stays empty unless something other than a diagnostic went wrong, and the exit
status is the one `rich` and `short` give: non-zero on any error, `0` on warnings
and hints alone.

```json
{
  "version": 1,
  "diagnostics": [
    {
      "code": "bynk.given.undeclared_capability",
      "severity": "error",
      "file": "src/relay.bynk",
      "start": { "line": 19, "column": 5, "byteOffset": 231 },
      "end": { "line": 19, "column": 9, "byteOffset": 235 },
      "message": "capability `Beta` is used but not listed in the handler's `given` clause",
      "labels": [],
      "notes": ["add `Beta` to the handler's `given` clause so the dependency surface is visible at the declaration site"],
      "fixes": [
        {
          "message": "add `Beta` to the `given` clause",
          "applicability": "machine",
          "edits": [
            {
              "file": "src/relay.bynk",
              "start": { "line": 18, "column": 32, "byteOffset": 224 },
              "end": { "line": 18, "column": 32, "byteOffset": 224 },
              "replacement": " given Beta"
            }
          ]
        }
      ]
    }
  ],
  "summary": { "errors": 1, "warnings": 0, "hints": 0 }
}
```

| Field | Meaning |
|---|---|
| `version` | The schema version, `1`. A change that removes or reshapes a field bumps it; an added field does not, so read the fields you need and ignore the rest. |
| `diagnostics` | Every diagnostic, **hints included** (a terminal hides them), in file order and then position order. A project-level problem with no file sorts last. |
| `code` | The stable [diagnostic code](/reference/diagnostics/), such as `bynk.types.type_mismatch`. |
| `severity` | `error`, `warning` or `hint`. |
| `file` | The file as you'd type it from the working directory, with `/` on every platform, the same path `short` and `fmt` report. `null` for a diagnostic with no file. |
| `start`, `end` | Where the diagnostic points, or `null` with no file. `line` and `column` are 1-based; `column` counts **UTF-16 code units**, the unit the Language Server Protocol and JavaScript string indices use, so an astral character such as `🦀` is two. `byteOffset` is the 0-based byte position in the file, for a consumer that slices the bytes. |
| `message`, `notes` | The primary message and the further notes the `rich` rendering prints. |
| `labels` | Secondary spans, each with `message` and its own `file`, `start` and `end`: a label can lie in another file of the same unit (a duplicate's "previously declared here"). A label into a built-in unit names it by its bare identity path (such as `bynk.bynk`), which is not a file on disk. They are `null` when that file's text isn't available. |
| `fixes` | Suggested changes. Each has a `message`, an `applicability` and `edits`, each edit replacing the span `start`–`end` of its `file` with `replacement` (empty deletes; an empty span inserts). `machine` means the fix is exactly right in itself; `suggested` means a person should read it first, because it holds placeholder text or changes what the program means. `applicability` is the suggestion's own, not a promise that [`--fix`](#applying-fixes) takes it: `--fix` also skips a hint unless you name its code, and leaves alone a diagnostic that offers more than one candidate fix, where two alternatives would conflict. A tool that applies fixes itself should apply one per diagnostic. |
| `summary` | The count of `errors`, `warnings` and `hints`, which add up to the length of `diagnostics`. |

In `ndjson` each diagnostic line is the same object with `"type": "diagnostic"`
and `"version": 1` ahead of its fields, and the last line is
`{"type":"summary","version":1,"errors":…,"warnings":…,"hints":…}`. Every line is
meaningful on its own, so a consumer can stream.

At a [workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces)
the members' diagnostics come as one document, with paths relative to the
working directory and no member headers. `--emit` has no machine format (its
findings are `tsc`'s), so `--emit` with `json` or `ndjson` is refused. A run that
cannot start on an input (a `bynk.toml` that doesn't read, a file that doesn't
exist) writes its reason to stderr and **nothing** to stdout, so "nothing was
checked" is never mistaken for "nothing was found". `--fix` runs first, as it
does for every format, and the document reports what remains.

#### Read diagnostics in CI

Gate on the exit status, and read the document for the detail. With
[`jq`](https://jqlang.org/), list only the errors as `file:line:column: code`:

```sh
bynk check --format json > diagnostics.json || status=$?
jq -r '.diagnostics[] | select(.severity == "error")
       | "\(.file):\(.start.line):\(.start.column): \(.code)"' diagnostics.json
exit "${status:-0}"
```

Count the warnings a pull request leaves, without failing on them, with
`jq '.summary.warnings' diagnostics.json`; list each diagnostic that offers a
`machine` fix once with
`select(any(.fixes[]; .applicability == "machine"))`. An `ndjson` stream pipes
straight into `jq -c 'select(.type == "diagnostic")'`.

### A directory of projects

A directory with no `bynk.toml` at or above it, but projects below it (a
repository of independent examples, say), is not a project, so `bynk check`
refuses it and names them. `--each` runs the check over each project directly
inside it instead, exactly as at a workspace root whose `members` is `["*"]`:
a `── alpha ──` header per project, every project checked, and a non-zero exit
if any fails. It is opt-in, so a command typed in the wrong directory never
quietly runs a dozen projects.

- **Which projects.** The immediate subdirectories that hold a `bynk.toml`, in
  name order. A project inside a project is that project's own business, and
  `out`, `node_modules` and hidden directories are never projects. A
  subdirectory with `.bynk` files but no `bynk.toml` is skipped, with a note
  naming it; one with none is skipped quietly. If projects exist only deeper,
  `--each` names them and asks for the directory that holds them.
- **Where it applies.** Only to such a directory. At a workspace root it changes
  nothing, since the root already runs each member. At a project, inside one, or
  on a single file it is an error, so the flag never silently does nothing, and
  `.bynk` files directly in the directory are refused, as at a workspace root.
- **Each project stands alone.** It resolves its own `bynk.toml`, lock and
  dependencies, as when run by itself. Path dependencies *between* projects work
  as they do when each is run alone, but a dependency's own path dependencies
  are honoured only inside a real workspace: add the root manifest for that.
- **`--fix` and `--emit`** work across the projects, as across workspace members.

`bynk test --each` does the same for [`bynk test`](#bynk-test), and `bynk fmt`
needs no flag, since it already formats a directory of projects. `build`,
`compile`, `deploy` and the other commands that act on one project do not take
`--each`.

### Type-checking the generated code

A program that passes `bynk check` should always produce TypeScript that `tsc`
accepts. When it does not, that is a bug in the compiler, and `bynk check --emit`
finds it without running any tests: it generates the Workers output into a
temporary directory (nothing is written to your project), runs `tsc --noEmit`
over it, and reports a failure as `bynk.codegen.tsc_failed` at the Bynk source
line the failing generated line came from, with the TypeScript error and the
generated line attached. The exit code is non-zero.

```sh
bynk check --emit
```

It is off by default because it is far slower than a check and needs `tsc` (with
Node.js) on your path; without `tsc` it fails with install guidance rather than
passing silently. The editor's checks never run it. `bynk test` reports the same
diagnostic when its own `tsc` step fails. Please
[report](https://github.com/bynk-lang/bynk/issues) any `bynk.codegen.tsc_failed`
with its output.

### Applying fixes

`--fix` writes the compiler's own fix suggestions back to your files, then
reports what is left as an ordinary `check` does. It is the command-line form of
the editor's [fix all](/reference/tooling/bynk-lsp/#on-save) (`source.fixAll.bynk`),
and takes exactly the fixes that action takes: a fix from a diagnostic that
offers exactly one machine-applicable fix, never a choice between candidates and
never one with placeholders for you to fill in.

```sh
bynk check --fix
bynk check --fix --format short
bynk check --fix src/shop.bynk
bynk check --fix=bynk.given.unused_capability
```

- **`--fix=<code>` takes one migration at a time.** `--fix=bynk.types.inverted_range`
  takes only the fixes of that code, and `--fix=<a>,<b>` (or `--fix=<a> --fix=<b>`)
  of several; the value needs the `=`. Use it where an upgrade note names a code.
  Naming a code asks for its fixes, so a hint `check` hides (such as
  `bynk.parse.let_else_staircase`) is fixed when you name it, and plain `--fix`
  still leaves hints alone. An editor-only quick fix, one that changes meaning
  you must choose, is never taken. A code that is not a real diagnostic code is
  refused before anything is read, naming the nearest real one; a real code with
  nothing to fix writes nothing. The rest of the rules below apply unchanged.
- **It repeats until nothing more applies.** A fix can expose another fixable
  diagnostic (adding a capability to an agent handler's `given` makes a provider
  that calls it short of the same capability), so each round re-checks the
  edited text and takes what is now fixable, up to ten rounds.
- **It writes nothing unless the whole run is sound.** Every round runs in
  memory and the files are written once it settles. If a round would introduce a
  diagnostic that has no single machine-applicable fix, or the fixes are still
  being applied after ten rounds (two fixes undoing each other), the run names
  the problem, writes nothing, and exits non-zero. The report of the diagnostics
  that remain follows as usual.
- **It writes only files that change, in place, and does not format.** A file
  with nothing to fix keeps its modification time. Run [`bynk fmt`](#bynk-fmt)
  afterwards if you want both. One line per file written precedes the report:
  `fixed: src/shop.bynk: bynk.given.unused_capability ×3`, naming each code
  and how many of its diagnostics were fixed.
- **A directory you cannot write to** is only noticed when its file is written, so a
  failure there can come after earlier files were written.
- **There is no dry run.** Review with `git diff` and undo with `git checkout`.
- **At a workspace root** every member is planned first, in path order, and
  files are written only if every member's plan is accepted. One refused member
  writes nothing anywhere and fails the run. Members are planned independently,
  so they do not see each other's fixes: if a fix in one member changes
  something another depends on, the second member can be left with a new problem
  that `check` then reports, and `git checkout` undoes the run.

The exit code is that of the check that follows: success only if no error
remains.

---

## `bynk build`

Build a project into its output directory, taking its settings from
[`[build]`](/reference/tooling/manifest/#build) in `bynk.toml`. `bynk build` on its own
equals `bynk compile . -o out` for a project with no `[build]` table.

```text
bynk build [INPUT] [--out DIR] [--target bundle|workers] [--platform cloudflare|node|browser] [--emit ts|js] [--profile dev|release] [--env NAME] [--locked] [--offline]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | A project directory. A single `.bynk` file is `bynk compile`'s. |
| `--out`, `-o` | `[build] out`, else `out` | The output directory. |
| `--target` | `[build] target`, else `bundle` | `bundle` builds one deployment unit; `workers` builds one Worker per context. |
| `--platform` | `[build] platform`, else `cloudflare` | The surface binding. |
| `--emit` | `[build] emit`, else `ts` | `ts` or `js`. |
| `--profile` | `release` | `release` builds without contract checks, the same bytes `compile` writes; `dev` builds with them. [`[profile.<name>]`](/reference/tooling/manifest/#profilename) overrides a profile's `contracts`. |
| `--env NAME` | `default` | The environment whose `[env.<name>.vars]` values the project's [`env(…)` constants](/reference/constants/#values-that-differ-per-environment) take: its own table, then `[vars]`, then each constant's default. |

Each setting is the flag, else the `[build]` key, else the default. On success
`build` prints one line, `bynk: built <target> -> <out>` (a build that is not the plain release one adds its profile: `built bundle (dev profile, contracts on) -> out`); warnings print as
`compile` prints them.

`build` infers from the project; [`compile`](#bynk-compile) is told. `compile`
never reads `[build]`, which keeps the editor's *Show Compiled Output* view from
changing with the manifest. `bynk dev`, `bynk deploy` and `bynk test` read only
`package_manager` from it. `bynk test` reads `[profile.dev]` and `bynk deploy` checks `[profile.release]`
(see [`[profile.<name>]`](/reference/tooling/manifest/#profilename)). See [build progress](#build-progress) for the
per-context lines `build` prints on a terminal.

---

## `bynk compile`

Compile a project to TypeScript in a directory, **in-process**. It also serves the editor's **Show
Compiled Output** view; for a build you deploy, use [`bynk dev`](#bynk-dev) or
[`bynk deploy`](#bynk-deploy).

```text
bynk compile [INPUT] --output DIR [--target bundle|workers] [--platform cloudflare|node|browser] [--emit ts|js] [--sources-stdin] [--env NAME] [--locked] [--offline]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | A project root directory, or one `.bynk` file. |
| `--output`, `-o` | | The output root (the output file, for a single-file input). Each module is written under it with its source map beside it (`greet.ts` and `greet.ts.map`). |
| `--target` | `bundle` | `bundle` builds one deployment unit; `workers` builds one Worker per context. |
| `--platform` | `cloudflare` | The surface binding: `cloudflare`, `node`, or `browser` (`bundle` only). |
| `--emit` | `ts` | `ts` writes typed TypeScript; `js` writes the same modules with types stripped. |
| `--sources-stdin` | off | Compile unsaved buffers: read a JSON object `{ "<absolute path>": "<text>" }` from stdin and layer it over the project's sources, so the overlay wins over the files on disk. A `.bynk` path inside the project that the build did not read is added, as an unsaved new file; a path outside the project, or one that is not a `.bynk` file, is an error and nothing is built. |
| `--env NAME` | `default` | The environment whose `[env.<name>.vars]` values the project's [`env(…)` constants](/reference/constants/#values-that-differ-per-environment) take: its own table, then `[vars]`, then each constant's default. |

`--sources-stdin` makes the run a **preview build**: `bynk.schema.lock` is
neither read nor written, so nothing changes outside `--output`. Only `.bynk`
sources can be overlaid; a `bynk.toml` edit is read from disk.

---

## `bynk client`

Write a typed TypeScript client for the project's `from http` services, as one
module a front end imports (experimental, #3508): one function per route, the
boundary types as TypeScript types (an optional value is `T | null`), the server's
refinement checks on the way out and on the way in, and a union of each route's own
responses keyed by `status`. The module inlines a small browser-safe runtime. The
surface is in the [HTTP client reference](/reference/http-client/), and a
recipe in [Generate a typed client for your front end](/reference/tooling/bynk-client/).

```text
bynk client [INPUT] [--service NAME]... [--out PATH] [--layout file|dir] [--check] [--locked] [--offline]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | A project directory, or any path inside one. |
| `--service` | every service | Generate only this `from http` service, as `Name` or `unit.Name`. May be given more than once. |
| `--out`, `-o` | stdout | Write the client to this path instead of stdout: a file, or with `--layout dir` a directory. |
| `--layout` | `file` | `file` writes one module; `dir` writes `index.ts`, `types.ts`, `codecs.ts` and `runtime.ts` into `--out`. |
| `--check` | off | Write nothing. Compare the client at `--out` with the one the project generates, and exit non-zero, naming the first differing line, when it is stale or missing. The CI drift gate. |

A flag left out takes its value from [`[build.client]`](/reference/tooling/manifest/#buildclient)
in `bynk.toml`, when the project has one, so `bynk client --check` needs no
arguments there. `bynk build` writes the client that table names.

The project must check clean; otherwise the command lists the errors and writes
nothing. It exits non-zero when the project has no `from http` service, when
`--service` names none, when `--check` or `--layout dir` has no path to work on,
when two routes of a service would share a function name,
and when every selected route is one this version cannot represent. A route it
cannot represent (a `@raw` body, a raw `query` or `cookies` view, a raw or streamed
response, a request-signature route, an `Option` inside a `Map`, `Set` or `Result`) is
named on stderr and left out.

---

## `bynk openapi`

Write an [OpenAPI 3.1](https://spec.openapis.org/oas/v3.1.0) document for the
project's `from http` services: paths and methods from the routes, path
parameters and the request body from the handler's params, and responses keyed
by the status codes the handler can return. Schemas come from the same boundary
derivation the emitted codecs use, so a refinement becomes the JSON Schema
constraint it re-validates. The mapping and a recipe are in
[Publish your service's OpenAPI](/reference/tooling/bynk-openapi/).

```text
bynk openapi [INPUT] [--service NAME] [--out FILE] [--format json|yaml] [--locked] [--offline]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | A project directory, or any path inside one. |
| `--service` | every service | Document only this `from http` service, as `Name` or `unit.Name`. |
| `--out`, `-o` | stdout | Write the document to this file instead of stdout. |
| `--format` | `json` | `json` or `yaml`. Defaults to `yaml` when `--out` ends in `.yaml` or `.yml`. |

The project must check clean; otherwise the command lists the errors and writes
nothing. It exits non-zero when the project has no `from http` service, when
`--service` names none, and when two selected services answer the same method and
path.

---

## `bynk token`

Mint a credential for a `Bearer` actor your project declares, to call a guarded
route by hand — `curl` under [`bynk dev`](#bynk-dev), or a smoke test against a
deployment. The actor declares everything a minter needs (HS256, the secret's
name, `sub` as the identity), so you never write a signing script of your own and
the token cannot disagree with the actor's seam.

```text
bynk token [ACTOR] [--sub ID] [--lifetime DURATION] [--expired] [--not-before DURATION] [--claim NAME[=VALUE]]... [--input PATH] [--locked] [--offline]
```

| Argument | Default | Meaning |
|---|---|---|
| `ACTOR` | — | The actor to mint for, as `Name` or `unit.Name`. Omit it to list the project's `Bearer` actors and the secret each reads. |
| `--sub` | required | The identity the token names (its `sub` claim). It is checked against the actor's identity type, refinements included, before anything is signed. |
| `--lifetime` | `1h` | How long the token is valid: a whole number and a unit (`s`, `m`, `h`, `d`), at most `365d`. |
| `--expired` | off | Mint a token that expired an hour ago, to test that the seam refuses it. Not with `--lifetime` or `--not-before`. |
| `--not-before` | — | Make the token valid only from this far in the future (`10m`), to test that the seam refuses it as not yet valid. The lifetime counts from that moment. |
| `--claim` | — | Add a claim: `name` signs `true`, `name=value` signs the string. Repeatable; works for any actor. Not `sub`, `iat`, `exp`, `nbf`, `iss` or `aud`, which `bynk token` signs itself. On a refinement actor it replaces the claim the predicate would have set. |
| `--input` | `.` | A project directory, or any path inside one. |

```console
$ API_TOKEN_SECRET=… bynk token Reader --sub me
eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.…
$ curl -H "Authorization: Bearer $(bynk token Reader --sub me)" http://localhost:8787/me
```

Only the token goes to stdout, on one line, so it composes in `$(…)`. Errors go
to stderr and the exit status is non-zero.

**Not `bynk org token mint`.** That mints a registry *publish* token for
[`bynk publish`](#bynk-publish), from your account. `bynk token` mints a JWT for
an `actor` *this project* declares, and talks to no registry.

**The secret** comes from the environment, under the name the actor declares —
`auth = Bearer(secret = "API_TOKEN_SECRET")` reads `$API_TOKEN_SECRET`. There is
no flag for it: an argument would show in a process listing and in shell
history. A variable that is unset or empty is an error that names it. The secret
is never printed.

**Which actors.** `Bearer` only. A `Signature` actor authenticates a request by
an HMAC over that request's body, so there is no standalone credential to mint;
an `Oidc` actor's tokens are minted by the identity provider, which holds the
private key. Both are refused with that explanation.

**Refinement actors.** `bynk token Moderator --sub ann`, for
`actor Moderator = Reader where hasClaim("mod")`, signs with the base actor's
secret and carries the claims the predicate needs — the same ones the
[`system` test harness](/reference/testing/) signs for
`by Moderator("ann")`. A predicate no claims can satisfy
(`hasClaim("a") && !hasClaim("a")`) is refused. To see the seam answer `403`,
mint with the base actor, or override the claim:
`bynk token Moderator --sub ann --claim mod=`.

**The token** is an HS256 JWT with the claims `sub`, `exp`, under
`--not-before` `nbf`, and any `--claim`s — the same header, claims and unpadded base64url the
emitted verifier reads. The project must check clean.

---

## `bynk fmt`

Format `.bynk` source files in place, **in-process**.

```text
bynk fmt [INPUTS]... [--check] [--indent tab|spaces] [--indent-width N] [--max-line-width COLUMNS] [--trailing-comma|--no-trailing-comma] [--no-config]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUTS` | the enclosing project | Files or directories to format. With none, `bynk fmt` formats the project that holds the working directory: the same files [`bynk check`](#bynk-check) reads for it, so a project's `[paths] include` trees minus `exclude` (a workspace root formats every member), however deep in the project you run it. Outside a project there is nothing to default to, so it stops with `no input files`. A directory formats the `.bynk` files [`bynk check`](#bynk-check) reads for it: a project root's `[paths] include` trees minus `exclude`, or any other directory walked recursively (hidden directories skipped). Pass `-` to read from stdin and write the formatted result to stdout. |
| `--check` | off | Report files that are not already canonically formatted **without writing changes**. Exits non-zero if any file would change. For CI. |
| `--indent` | `[fmt] indent`, else `tab` | Indent with tabs or spaces. Tabs are the default so each reader sets their own width in their editor. |
| `--indent-width N` | `[fmt] indent_width`, else `2` | Spaces per nesting level. Rejected when the run resolves to tabs, where it would have no effect. |
| `--max-line-width COLUMNS` | `[fmt] max_line_width`, else `100` | Soft target line width. A construct wider than this wraps across lines where the grammar allows; one with no break point in it (a long string literal) is left long. |
| `--no-trailing-comma` | off | Omit the trailing comma in multi-line records, sums, list literals and `exports` clauses. `--trailing-comma` is its opposite (and overrides a project's `trailing_comma = false`); the later of the two wins. |
| `--no-config` | off | Ignore the project's `bynk.toml` `[fmt]` section and format to the canonical style plus whatever flags this run passes. |

**Behaviour** — `bynk fmt` and `bynk fmt --check` with no paths cover exactly what `bynk check` compiles, so a CI gate needs no path list to keep up with `[paths]`. Explicit paths, and `-` for stdin, behave as they always did and never fall back to the project. Standard input is read only with `-`: piped text and no paths formats the project and leaves the pipe unread. The project is the nearest `bynk.toml` above the working directory, so inside a workspace member that means the member alone; run `bynk fmt` at the workspace root to format every member. Each file is formatted and rewritten only when it changes; a file
already canonical is left untouched. A file that does not parse is reported and
skipped; the other inputs are still processed. A file named more than once,
directly or through a directory, is formatted once; a directory holding no
`.bynk` file is an error, so a mistyped path cannot pass `--check`. A
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces)
formats every member's files, named relative to the working directory.

Line endings aren't a formatting difference. A file with CRLF line endings (a
Windows checkout with `core.autocrlf=true`) whose LF form is canonical passes
`--check` and is left as it is; a file that does need formatting is written
with LF line endings throughout. [`bynk new`](#bynk-new)'s `.gitattributes`
keeps `.bynk` files LF on every platform.

The formatter never deletes your text. When a comment or a `---` documentation
block has nowhere to go in the formatted output, the file is left unchanged and
reported with `bynk.fmt.comment_loss`. A `---` block attaches to nothing when a
blank line separates it from the next declaration, or when what follows carries
no doc (a `uses` or other clause, a service policy, or anything inside a
policy): `bynk check` warns `bynk.parse.orphan_doc_block`, and the formatter
keeps the block where it is, so formatting never attaches it. Remove the blank
line to attach it, or make it a `//` comment. A `//` comment on the same line as
an opening `{` moves onto its own line under the brace, and one on the `=` line
of a sum type moves onto its own line above the first variant. A comment
between `private` and the keyword it modifies (`private // why` above `fn`) has
nowhere to go, so the file is refused; put the comment above `private` instead.

**Where the style comes from** — three layers, each overriding the one before:
the canonical defaults, then the project's `[fmt]` section in
[`bynk.toml`](/reference/tooling/manifest/), then the flags this run passes. The manifest is
found by walking up from each file, so one command may format files from two
projects and give each its own style. `--check` judges files against those
resolved options, so a project on a non-default style has a CI gate that can
pass. `--no-config` skips the manifest layer for a run that wants the canonical
style whatever project it is pointed at; nothing here ever writes to
`bynk.toml`.

Format-on-save in the editor reads the same `[fmt]` section through the same
reader, so the two agree by construction.

**Exit code** — `0` when every input was formatted (or, under `--check`, already
canonical). Non-zero if a file could not be read/written, failed to parse, or
(under `--check`) was not canonical.

---

## `bynk test`

Discover and run a project's test declarations, **in-process**. It still starts the external `tsc` and
`node` for the runner (and `bynk dev` for `--against dev`). Requires `tsc` (with
Node.js) or `tsx` on `PATH`.

```text
bynk test [INPUT] [--each] [--env NAME] [-o OUTPUT] [--no-run] [--require-tests] [--format rich|json|ndjson] [--inspect] [--seed HEX] [--case NAME]... [--suite NAME]... [--coverage] [--against TARGET [--skip-unsupported] [--settle DURATION]] [--smoke [--print] [--smoke-unsafe]] [--no-install] [--update-snapshots] [--mutate [--only DECL] [--max-mutants N] [--mutate-timeout SECONDS] [--mutate-all-suites] [--mutate-fail-under PERCENT] [--changed GIT_REF] [--mutate-jobs N]]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | Project root directory. |
| `-o, --output OUTPUT` | `<input>/out` | Where to write the compiled TypeScript test-runner modules. |
| `--no-run` | off | Skip the runner. With `--format rich`, emit the generated test files; with `--format json`, emit a discovery document listing every suite and case without running them. |
| `--each` | off | Test each project directly inside a directory that has no `bynk.toml`. See [A directory of projects](#a-directory-of-projects). |
| `--env NAME` | `default` | The environment whose `[env.<name>.vars]` values the project's [`env(…)` constants](/reference/constants/#values-that-differ-per-environment) take, as for `bynk build`. A suite's `env { … }` is run-time and unaffected. Refused with `--mutate` and `--against`, which would ignore it. |
| `--require-tests` | off | Fail when the project declares no tests. Without it, `bynk test` prints `warning: no tests found in <dir>` on stderr and exits 0; with it that is an `error:` and exit 1 (`error.kind == "no_tests"` under `--format json`). Applies with `--no-run`. At a workspace root, each member must declare a test. |
| `--format` | `rich` | `rich` is the grouped ✓ / ✗ human output; `json` is a single pinned JSON document of results, for tooling; `ndjson` is the same results as a stream of one JSON event per line, written as each suite finishes (see [Streaming results](#streaming-results)). `ndjson` is refused with `--coverage` and `--no-run`, and across a workspace. |
| `--inspect` | off | Launch the runner under Node's inspector (`node --inspect-brk`) and print the inspector URL. Requires Node ≥ 22.18 (or ≥ 23.6 unflagged). Does not run `tsc`. |
| `--seed HEX` | random | Root seed for generative `property` tests (e.g. `0x5f3a`). A failing property prints the seed it used; re-running with `--seed <hex>` reproduces the run byte-for-byte. |
| `--case NAME` | — | Run only the test cases named `NAME`, across suites — the filter behind the editor's per-case *▷ Run Test* lens. Repeat the flag to run several cases in one process. See [Selecting tests](#selecting-tests). No effect with `--no-run`. |
| `--suite NAME` | — | Run only the cases of the suite `NAME`, as the JSON document reports it (an integration suite without its prefix). Repeat the flag to select several suites. See [Selecting tests](#selecting-tests). No effect with `--no-run`. |
| `--coverage` | off | After the run, report statement/line coverage attributed to `.bynk` source (rich table, or a `coverage` block under `--format json`). Requires the `tsc → node` path — incompatible with `--inspect` and `--no-run`. |
| `--against TARGET` | — | Run the `system` suites against a real runtime instead of in process: `dev`, a local `bynk dev` the runner starts itself, or a target named under [`[test.targets]`](/reference/tooling/manifest/#test) in `bynk.toml`. See [Running against a deployment](#running-against-a-deployment). Incompatible with `--inspect` and `--coverage`. |
| `--skip-unsupported` | off | With `--against`, skip each `system` case a real runtime can't honour, naming it and why, instead of refusing the run. |
| `--settle DURATION` | `5s` | With `--against`, how long a failing `expect` is retried before it fails, replaying the reads before it (see [Waiting for a real system](#waiting-for-a-real-system)). `500ms`, `5s`, `2m`, or `0` for no retry. |
| `--smoke` | off | Run a smoke test of every `from http` route instead of the project's own suites. See [Smoke-testing every route](#smoke-testing-every-route). Combine with `--against` to smoke a running system. |
| `--print` | off | With `--smoke`, print the synthesised suites as `.bynk` source and run nothing. |
| `--no-install` | off | Do not install the build's npm packages; stop, naming the detected install command and `out`, when one is missing. Also `BYNK_NO_INSTALL=1`. See [Installing npm packages](#installing-npm-packages). |
| `--smoke-unsafe` | off | With `--smoke --against <target>`, also call the routes that write (`POST`, `PUT`, `PATCH`, `DELETE`). |
| `--update-snapshots` | off | Record snapshots instead of comparing: write each `expect … matches snapshot` that has no stored snapshot, refresh the ones whose value changed, and delete the obsolete ones, in `snapshots/<suite file>.snap.json`. Without it a missing snapshot fails the case, a changed one fails with a diff, and an obsolete one is only reported. See [Snapshots](/reference/testing/#snapshots). Incompatible with `--against`, `--smoke`, `--coverage`, `--inspect` and `--no-run`. |
| `--mutate` | off | Check that the suites would notice a small change to the code: apply one typed mutation at a time to `src/`, run the suites that exercise the mutated unit, and report the mutants no suite failed on. See [Mutation testing](#mutation-testing). Incompatible with `--inspect`, `--no-run`, `--coverage`, `--against`, `--smoke`, `--update-snapshots`, `--case`, `--env` and `--format ndjson`. |
| `--only DECL` | — | With `--mutate`, mutate only `unit.decl` (`orders.refund`), or every declaration of `unit`. |
| `--max-mutants N` | — | With `--mutate`, stop after `N` mutants, in source order. |
| `--mutate-timeout SECONDS` | 3× the unmutated run, at least 10 | With `--mutate`, kill a mutant's run after this long; it counts as killed. |
| `--mutate-all-suites` | off | With `--mutate`, run every suite against every mutant, not only the suites that reach the mutated unit. |
| `--mutate-fail-under PERCENT` | — | With `--mutate`, exit non-zero when fewer than `PERCENT` of the mutants that ran were killed, or when no mutant reached a suite. |
| `--changed GIT_REF` | — | With `--mutate`, mutate only declarations in `.bynk` files that differ from `GIT_REF` (a ref, not an option; what `git diff GIT_REF` names, plus files git doesn't track yet). An unknown ref is an error. When nothing changed, `--mutate-fail-under` passes. |
| `--mutate-jobs N` | 1 | With `--mutate`, run `N` mutants at once (1–256), each in its own scratch output directory. The report doesn't depend on it. |

**Exit code** — follows the runner's own process status: `0` when every case
passed, non-zero on a failing case, a `--case` or `--suite` value that selected
nothing, a compile error, or a missing runner.

**At a workspace root** — `INPUT` naming a
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces)
tests every member in path order, printing a `── packages/money ──` line before
each member's output. Every member runs, and the exit code is non-zero if any
fails. `--format json`, `--output` and `--against` each belong to one project,
so they are refused at a root: run them inside a member.

**A directory of projects** — `--each` runs the same loop over each project
directly inside a directory that has no `bynk.toml` (see
[A directory of projects](#a-directory-of-projects)): a header per project,
every project run, non-zero if any fails, and the same flags refused as at a
workspace root. `--suite` and `--case` are judged across all the projects.

### Selecting tests

`--case` and `--suite` choose which tests one run executes, and each may be
repeated:

```text
bynk test --case "accepts positive" --case "rejects zero"   # those cases, in any suite
bynk test --suite commerce.money                             # every case in that suite
bynk test --suite commerce.money --case "accepts positive"   # that case, only in that suite
```

- Values of the same flag combine as **or**; the two flags combine as **and**. A
  case runs when its suite is among the `--suite` values (if any are given) and
  its name is among the `--case` values (if any are given). A pair of flags
  cannot name an arbitrary set of `suite`/`case` pairs: `--suite A --suite B
  --case x --case y` includes `A`'s `y` and `B`'s `x`.
- A case name is matched exactly and may contain commas, so the flag is repeated
  rather than comma-separated. An empty value is ignored, as is a repeated one.
- The whole selection runs in **one process**: one seed, one coverage report. A
  suite stays one record in the output, holding its selected cases in the order
  the flags named them. A suite that `--suite` leaves out is omitted entirely.
- **A value that selects nothing fails the run.** Each value is judged on its
  own, so `--case a --case typo` fails although `a` matches. The run prints its
  usual output first, then names each unmatched value on stderr
  (`bynk test: no case matched --case "typo"`, `no suite named --suite "x"`) and
  exits non-zero. Before this, a filter that matched nothing passed with
  `0 passed, 0 failed`.
- `--inspect`, `--coverage` and `--against` (`--against dev` included) honour the
  selection. `--no-run` discovery lists every case regardless. Under
  `--against … --skip-unsupported`, naming a skipped case is not an error; it
  stays listed as skipped, and a skipped case the selection leaves out is not
  listed.
- At a [workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces)
  a value may live in one member only: each value is judged against every member's
  suites together, and a member that holds none of the selected tests runs nothing.

### Mutation testing

Passing suites show the code does what they ask, not that they would notice if it
stopped. `bynk test --mutate` checks that: it changes the code in `src/` one small
way at a time, runs the suites against each change, and reports the changes no
suite failed on.

```text
bynk test --mutate                        # every mutant in src/
bynk test --mutate --only orders.refund   # one declaration
bynk test --mutate --format json          # the run's document, plus a `mutation` block
```

```text
mutation: 3 mutants, 1 killed, 1 survived, 1 not covered by any suite, 0 invalid (score 50%)
  SURVIVED  src/demo/limits.bynk:4:5  demo.limits.within  `<` → `<=`
  NO SUITE  src/demo/lonely.bynk:4:5  demo.lonely.same  `==` → `!=`
```

- **The mutations.** A comparison operator swapped for its neighbour (`<`↔`<=`,
  `>`↔`>=`, `==`↔`!=`) and a boolean connective exchanged (`&&`↔`||`), in every
  declaration of the project's non-suite files. Operators in comments and string
  literals are left alone. An `if` condition, and a `match` arm's `if` guard, is
  replaced by `true` and by `false` (a mutant is skipped when the condition
  already is that literal), and a binary `+` becomes `-` and `-` becomes `+`
  (negation is left alone). A mutant that changes the types, such as `+` on
  strings, doesn't compile and is `invalid`.
  Inside the SQL of a `D1` call (the argument of `all`, `first`, `run`,
  `select` or `firstAs`, an inline `batch` statement, or a `String` `const`
  bound to one, whatever its case), `<`↔`<=` and `>`↔`>=`, `max(`↔`min(`, and
  dropping one `AND` conjunct of a `WHERE` with two or more (a `WHERE`
  containing an `OR` is left alone, `BETWEEN … AND …` is one conjunct, and so
  is a `CASE … END`). Any other string, however much it looks like SQL, is left
  alone. A SQL mutant the checker's SQL check refuses is `invalid`.
- **Nothing is written.** A mutant exists only in memory and the output goes to a
  scratch directory, so the working tree and `out/` are untouched. A mutant that
  doesn't compile, or whose run fails before any case runs (it throws as the
  module loads), is `invalid` and isn't scored.
- **The unmutated project runs first and must pass.** Otherwise every mutant would
  be "killed" by the failure that was already there. The run uses one seed
  throughout (yours with `--seed`, else a fixed one), so a `property` test draws the
  same inputs for every mutant.
- **Which suites run.** The suites that target the mutated unit, a unit that
  `uses` it, or that `use` it themselves. A mutant no suite reaches is `NO SUITE`:
  reported, not run and not scored. `--mutate-all-suites` runs every suite against
  every mutant instead.
- **A survivor is a gap, not a bug.** Some survivors change nothing observable (an
  *equivalent* mutant); the span and the exact change are there so you can judge.
  Leave code out with [`[test.mutate] exclude`](/reference/tooling/manifest/#testmutate).
- **Exit code.** `0` when mutants survive, so a report can run in CI without
  failing it; non-zero if the unmutated project fails or won't build, and with
  `--mutate-fail-under PERCENT`, when the score is below it or nothing was scored. The score is the share
  of mutants that ran which were killed.
- **Cost.** Every mutant is a compile, `tsc` and `node` run, so use `--only` and
  `--max-mutants` to keep it bounded, `--changed GIT_REF` to mutate only what a
  change touched, and `--mutate-jobs N` to run mutants in parallel. The report is
  the same whatever `N` is: the same mutants, in the same order, with the same ids,
  and each mutant's timeout applies to its own run. The default timeout is derived
  from the unmutated run, so on a loaded machine with many jobs raise it with
  `--mutate-timeout`. A mutant is reached by a suite by its target and `uses` only:
  per-mutant coverage narrowing isn't offered, since coverage is a whole-run
  report, not per suite or case. At a workspace root `--mutate` is refused: run
  it inside a member.

With `--format json` the document is the unmutated run's, plus a `mutation` object:
`total`, `killed`, `timeout`, `survived`, `no_suite`, `invalid`, `score` and
`mutants[]`, each with `id`, `file`, `line`, `column`, `end_line`, `end_column`,
`unit`, `decl`, `from`, `to`, `status` and `suites`. With `--changed`, the object
also holds `changed`: `{ "ref": …, "files": [...] }`, the files the run was
limited to.

### Snapshots

A case can compare a whole value with a stored snapshot (`expect v matches
snapshot "name"`; see [Snapshots](/reference/testing/#snapshots)). The files
live in `snapshots/<suite file>.snap.json` and are reviewed like code.

```text
bynk test                      # fails with a diff if a value changed, or has no snapshot
bynk test --update-snapshots   # writes new snapshots, refreshes changed ones, deletes obsolete ones
```

- A missing snapshot **fails** without the flag, so CI cannot pass a snapshot nobody
  reviewed. A snapshot no case takes any more is **obsolete**: the run reports it
  and passes, and `--update-snapshots` deletes it.
- Every format reports what the run did. `rich` ends with a line such as `snapshots:
  2 written, 1 updated, 3 obsolete (remove with --update-snapshots)`, and nothing
  when there is nothing to say. The `--format json` document has a last field
  `"snapshots": { "written": 2, "updated": 1, "obsolete": 3 }`, and the NDJSON
  `run-end` carries the same object. Both are absent for a project that takes no
  snapshot. Under the flag, `obsolete` counts the snapshots deleted.
- The files are written once the run has finished, from the runner's report, and
  only by the driver. With `--case` or `--suite`, only the snapshots of the cases
  that ran change.
- `--update-snapshots` is refused with `--against`, `--smoke`, `--coverage`,
  `--inspect` and `--no-run`. At a workspace root each member updates its own
  `snapshots/`.

### Streaming results

`--format ndjson` writes one JSON event per line as the run produces them, so a
consumer sees each suite when it finishes instead of everything at the end. It
carries the same information as `--format json`, which stays the single document
and the right choice for CI that wants one result.

| Event | Fields, in order |
|---|---|
| `run-begin` | `type`, `suites` |
| `suite-begin` | `type`, `name`, `kind`, `tests` |
| `case` | `type`, `suite`, then the document's case object: `name`, `outcome`, `message`, `location`, `retries`, `tier`, `kind`, `seed`, `counterexample`, `violation`, `refusals`, `origin`, `warnings`, each omitted when absent, then `durationMs` (omitted for a skipped case) |
| `suite-end` | `type`, `name` |
| `run-end` | `type`, `passed`, `failed`, `skipped` and `transport` (a remote run only), then `snapshots` (`{ written, updated, obsolete }`, for a project that takes [snapshots](/reference/testing/#snapshots)) |
| `error` | `type`, then the document's `error` object: `kind` (`compile` or `runtime`), `message`, `diagnostics`, `stderr` |

- The stream ends with **exactly one** of `run-end` or `error`. A project that does
  not compile is a single `error` with `kind: "compile"`; a runner that dies before
  `run-end` ends the stream with an `error` with `kind: "runtime"` and its stderr;
  a failing case is not an error, and the exit code is non-zero for it as before.
  A stream that ends with an `error` also exits non-zero, even if the runner process itself
  exited 0, and so does a `--format json` document that carries an `error`: an incomplete report
  (a suite missing its `cases`) never passes as a clean run.
- `suite-begin.tests` is the number of cases the suite will report, and it is sent
  before the first of them runs: the runner counts what the filter selects. A case a remote
  run skipped (`--against … --skip-unsupported`) is a `case` with outcome `skipped`
  after the last `suite-end`, before `run-end`.
- Field order is part of the contract. A consumer ignores event types and fields it
  does not know, so the set can grow. A line of the runner's that is not an event is
  not forwarded.
- Events are flushed as they are written, and a `case` is written when that case
  finishes, so a consumer sees a result before the next case runs. `durationMs` is
  how long the case ran in milliseconds, measured by the host's own clock around the
  case (not the test harness's `Clock`), to a hundredth of a millisecond. It is the
  stream's alone: the `--format json` document carries no timing, so it stays the
  same from run to run. Under `--against dev`, which runs one process per
  environment group, the stream is written when the groups have finished.
- A driver older than the stream rejects the value; the VS Code extension then falls
  back to `--format json` for the session.

### Waiting for a real system

In process, every send and event a call starts has finished before the next
line of the case runs. A real system is not so tidy: a write may become
visible a moment later. So under `--against`, a failing `expect` is retried
until it holds or `--settle` expires (default `5s`; `0` turns it off).

The retry is a fixed rule, not a guess. Before each retry, the runner re-runs
the addresses made since the last **unsafe** one: an address whose HTTP method
isn't safe per RFC 9110 (anything but `GET`, `HEAD` or `OPTIONS`), or a
`schedule` or `message` address. The rule fails closed: any other effectful
line that isn't itself a safe address counts as unsafe too, such as a `<-` on
an `if` or `match` around an address, a `do e.detach()`, or a store write. Only
a plain `let` and an `expect` are replayed with the reads. An unsafe address is never repeated, so a
retry can't place a second order. An `expect` with no safe address before it
in that stretch isn't retried at all.

- **The deadline is per stretch, not per case.** Each retried stretch gets its
  own `--settle` window, so a case with two stretches can wait up to twice as
  long before it fails.
- **An `expect` after a `let … else` is never retried.** The rest of the block
  after a `let … else` becomes the body of its pattern match, outside the
  top-level lines the rule sees. A stretch before the `let … else` is still
  retried as usual.

```bynk,fragment
case "a placed order shows in the list" {
  let o <- api.POST("/orders", Order { sku: "w" }) by User("ada")  // unsafe: runs once
  expect o is Created(_)
  let list <- api.GET("/orders") by User("ada")                     // safe: replayed
  expect list is Ok(_)                                              // retried until it holds
}
```

Each case's retries are counted (`"retries": N` on the case under
`--format json`), so an ordering bug shows up as a retry count instead of
hiding behind a pass. In process nothing is retried: the in-process run stays
the gate.

### Running against `bynk dev`

`--against dev` runs the `system` suites against a local [`bynk dev`](#bynk-dev)
that the runner starts, waits for, and stops itself. Nothing reaches
Cloudflare: it is local `wrangler dev` only, and `-- --remote` is never passed.

- **One `bynk dev` per distinct `env`.** The runner applies each case's `env`
  to the Workers (as `-- --var NAME:VALUE`), so cases whose `env` maps differ
  run against separate `bynk dev` sessions, in turn. Each declared secret a
  case's `env` doesn't set gets a fixed test value, given to the Workers and
  used to sign, as in process.
- **Readiness and ports.** Each session gets free ports and a
  [`--ready-json`](#readiness) file; the runner waits for it (up to three
  minutes) and accepts only loopback URLs from it. A session that doesn't come
  up is reported with the path of its log, under the `--output` directory.
- **Clean stop.** Each session is stopped by its own pid when its cases finish
  (it stops its wranglers and their `workerd`s). Its process group is then
  killed as well, after up to 30 seconds, so nothing it started is left
  running.
- **What it can't honour.** As against a deployment, a case that moves the
  clock, addresses an `on call` service, delivers a queue message, runs a cron
  handler, or fetches an asset is refused (or skipped under
  `--skip-unsupported`). A cron handler is reachable on `bynk dev` only through
  wrangler's `/__scheduled`, which returns no result to check, so it stays
  refused.
- **Started for you.** `bynk test --against dev` starts `bynk dev` itself, runs
  the groups against it and stops it afterwards.

Under `--format json` the groups are reported as one document, with
`"transport": "remote:dev"`.

### Running against a deployment

`--against <name>` runs the same `system` cases, unchanged, against Workers
already running somewhere: a preview deployment, or anything else that serves
them. Each driver's request goes to the Worker's URL from
[`[test.targets.<name>]`](/reference/tooling/manifest/#test) instead of the in-process
harness, and each response is decoded exactly as in process.

- **Only named targets.** `--against` takes a name, never a URL, and a target
  marked `production = true` is refused.
- **Secrets from the environment.** The runner signs each credential with the
  secrets the target lists, read from the environment by name. A name that isn't
  set refuses the run before any request.
- **The target is printed first.** Before the first request, the runner prints
  the target's name and each Worker's URL on stderr.
- **Only the `system` suites run.** Unit and `integration` suites stay in
  process.
- **What a real runtime can't honour is refused.** A deployment answers only on
  its public `http` routes, so a `system` case that moves the clock
  (`advance Clock`), addresses an `on call` service, delivers a queue message,
  runs a cron handler, or fetches an asset is refused with a
  `bynk.test.remote_unsupported_*` diagnostic. `--skip-unsupported` skips such
  cases instead, naming each one and why; under `--format json` each is an
  `outcome: "skipped"` case, counted in `skipped`. Without it, a suite can't
  silently shrink to nothing.
- **A case's `env` must match the target.** A deployment already runs with its
  own variables, so the runner can't apply a suite's `env`. A case runs only
  if its `env` and the target's `vars` agree on every name either one
  mentions; one that sets a secret, sets a name the target doesn't declare,
  sets a different value, or leaves a declared name unset is refused
  (`bynk.test.remote_unsupported_env`). This keeps a suite written for a fake
  payment processor from running against a deployment wired to the real one.
- **State isn't fresh.** A real deployment keeps its state between runs, so a
  case that relies on starting empty may not hold the second time. Key what a
  case creates by [`run.id`](/reference/testing/#run-id), which is fresh
  for each `--against` run, so a second run doesn't meet the first run's data.
- **The clock is the Workers' own.** In process the harness clock starts at the
  epoch; a real runtime's Workers read wall-clock time, and the runner can't
  move it. A `system` case whose `expect` compares an `Instant` built from a
  constant (`Instant.fromEpochMillis(0)`) gets a
  `bynk.test.remote_clock_dependent` warning: it can pass in process and fail
  against a real runtime, and no `--settle` retry fixes that.

Under `--format json` the document gains `"transport": "remote:<name>"`, and
a case that `--settle` retried carries `"retries"`.

### Smoke-testing every route

`--smoke` derives a `system` suite from each context's `from http` services
and runs it in place of the project's own suites, so every declared route is
called without a test written for it. Each route gets up to three cases:

| Case | Sent | Passes when |
|---|---|---|
| `smoke: <METHOD> <path> answers` | every argument fabricated by [`Val`](/reference/testing/#val), as the route's actor | the answer is not a 5xx, and its JSON body decodes as the route's declared type |
| `smoke: <METHOD> <path> rejects a malformed body` | the same, with `{not json` as the body (routes with a JSON `body`) | the boundary refuses it, or the service's [`on rejected`](/reference/http/#rendering-rejections) renders a `400` |
| `smoke: <METHOD> <path> refuses no credential` | the same, `by Nobody` (routes behind an actor) | the auth seam refuses it with `401` |

A case acts as `Visitor` when the route admits it, and otherwise as the route's
first actor with a fabricated identity (`by User(Val[UserId])`). A context with
more than one `from http` service puts the service's name before the method.

- **What is skipped.** A route whose arguments or principal `Val` can't
  fabricate, such as a `String where Matches(…)` path parameter with no
  [`@example`](/reference/testing/#val-example), is skipped and
  named with the checker's reason (`[bynk.val.needs_pin]`). Under
  `--format json` it is an `outcome: "skipped"` case, counted in `skipped`.
  The run reports `smoke: <n> cases from <m> routes, <k> skipped`, and fails if
  no case could be built.
- **Only the smoke runs.** The project's own suites and doc examples are left
  out of a smoke run, so one of them a real runtime can't honour doesn't refuse
  it.
- **Writes stay local.** In process and against `dev` every route is called.
  Against a named target only `GET` routes are, since fabricated writes would
  land on a shared deployment; pass `--smoke-unsafe` to call the others too.
- **Decoding is strict.** A smoke case fails on a status no `HttpResult`
  carries, or a JSON body that doesn't decode as the route's declared type,
  where an author's own case receives the raw value to judge. That catches a
  deployment built from different source than the run.
- **An answer a route does not declare is a warning.** A `answers` case that gets a
  non-5xx status outside the ones the route'"'"'s handler can return (the set
  `bynk openapi` documents, which includes the boundary'"'"'s own `400`, `401`,
  `413` and `415`) passes, and warns with `bynk.test.smoke_undeclared_status`,
  naming the route, the status and the declared statuses:

  ```text
  bynk test: warning[bynk.test.smoke_undeclared_status]: `GET /clash` (shop.api, service `api`) answered 409 Conflict, which it does not declare; it declares 200
  ```

  The warning is on stderr in every format. Under `--format json` it is on the
  case, as `"warnings": [{ "code", "route", "status", "declared", "message" }]`,
  after `"origin"`; under `--format ndjson` the `case` event carries the same
  array, before `"durationMs"`. It never fails the run: the set is walked from the
  handler'"'"'s body, so a status that comes from a helper'"'"'s result is outside it, and
  treating the set as exhaustive would fail on the false positives the warning
  exists to surface.
- **`--print`.** Prints the synthesised suites, each headed by the path it would
  have beside its context file, and the skipped cases as comments. Copy one
  into `tests/` to extend it. A smoke run writes nothing into the project: a
  failure's location names the synthesised file, which `--print` shows.

A smoke proves that every route answers as declared. It says nothing about
behaviour across requests, which the project's own suites cover.

Every `--format json` document, in a run or in `--no-run` discovery, gives a case
its effective tier as `"tier"` (`unit`, `integration` or `system`: its own `as`
clause, else its suite's, else `unit`). It is absent for a case with no tier, such as a
property, so an editor can select the `system` cases without guessing from the
file name.

A failed `property` or history property also carries `"kind"` (`property` or
`history`), `"seed"` (the run's root seed as `0x…` hex, which `--seed` takes
back) and, when shrinking produced one, `"counterexample"` (the shrunk inputs, or
the shrunk sequence of a history, as the runner rendered them). They follow
`"tier"` in that order and are absent from a passing case.

A failed case that an invariant or transition violation caused also carries
`"violation"`, after `"counterexample"`: an object with `"kind"` (`invariant` or
`transition`, which the name alone cannot say), `"agent"`, `"name"` (the declaration's
name) and `"location"` (its `{ "path", "line", "col" }`, where the name is declared).
It is absent from a case that failed any other way.

A failed history can also carry `"refusals"`, after `"violation"`: the steps of its
shrunk sequence that an agent refused, which a history records as data rather than
as a failure. Each is an object with `"index"` (the step's position in the reported
sequence, from 0), `"call"` (the handler it called) and the refusing declaration's
`"kind"`, `"agent"`, `"name"` and `"location"`, as `"violation"` gives them. It says
what was refused, not that the refusal failed the case. It is absent when no step
was refused.

A case from a [doc-comment example](/reference/testing/#doc-examples-run)
marked `bynk test` also carries `"origin": "doc"`, after `"refusals"`. It is absent
from every other case, so a missing `"origin"` is an ordinary `case`, and a
document with no doc examples is unchanged.

A property that threw
something other than a failed `expect` carries `"kind"` and `"seed"` only, since
its draws depend on the seed just the same. `--case <name> --seed <seed>` runs
just that case from the reported seed and draws the same stream, so long as the
property tests declared before it in its suite are unchanged. In `--no-run`
discovery, every generative case carries `"kind"`, so a tool can tell which cases
a seed's meaning depends on.

A `violation` is not reported for a case in a `system` suite or a remote run: the agent
runs behind the context's services, so its fault is converted at the call boundary and never
reaches the case, which sees a `CalleeFaulted` or a response that is not `Ok`. An absent
`violation` therefore does not mean that no declaration refused the case.

---

## `bynk eval`

Evaluate a snippet in a unit's scope against the project, and print the value it
ends in as JSON (#3585). It is what an editor notebook runs for a cell, and the
guide [Try an expression against your project](/book/guides/testing/try-an-expression/)
walks through it.

```text
bynk eval [INPUT] --unit UNIT [--expr SNIPPET] [--sources-stdin] [--suite UNIT] [--timeout SECONDS] [--locked] [--offline]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | Project root directory. |
| `--unit UNIT` | required | The `commons` or `context`, by dotted name, whose scope the snippet is evaluated in. |
| `--expr SNIPPET` | stdin | The snippet. Without it, the snippet is read from standard input. |
| `--sources-stdin` | off | Evaluate against unsaved buffers: a JSON object `{ "<absolute path>": "<text>" }` on stdin wins over the files on disk, as for [`bynk compile`](#bynk-compile) (ADR 0626). Stdin then holds the buffers, so the snippet must be `--expr`; asking for both is an error. |
| `--suite UNIT` | — | Build `--unit`'s own suite into the evaluation and put the snippet inside it, so the suite's `stub`s, `fn`s and fixtures apply. `UNIT` must be the unit named by `--unit`; any other is refused. A suite that does not compile is reported. It is an error when the unit has no unit-tier suite (a suite of another tier cannot hold a snippet). |
| `--timeout SECONDS` | `60` | Kill the evaluation after this many seconds and report it as timed out. |

**How it runs.** The snippet becomes the body of one `case` in a synthetic `suite`
for `--unit`, and is built and run by the build and runner `bynk test` uses
(`tsc` with Node.js, or `tsx`, on `PATH`). The `case`'s last expression is
encoded as the wire shape `Json.encode` gives it and reported with its type.
Each evaluation is **independent**: an earlier cell's `let`s are sent again
ahead of the snippet. The snippet sees the unit's declarations and what a suite
for that unit sees. Its leading `consumes` and `stub` lines (with the blank and
`//` lines among them) belong to the suite, and the rest is the case, so it
`consumes` what it uses from other units, as a suite file does.

**No real I/O.** A capability that reaches outside the process (`Fetch`, a
database, a secret store) is refused unless it has a `stub`, in the snippet or in
a suite named with `--suite`; `Clock`, `Random` and `Logger` are the in-memory
doubles every test has. By default the project's own suites are left out of the
build, so a suite that does not compile cannot stop an evaluation.

This is an explicit, local run of code the user wrote in their own workspace, at
the trust level of `bynk test`. It does not change how the refined-type explorer
evaluates a hover (ADR 0638).

**Result.** One document on stdout, in this order (when the project's dependencies cannot be prepared, the reason goes to stderr as for the other commands, and the document is a `setup` error):

```json
{"value": <wire shape>, "type": "Result[Percent, ValidationError]", "diagnostics": []}
```

| Field | Meaning |
|---|---|
| `value` | The wire shape of the snippet's last expression; `null` when the snippet did not check (see `diagnostics`). Absent when `error` is present. |
| `type` | That expression's type; `null` when the snippet did not check. Absent when `error` is present. |
| `diagnostics` | When the snippet or the project does not check: one entry each of `line`, `col` (1-based, in the snippet), `severity`, `code` and `message`. An entry in another file adds its `file`, with that file's own line and column. Empty otherwise. |
| `error` | Present only when the snippet checked, or could not be set up, and produced no value: `kind` is `runtime` (a failed `expect`, an unstubbed capability, a value with no wire shape such as a bare effect), `timeout`, or `setup` (no such unit, a bad flag combination, no suite to build the snippet into), and `message` says why. With an `error`, `value` and `type` are left out: `{"diagnostics": [], "error": {"kind": "runtime", "message": "…"}}`. |

**Exit code** — `0` when there is a value (a refused refined-type construction,
`{"kind":"Err",…}`, is a value), `1` otherwise.

**Timing.** An evaluation is a build and a run of the project, in process, so its
cost is that of a one-case `bynk test`. It is not incremental: each evaluation
compiles the whole project again, and what is reused between runs is the `out`
directory and the installed npm packages. Measured with a release build on a
4-core Linux machine with Node 22 and a global `tsc`, for a project of one unit
and one of six units:

| | Time |
|---|---|
| First evaluation, empty `out` | 0.8 – 0.9 s |
| Later evaluations | 0.7 – 0.9 s |
| `bynk test` on the six-unit project, for comparison | 0.9 s |

A project that needs npm packages (a first-party adapter, say) pays their
installation once, on the first evaluation, as `bynk test` does.

## Dependencies: `bynk.lock`, `--locked` and `--offline`

Before `bynk check`, `bynk test`, `bynk eval`, `bynk dev` and `bynk deploy`
compile, they resolve the project's [`[dependencies]`](/reference/tooling/manifest/#dependencies) (and
`[dev-dependencies]`) against [`bynk.lock`](/reference/tooling/manifest/#bynklock), fetch any
git dependency missing from the store, re-check every fetched dependency
against its locked checksum, and rewrite the lock if the dependencies changed.
In a workspace, the members share one lock at the workspace root.

| Flag | Meaning |
|---|---|
| `--locked` | Fail (`bynk.package.lock_out_of_date`) instead of changing `bynk.lock`: build exactly what was committed. |
| `--offline` | Fail (`bynk.package.not_fetched`) instead of fetching: every git dependency must already be in the store. |

Both flags are accepted by `check`, `test`, `dev`, `deploy` and `fetch`. CI
typically runs `bynk fetch` (or restores a cached store) and then
`bynk check --locked --offline`. A dependency missing from the store under `--offline` is one
`bynk.package.not_fetched` diagnostic.

**The store** holds each fetched commit's files, named by their checksum, under
`$BYNK_HOME/store/sha256/` (`BYNK_HOME` defaults to `$XDG_CACHE_HOME/bynk`, or
`~/.cache/bynk`). Its files are read-only. If one is changed,
`bynk.package.checksum_mismatch` fails the build: delete that entry and run
`bynk fetch`.

**Git** is your own `git`, run without hooks or submodules, over `https` or
`ssh` only (a `file://` URL is allowed in your own manifests). Bynk passes no
credentials: a private repository works through your git setup — an ssh agent
or a credential helper — exactly as `git clone` would.

## `bynk fetch`

```text
bynk fetch [PATH] [--locked] [--offline] [--format short|json]
```

Resolve, fetch and lock, and nothing else: warm the store before an
`--offline` step, or update `bynk.lock` after editing `[dependencies]`. Prints
each package it fetched and whether it wrote the lock. **Exit code** — `0` on
success; non-zero on any resolution, fetch or checksum failure.

When the lock it writes has npm packages awaiting approval, `fetch` (like
`add` and `update`) asks about each on a terminal, and otherwise lists them
with the command to run; it succeeds either way.

With `--format json` the outcome is one document on stdout (the notices stay on
stderr), for editors and scripts:

```json
{ "wrote_lock": true, "fetched": ["acme/money@3f2a…"], "warnings": [],
  "pending_npm": [{ "package": "jose", "range": "^5", "required_by": ["acme/ledger:auth"] }] }
```

## `bynk approve`

```text
bynk approve [NPM-PACKAGE] [--all] [--list [--format short|json]] [--path PATH]
```

Approve the npm packages that git and registry dependencies' adapters bring
into the build, recording each as `approved = true` on its `[[npm]]` row in
`bynk.lock`. A build that reaches an adapter whose npm package isn't approved
fails with `bynk.requires.unapproved`. For each pending package it shows the
range, the chain of dependencies that requires it, and its npm page.

| Form | What it does |
|---|---|
| `bynk approve` | Asks `[y/N]` about each pending package; on a non-terminal it refuses and names the other forms. |
| `bynk approve <package>` | Approves that npm package's pending ranges. |
| `bynk approve --all` | Approves everything pending without asking — for adopting an existing lock. |
| `bynk approve --list` | Lists each row as `pending`, `approved` or `own` (required only by path dependencies); changes nothing. |
| `bynk approve --list --format json` | The same listing as one JSON document, for tooling (the editor's review reads it). |

The JSON document is `{ "pending": N, "requirements": [ … ] }`. Each row has
`package`, `range`, `state` (`pending`, `approved` or `own`), `required_by`
(`org/package:adapter`), `chains` (for each requirer, the path from `this
project`), `url`, and, for a pending row, `change`: `new`, or `range_changed`
when another range of the package is approved (`approved_ranges`). `--format`
needs `--list`.

It rewrites only `approved` fields and never resolves or fetches. See
[Approve a dependency's npm packages](/book/guides/projects-build-and-deployment/share-code-between-projects/#approve-a-dependencys-npm-packages).

## `bynk update`

```text
bynk update [DEPENDENCY…] [--path PATH]
```

Re-resolve git dependencies' revisions and rewrite `bynk.lock`. A dependency
locked to a branch keeps its locked commit until it is updated, even when the
branch moves upstream; `update` moves it. Name aliases or `org/package` names to
update only those; with none, every git dependency is re-resolved. Changing a
dependency's `rev` in `bynk.toml` re-resolves it on the next command, without
`update`.

---

## `bynk revert`

Makes the previous deployment live again, without rebuilding. It drives
Cloudflare's own version rollback, after checking what a platform rollback
cannot: migrations, agent schema versions, contracts, ids and secrets. See
[Roll back a deployment](/book/guides/projects-build-and-deployment/roll-back-a-deployment/)
for what each check means and how to read a refusal.

```
bynk revert [PATH] [--context NAME | --worker NAME] [--env NAME] [--to VERSION] [--dry-run]
            [--format short|json] [--yes]
            [--accept-schema-ahead BINDING]... [--accept-secret-changes]
```

| Flag | Default | Meaning |
|---|---|---|
| `--context NAME` | every recorded Worker | Revert this context alone (dotted name or Worker directory). A member of a [`[deploy.workers.<key>]`](/reference/tooling/manifest/#worker-groups) group is refused, naming the Worker that holds it. |
| `--worker NAME` | every recorded Worker | Revert this Worker alone, by its directory name: a group's key, or a context's worker-directory form. A group is one script, so every member goes back together, atomically. Not with `--context`. |
| `--env NAME` | `default` | The environment to revert. |
| `--to VERSION` | what was live before the live release | A Cloudflare Worker version id to go back to; needs `--context` or `--worker`, since a version id belongs to one Worker. A git tag is not accepted: it names source, which means a rebuild. |
| `--dry-run`, `--plan` | off | Print each check's verdict and stop. Unlike `deploy --dry-run` it reads Cloudflare, so it needs credentials. |
| `--format short\|json` | `short` | Plan format. `json` is for CI. |
| `--yes` | off | Skip the confirmation. Never overrides a refusal. |
| `--accept-schema-ahead BINDING` | none | Assert that the D1 migrations applied after the target was built are additive, for this binding. Repeatable. |
| `--accept-secret-changes` | off | Accept secret changes made after the target was live. This restores the old values of any rotated secret; a `bynk migrate sweep` makes two harmless changes. |

It needs `CLOUDFLARE_API_TOKEN` (with Workers Scripts and D1 read access) and
`CLOUDFLARE_ACCOUNT_ID`. A version can be reverted only if the `bynk` that
deployed it recorded its deploy facts; for any earlier version, redeploy its
tag. A refusal exits non-zero and changes nothing.

**The JSON plan.** With `--format json`, `revert` writes the plan as one JSON document to
stdout, before it asks to confirm (and instead of the confirmation with `--dry-run`):

```json
{
  "version": 1,
  "environment": "default",
  "workers": [
    {
      "worker": "shop", "script": "shop",
      "live_version": "v2", "target_version": "v1",
      "checks": [
        { "check": "contracts", "subject": "shop", "verdict": "ok",
          "detail": "no skew against the other live Workers" }
      ],
      "ok": true,
      "lock_changes": true
    }
  ],
  "ok": true,
  "notes": []
}
```

`workers` is in the order they would be reverted. A worker's `ok` is `true` when none of its
`checks` refused, and the top-level `ok` is `true` when every worker's is. The run exits non-zero
when `ok` is `false`. `lock_changes` says whether the revert changes the Worker's entry in
`bynk.deploy.lock`, and `notes` are lines to read before confirming. Each check is
`{"check", "subject", "verdict", "detail"}`, and a refusal adds `code`, the registered
`bynk.revert.*` code, so a script matches on it. `verdict` is one of two words:

| `verdict` | Meaning |
|---|---|
| `ok` | The check found nothing that makes the rollback unsafe. It has no `code`. |
| `refused` | The check found something, `detail` says what, and `code` names it. The worker's `ok` is `false`. |

| `check` | What it looks at | Refusal `code` |
|---|---|---|
| `facts` | The deploy facts recorded on the live version and the target. | [`bynk.revert.facts_missing`](/reference/diagnostics/bynk.revert.facts_missing/) |
| `history` | Placing the live version and the target in the Worker's deployment history. | [`bynk.revert.history_unplaceable`](/reference/diagnostics/bynk.revert.history_unplaceable/) |
| `members` | A `[deploy.workers.<key>]` group holding the same contexts in both versions. | [`bynk.revert.membership_changed`](/reference/diagnostics/bynk.revert.membership_changed/) |
| `contracts` | Contract hashes against the other live Workers, as if every selected Worker were reverted. | [`bynk.revert.contract_skew`](/reference/diagnostics/bynk.revert.contract_skew/) |
| `ids` | Each bound resource being the one `bynk.deploy.lock` records. | [`bynk.revert.id_unrecorded`](/reference/diagnostics/bynk.revert.id_unrecorded/) |
| `secrets` | A secret changed since the target was live. | [`bynk.revert.secret_changed`](/reference/diagnostics/bynk.revert.secret_changed/) |
| `d1` | D1 migrations applied after the target was built. | [`bynk.revert.database_schema_ahead`](/reference/diagnostics/bynk.revert.database_schema_ahead/) |
| `agent` | The agent schema version the target writes against stored instances. | [`bynk.revert.agent_schema_older`](/reference/diagnostics/bynk.revert.agent_schema_older/) |
| `layout` | The agent storage layout the target writes. | [`bynk.revert.layout_older`](/reference/diagnostics/bynk.revert.layout_older/) |
| `classes` | The Durable Object classes of the live version and the target. | [`bynk.revert.class_changed`](/reference/diagnostics/bynk.revert.class_changed/) |
| `container` | A container class the rollback restores. | none: it only passes, with a `warning:` in `detail` that running instances keep the image they started with. |

A check can appear for each binding or class it looks at, so a worker can have several of the
same `check`; match on `code`, not on position.

**Missing credentials.** A revert reads the account, so it stops when `CLOUDFLARE_API_TOKEN` or
`CLOUDFLARE_ACCOUNT_ID` is unset, once it has checked its own arguments and read `bynk.deploy.lock`.
With `--format json` it writes this to stdout, beside the `bynk: error:` line on stderr, and exits
non-zero:

```json
{
  "error": {
    "code": "bynk.cloudflare.credentials_missing",
    "missing": ["CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"],
    "message": "set CLOUDFLARE_API_TOKEN and CLOUDFLARE_ACCOUNT_ID to revert: …"
  }
}
```

`missing` names only the variables that are unset, and the `message` says what the token needs.
An `error` document replaces the plan, so a client reads `error` first. **Only this failure is a
structured document.** Every other failure of a revert (`--to` without `--context`, a
`bynk.deploy.lock` that does not read or records no deployed Worker, a `--context` or `--worker`
that is not recorded) is the `bynk: error:` line on stderr, a non-zero exit and an empty stdout, and
the other commands that read the account say the same on stderr.

---

## `bynk lock`

Changes to `bynk.schema.lock` that are a person's decision.

### `bynk lock accept-order`

```text
bynk lock accept-order <TYPE> [--path PATH]
bynk lock accept-order --all [--path PATH]
```

Accepts the new order of a `derives Ord` type: rewrites its entry under
`[orders]` to the field or variant order now declared. A reorder changes how
every value of the type sorts, so a build refuses it
([`bynk.types.ord_order_changed`](/reference/diagnostics/bynk.types.ord_order_changed/))
until it is accepted. Commit the rewritten lock with the reorder.

`TYPE` is the type's name, or `<unit>.<Type>` when two types share a name. `--all`
accepts every type whose order differs from the lock and prints each one.
`--path` is the project directory (default `.`). It reads the source without
checking it, and refuses a project that does not parse. **Exit code:** `0`, or
non-zero when the type is not found or the lock cannot be written. Accepting an
order that is already recorded says so and writes nothing.

### `bynk lock accept-analytics`

```text
bynk lock accept-analytics <INSTANCE> [--path PATH]
bynk lock accept-analytics --all [--path PATH]
```

Accepts the new column mapping of an `Analytics` instance written with `writeValue`
(#4105): rewrites its entry under `[analytics]` to the mapping now written. Points already
written keep their old column positions, so a build refuses a reorder, a retyped field or a removed
field of the written record
([`bynk.analytics.mapping_changed`](/reference/diagnostics/bynk.analytics.mapping_changed/))
until it is accepted. Commit the rewritten lock with the change.

`INSTANCE` is the instance's name, or `<unit>.<Instance>` when two instances share a name. `--all`
accepts every instance whose mapping differs from the lock and prints each one. `--path` is the
project directory (default `.`). Unlike `accept-order` it checks the project, because the mapping is
read from the checked units, and refuses one that does not check. **Exit code:** `0`, or non-zero
when the instance is not found, the project does not check, or the lock cannot be written. Accepting
a mapping that is already recorded says so and writes nothing.

---

## `bynk migrate`

An agent's stored record migrates lazily, when it is next loaded. `bynk migrate`
shows where each agent class stands, and wakes every stored instance so the
class's **floor** can rise and the `migrate` steps below it can be deleted. See
[Retire old migration steps](/book/guides/agents-and-state/stateful-agent/#retire-old-migration-steps).

### `bynk migrate status`

```text
bynk migrate status [PATH] [--format short|json] [--locked] [--offline]
```

Offline. For each class `bynk.schema.lock` records, prints:

- its version and floor;
- each valid `migrate from N` step and the versions it covers;
- the versions that read forward without a step;
- any version with no path forward;
- the environments `bynk.deploy.lock` records its Worker in.

It checks the source against the lock, read-only, as `bynk check` does, and
refuses a project with errors. **Exit code:** `0`, or non-zero on errors.

`--format json` prints one document, `{ "classes": [...] }`, with an entry per
class: `class`, `version`, `floor`, `oldest_readable`, `steps` (`from`,
`covers_from`), `without_path` and `deployed_to`. VS Code's State Migrations view
reads it.

### `bynk migrate sweep`

```text
bynk migrate sweep [PATH] [--context NAME] [--env NAME]… [--batch N]
                   [--url WORKER=URL]… [--yes]
```

Wakes every stored instance of the selected agent classes in the deployed
Workers. Each instance loads, runs any `migrate` step, and commits at its class's
current version. An instance already current is read and not written.

| Option | Meaning |
|---|---|
| `--context NAME` | Sweep one context's classes. |
| `--env NAME` | Sweep this environment (repeatable). Default: every environment `bynk.deploy.lock` records. |
| `--batch N` | Instances woken per request to a Worker (default 40, within the Free plan's 50 subrequests). |
| `--url WORKER=URL` | Where a Worker that doesn't serve on `workers.dev` is reached, by its deployed name. Repeatable. |
| `--yes` | Skip the confirmation. Required when not interactive. |

- **Credentials.** The instances are listed through Cloudflare's API, so the sweep
  needs both `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID`. The token needs
  **Workers Scripts: Edit**: `wrangler` uses it too, to set and delete the run's key.
- **The run's key.** Each Worker gets a temporary secret, `BYNK_MIGRATE_KEY`, set
  before its first wake and deleted after its last. That includes when the sweep
  fails or you press Ctrl-C. Setting and deleting it each create a Worker version.
  Without the secret, the wake route answers `404`.
- **The floor.** A class's floor rises only when one run completed every
  environment the ledger records its Worker in. It rises to the oldest version
  any of them holds or writes. A narrower run (`--env`, `--context` aside) migrates
  but raises no floor, and the output says why.
- **After a deploy.** The floor also waits until each swept Worker's code was
  deployed at least 30 minutes earlier, because Cloudflare's listing can take about
  10 minutes to show a new instance. A run inside the window migrates, and says how
  many minutes remain. The deploy time comes from Cloudflare's deployments API.
  Changes to the sweep's own key are secret changes, not deploys, so they don't count.
- **The lock.** A raised floor rewrites `bynk.schema.lock`; commit it.
- **Duplicates under `@unique`.** A [`@unique(by: k)`](/reference/agents/#unique) added to a `Map`
  that already holds data is enforced only on new commits, so a value two keys already hold stays
  until a commit touches it. The wake reads each `@unique` index and the sweep reports what it
  finds under the class's line, for example
  ``@unique(by: email) on `people`: 3 values are held by more than one key: 2, 2 and 4 holders (in 2 instances)``.
  The report is the map, the field, how many values are shared and how many keys hold each. It
  never names a stored value, a hash of one or the keys that hold it. It is information: it
  neither fails the sweep nor holds a class's floor.

**Exit code:** `0` when every selected instance woke; non-zero otherwise.

## `bynk add`

```text
bynk add <ORG/PACKAGE>[@RANGE] [--as ALIAS] [--path PATH] [--format short|json]
```

Add a registry dependency: the highest version the package's registry offers
(or one matching `RANGE`), written to `[dependencies]` as
`<alias> = { package = "…", version = "^<version>" }` without reformatting the
rest of `bynk.toml`, then resolved and locked like `bynk fetch`. The alias
defaults to the package segment; an alias already in `[dependencies]` is
refused. With `--format json` it prints the entry it wrote on stdout:
`{ "added": { "alias": "money", "package": "acme/money", "version": "^1.4" } }`.

## `bynk search`

```text
bynk search <ORG/PACKAGE> [--path PATH] [--format short|json]
```

Look a package up on the registry its organisation routes to (in the project at
`PATH`), through the same verifying client a build uses: every version its
signed index lists, and the newest one that is neither yanked nor withdrawn.
It is an **exact** lookup: a registry's index is a file per package, with no
listing or free-text search to query. A name the registry doesn't hold, an
organisation that routes nowhere and a registry that can't be reached all print
the reason on stderr and exit non-zero, with nothing on stdout.

```json
{ "package": "acme/money", "registry": "acme-internal", "latest": "1.4.0",
  "versions": [{ "version": "1.3.0", "checksum": "sha256-…", "yanked": false, "withdrawn": null }] }
```

`latest` is `null` when every version is yanked or withdrawn. This is what
[vscode-bynk](/reference/tooling/vscode-bynk/) asks for **Add Dependency…** and its
upgrade lenses; the editor never queries a registry itself.

## `bynk deps`

```text
bynk deps [--path DIR] [--format short|json]
```

List `[dependencies]` and `[dev-dependencies]` with what `bynk.lock` resolved
each to. Offline: it reads `bynk.toml` and `bynk.lock` only. Each entry has its
`alias`, `package`, `dev`, `kind` (`registry`, `git` or `path`), `requested`
(the version range, git revision or path), `source` (the registry name, git URL
or path) and `locked` (the version `bynk.lock` pins, or `null`).

## `bynk publish`

```text
bynk publish [--path DIR] [--dry-run] [--expect-checksum CHECKSUM] [--format short|json]
```

Publish the package at `DIR` (default: the project holding the current
directory) to the registry its organisation routes to in `[registries]`. It
resolves and checks the package first — a package that doesn't check isn't
published — refuses one without a `version` or with a `path`/`git` dependency,
packs it, and uploads it to the registry's [publish API](/reference/tooling/registry-protocol/#publish-api).
The registry signs it. `bynk publish` then reads the new index line back,
verifying its signature and that its checksum is the tree it sent.

The token is the first of these:

1. `BYNK_REGISTRY_<NAME>_TOKEN`, which overrides every saved token;
2. the organisation's own slot, `[registries.<name>.organisations.<org>] token`
   in `~/.config/bynk/credentials.toml`, where
   [`bynk org token mint --save`](#bynk-org) puts one;
3. the registry-wide `[registries.<name>] token`, for any organisation without
   its own slot.

A first-party publish token acts for one organisation, so someone who publishes
for two keeps one slot for each. Resolving and fetching dependencies use only
the variable and the registry-wide slot. A `401` or `403` says which of these
the refused token came from. With
`--dry-run` it checks and packs, prints what it would publish, and uploads
nothing. With `--format json` it prints `package`, `version`, `checksum`,
`registry`, `dry_run` and the `files` it packs (each with its `path` and
`bytes`) on stdout, for a `--dry-run` as for the real thing. With
`--expect-checksum`, it refuses before uploading anything unless the package
packs to that checksum, so what a `--dry-run` reported is what gets published
or nothing is. The editor's
**Publish…** shows that plan and publishes only after a confirmation. A refusal is
`bynk.registry.publish_refused`, with the registry's
reason; a registry without an API points you to `bynk registry add`.

A registry read through a mirror (a `[registries.<m>]` with
`mirror-of = "<name>"`) is refused, `--dry-run` included, before anything is
fetched: the mirror's `config.json` names the publish API, and a mirror's word
isn't the registry's. Publish from a project without the mirror.

## `bynk yank`

```text
bynk yank <ORG/PACKAGE> <VERSION> [--undo] [--path DIR]
```

Yank a published version through its registry's API (`--undo` clears it), then
confirm it by reading the index back. A yanked version is kept for locked
builds and never newly chosen. The registry is the one the package's
organisation routes to in the project at `DIR`; like `bynk publish`, it is
refused when a mirror serves that registry, and it finds its token the same
way, preferring the organisation's own slot.

## `bynk login`

```text
bynk login [--registry NAME] [--path DIR]
```

Sign in to a registry's accounts, which the hosted first-party registry serves
beside its publish API. `bynk login` runs GitHub's device flow: it prints a
URL and a code, and you approve the code in your browser. Never enter a code you
didn't request. It then polls the registry at the interval the registry gives,
and less often whenever the registry asks it to slow down, until you approve,
decline, or the code expires.

The registry is `--registry`, by its `[registries]` name in the project at
`DIR`, and the first-party registry by default. Its account API sits beside the
`api` in its verified `config.json`; a registry with no `api`, or one served by
`bynk registry serve`, has no accounts. A registry read through a mirror is
refused, so the account token reaches only the registry itself.

The **account token** (`bynka_…`, valid for 30 days) is kept in the credentials
file, apart from publish tokens:

```toml
# ~/.config/bynk/credentials.toml ($XDG_CONFIG_HOME/bynk/credentials.toml), mode 0600
[registries.bynk]
token = "bynk_…"            # a publish token for any organisation without its own

[registries.bynk.organisations.acme]
token = "bynk_…"            # acme's publish token: bynk publish, bynk yank

[accounts.bynk]
token = "bynka_…"           # the account token: bynk login, bynk org
login = "ann"
expires-at = 1795392000000
```

`BYNK_ACCOUNT_<NAME>_TOKEN` overrides the saved session; an operator's token
with `operate` works there too, for `bynk org claim --owner`. An account token
can't publish, and a publish token can't manage an organisation. A refusal is
`bynk.registry.account_refused`, with the registry's reason.

## `bynk logout`

```text
bynk logout [--registry NAME] [--path DIR]
```

End the session on the registry and remove `[accounts.<name>]` from the
credentials file. It can't unset `BYNK_ACCOUNT_<NAME>_TOKEN`, and says so.

## `bynk org`

```text
bynk org claim <ORG> [--block] [--owner LOGIN]
bynk org show <ORG> [--format short|json]
bynk org block <ORG>
bynk org unblock <ORG>
bynk org member add <ORG> <LOGIN>
bynk org member remove <ORG> <LOGIN|ID>
bynk org token mint <ORG> [--can publish,yank] [--days N] [--note TEXT] [--save [--force]]
bynk org token revoke <ORG> <ID>
```

Each takes `--registry NAME` and `--path DIR` too. Without `--registry`, the
organisation picks the registry, as for `bynk publish`: the `[registries]` entry
whose `organisations` lists it, else the first-party registry. Sign in first
with `bynk login`.

| Subcommand | What it does |
|---|---|
| `claim` | Claims a free name, first come: you become its owner and first member. `--block` holds the name without publishing. A reserved name is refused; an operator claims it for someone with `--owner`. |
| `show` | Prints the owner, the members (GitHub login and id), whether the organisation is blocked, and its publish tokens: id, capabilities, expiry, who minted it and its note. Never the tokens themselves. `--format json` prints the registry's answer. |
| `block`, `unblock` | Blocks or unblocks the organisation (the owner only). A blocked organisation refuses every publish. |
| `member add` | Adds a GitHub user as a member (the owner only); they may mint publish tokens. |
| `member remove` | Removes a member by login or GitHub id (the owner only), which ends every token they minted. |
| `token mint` | Mints a publish token for the organisation that can `publish`, `yank` or both (the default), for 1 to 365 days (default 90). It's shown once: alone on stdout, so it can be piped into a CI secret. With `--save` it's written to the organisation's own slot, `[registries.<name>.organisations.<org>] token`, instead, where `bynk publish` and `bynk yank` read it; if that slot holds a different token, `--save` refuses (and mints nothing) unless `--force`. The registry-wide slot and other organisations' slots are left alone. |
| `token revoke` | Revokes a token by the id `show` prints, or a unique prefix of it (the owner, or the member who minted it). |

From nothing to a first publish:

```bash
bynk login --registry bynk
bynk org claim acme
bynk org token mint acme --save
bynk publish
```

## `bynk registry`

Build a registry in a directory — see
[Run a private registry](/book/guides/projects-build-and-deployment/run-a-private-registry/)
and the [Registry protocol](/reference/tooling/registry-protocol/).

```text
bynk registry init <DIR> --id <ID> [--key-out <FILE>]
bynk registry add <DIR> <PACKAGE-DIR> --key <FILE>
bynk registry yank <DIR> <ORG/PACKAGE> <VERSION> [--undo]
bynk registry withdraw <DIR> <ORG/PACKAGE> <VERSION> --reason <TEXT>
bynk registry serve <DIR> --key <FILE> --tokens <FILE> [--listen ADDR]
bynk registry token <TOKENS-FILE> --org <ORG>... [--can publish,yank]
bynk registry conformance <INDEX-URL> --org <ORG> [--name NAME] [--key KEY] [--narrow-token-env VAR] [--max-upload BYTES] [--format short|json]
bynk registry conformance <INDEX-URL> --read-only [--package ORG/PACKAGE]
bynk registry rotate <DIR> --key <CHAIN-KEY> --key-out <FILE>
bynk registry delegate <DIR> --key <CHAIN-KEY> --delegate-out <FILE>
bynk registry revoke <DIR> <ed25519:KEY> --key <CHAIN-KEY> [--force]
bynk registry resign <DIR> --key <SIGNING-KEY>
```

| Subcommand | What it does |
|---|---|
| `init` | Writes `<DIR>/config.json` and a new ed25519 signing key (default `./<ID>.key`, refused inside `<DIR>`), and prints the `key = "…"` line consumers pin. The id follows `[a-z][a-z0-9-]*` and is not a `[registries]` name, so `bynk`, the first-party registry's own id, is accepted here even though a project's `[registries.bynk]` is refused. |
| `add` | Signs with the newest chain key or a valid, unrevoked delegate. Packs the package (its sources; never `out/`, `node_modules/` or hidden files), signs its index line with the registry's current key, and appends it. A version already present with a different tree, or a `path`/`git` dependency, is refused; the identical tree again changes nothing. |
| `yank` | Marks a version yanked (`--undo` clears it): kept for locked builds, never newly chosen. Nothing is deleted or re-signed. `--undo` refuses a withdrawn version. |
| `withdraw` | [Withdraws](/reference/tooling/registry-protocol/#withdrawals) a version — a takedown: yanks its line, records `--reason` (one line, at most 500 characters) as its `withdrawn`, and deletes its archive, unless another version names the same tree (kept, with a warning). Nothing is re-signed. Re-run after a failure, it finishes deleting the archive. Final: it otherwise refuses a version already withdrawn, and `add`, the publish API and `yank --undo` then refuse that version too. |
| `serve` | Serves `<DIR>` over HTTP with the [publish API](/reference/tooling/registry-protocol/#publish-api) at `/api` (recorded as `"api"` in `config.json`), signing uploads with `--key` and authorising them with `--tokens`. It binds `127.0.0.1:8730` by default and handles one request at a time, so writes never interleave. It speaks plain HTTP: to serve beyond the machine, put a TLS-terminating proxy in front. The key and tokens files must be outside `<DIR>`. |
| `rotate` | Appends a link to the [key chain](/reference/tooling/registry-protocol/#trust): a new key (written to `--key-out`, refused inside `<DIR>`), signed by the current chain key. Clients pinned to an older key follow the chain, and every valid delegation and revocation is re-signed with the new key, so a client pinned to it accepts them too. |
| `delegate` | Creates a key that may sign index lines and nothing else, delegated by the newest chain key (so every client, whichever link it trusts, accepts it); use it as `serve`'s or `add`'s `--key` and keep the chain key offline. Makes the config protocol 2. |
| `revoke` | Revokes a delegate, signed by the newest chain key. Refuses when a line would be left with no valid signer — `resign` first — unless `--force`. |
| `resign` | Re-signs every index line with the given key (the newest chain key or a valid delegate); only each line's `sig` changes. A line no key the registry knows signed (chain, delegate, or revoked delegate) is refused, never laundered. Don't run it while a server is publishing to the same directory: a line appended mid-run would be lost. |
| `conformance` | Checks a registry against the [protocol](/reference/tooling/registry-protocol/#conformance): each check passes, fails or is skipped, and any failure is `bynk.registry.nonconformant` (exit 1). A full run publishes one throwaway package under `--org`, using the token for `--name` (default `conformance`, so `BYNK_REGISTRY_CONFORMANCE_TOKEN`), and leaves it yanked; `--read-only` publishes nothing. |
| `token` | Mints a publish token for the organisations named by `--org` that can `publish`, `yank` or both (the default), appends its sha256 to the tokens file (created private), and prints the token once. Restart `serve` to pick it up. |

---

## `bynk explain`

Explain a diagnostic code — the longer-form answer behind a `bynk.*` error, the
analogue of `rustc --explain`. When you hit an error like
`bynk.resolve.unknown_type`, `bynk explain bynk.resolve.unknown_type` prints
what the rule is, *why* it exists, a minimal before/after example, and a link to
the relevant [Book](/book/) concept page. Runs entirely in-process — it shells
nothing and reads no network, so the blurb is the complete, offline answer.

```text
bynk explain <CODE>
```

| Argument | Meaning |
|---|---|
| `CODE` | The diagnostic code to explain, e.g. `bynk.resolve.unknown_type`. |

The explanations are curated highest-traffic-first, so coverage grows over time:
a recognised code that is not yet curated prints its one-line summary and a
pointer to the [diagnostic index](/reference/diagnostics/) rather than a
full explanation. The same compiler-owned mapping powers the editor's clickable
diagnostic-code links (`codeDescription`) and the language server's
`bynk/explain` request, behind VS Code's **Explain** quick fix, so the CLI and
the editor never disagree.

**Exit code** — `0` for any code the compiler recognises (explained or not);
non-zero for an unrecognised code. A misspelt code that is close to a real one
also gets a `did you mean` line, found by the same search as
`bynk check --fix=<code>`, so the two commands suggest the same code:

```text
$ bynk explain bynk.types.not_orderabel
error: `bynk.types.not_orderabel` is not a diagnostic code the compiler emits.
did you mean `bynk.types.not_orderable`?
Diagnostic codes look like `bynk.resolve.unknown_type`; see … for the full list.
```

A code far from any real one gets the message without the suggestion.

---

## `bynk book`

Print or search the [Book](/book/) — every tutorial, guide, reference page and the
specification, in reading order. The text is embedded in the `bynk` binary when
it is built, so it is the Book **for this toolchain**: it describes what this
version compiles, and it needs no network. The editor's `bynk_search_book`
[language-model tool](/reference/tooling/vscode-bynk/) calls it.

```text
bynk book [--search QUERY [--format text|json]]
```

| Argument | Default | Meaning |
|---|---|---|
| *(none)* | | Print the whole Book as plain text. The first line names the version (`Bynk Book v0.406.0`). |
| `--search QUERY` | | Print the five sections that best match the words in `QUERY` instead, each with its page link and the version it came from. A word in a heading counts for more than one in the body, and a section holding every word beats one holding some. |
| `--format` | `text` | With `--search`: `text` is Markdown; `json` is one document with `version`, `query` and `results` (each with `trail`, `url` and `text`). |

A `bynk` built from source without the bundle (a plain `cargo build`; the release
binaries always carry it) has no Book: it prints "no Book bundled in this build"
on stderr and the hosted copy at `bynk-lang.org/llms-full.txt` is the fallback.

**Exit code** — `0` when it printed the Book or search results (including a
search with no match); non-zero when this build carries no Book.

---

## `bynk mcp`

Serve the toolchain's read-only answers to an AI agent over the [Model Context
Protocol](https://modelcontextprotocol.io/), on stdin and stdout. A client
launches it as a subprocess; it exists so an agent can ask the installed
toolchain about Bynk rather than rely on remembered syntax. Per-client set-up is
in [`bynk mcp`](/reference/tooling/bynk-mcp/).

```text
bynk mcp [PATH]
```

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | `.` | The workspace the tools may read. A tool's `file` argument must be inside it. |

Eight tools: `bynk_get_diagnostics`, `bynk_check`, `bynk_explain_diagnostic`,
`bynk_describe_symbol`, `bynk_get_documentation`, `bynk_get_architecture`,
`bynk_get_wire_contract` and `bynk_search_book` (the same search as
[`bynk book --search`](#bynk-book)) — the same names and inputs as the VS Code
extension's language-model tools. All are read-only: nothing is written, emitted
or fetched.
Each call analyses the project from the files on disk, in-process, and answers
in compact Markdown with `path:line:column` locations.

Standard output carries protocol messages only; diagnostics about the server
itself go to standard error. The server runs until its input closes.

**Exit code** — `0` when the client closes the connection; non-zero if `PATH` is
not a readable directory.

---

## `bynkc` (removed)

`bynkc` is gone: the executable, the `bynkc` crate's shim and the `bynkc-<target>`
release asset. Use `bynk`: `bynk check`, `bynk fmt`, `bynk compile` and `bynk test`
take the same flags. `BYNK_BYNKC`, `BYNK_ALLOW_SKEW` and the hidden `--allow-skew`
flag are gone too: there is no separate compiler to pin or to drift from, so
passing `--allow-skew` is now an unknown-flag error. (`--accept-compatible-skew`
on `bynk deploy` is unrelated: it concerns the contracts between deployed
services, and is unchanged.)

---

## `bynk doctor`

Survey the toolchain — grouped by capability — and print the exact remedy for
anything missing. Documented in full in the guide [Check your environment with
`bynk doctor`](/reference/tooling/editor-and-tooling/doctor/).

```text
bynk doctor [PATH] [--only CAPABILITY] [--strict] [--format human|short|json]
```

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | `.` | Project directory, for project-local `node_modules/.bin` resolution. |
| `--only CAPABILITY` | — | Scope the check — and the exit code — to one of `compile`, `test`, `deploy`, `editor`, `build`. |
| `--strict` | — | Treat every warning (optional gaps, `npx` provisionability, minor skew) as a failure. For CI. |
| `--format` | `human` | `human` is a grouped table; `short` and `json` are the stable scriptable surface. |

**Exit code** — Bare `bynk doctor` is informational: it exits `0` unless the
compiler itself is unusable. `--only <capability>` gates on that capability; `--strict`
fails on any warning.

In a project that declares account resources (KV, R2, D1, queues, Vectorize,
Hyperdrive or a linked kind), the `deploy` block carries a `cloudflare api` row:
whether `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` are set. It never reads
their values. It is a warning at most, never a failure, so neither `bynk dev` nor
`--only deploy` stops on it.
