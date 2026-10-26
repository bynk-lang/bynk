---
title: "CLI (`bynk` driver)"
---
The **`bynk`** driver is the developer front-end — it **links the compiler
pipeline in-process** and orchestrates the Node toolchain (`bynk` is to `bynkc`
as `cargo` is to `rustc`). A fresh `cargo install bynk` is self-contained: it
compiles, checks, and formats without a separately-installed `bynkc`. This page
is the argument and exit-code reference for its subcommands. The pure-pipeline
binary [`bynkc`](/docs/cli/) (`compile`, `check`, `fmt`, `test`) survives for CI
and direct use.

The everyday commands — [`bynk check`](#bynk-check), [`bynk fmt`](#bynk-fmt), and
[`bynk test`](#bynk-test) — mirror their `bynkc` counterparts exactly (same flags,
same output, same exit codes), so the two are drop-in equivalent. `check` and
`fmt` run the pipeline in-process; `test` delegates to the `bynkc` the driver
resolves (see [Which `bynkc`?](#which-bynkc)), so an editor or developer inherits
the driver's resolution instead of locating the compiler themselves.

```text
bynk <command> [options]
```

| Command | What it does |
|---|---|
| [`bynk doctor`](#bynk-doctor) | Check whether your machine is ready to compile, test, and deploy. |
| [`bynk new`](#bynk-new) | Scaffold a new, runnable project. |
| [`bynk dev`](#bynk-dev) | Build the project and serve it locally with `wrangler dev`. |
| [`bynk deploy`](#bynk-deploy) | Provision each context's Cloudflare resources and deploy every Worker, in dependency order. |
| [`bynk check`](#bynk-check) | Type-check a file or project without writing output. |
| [`bynk fmt`](#bynk-fmt) | Format `.bynk` source files in place. |
| [`bynk test`](#bynk-test) | Discover and run a project's tests. |
| [`bynk fetch`](#bynk-fetch) | Resolve the project's dependencies, fetch git ones into the store, and write `bynk.lock`. |
| [`bynk approve`](#bynk-approve) | Approve the npm packages dependencies' adapters bring into the build. |
| [`bynk update`](#bynk-update) | Re-resolve git dependencies' revisions and rewrite `bynk.lock`. |
| [`bynk migrate`](#bynk-migrate) | Show where each agent class's stored state stands, and sweep deployed instances so old `migrate` steps can retire. |
| [`bynk add`](#bynk-add) | Add a registry dependency to `bynk.toml`, then resolve and lock it. |
| [`bynk publish`](#bynk-publish) | Check, pack and upload the package to its registry's publish API. |
| [`bynk yank`](#bynk-yank) | Yank (or un-yank) a published version through its registry's API. |
| [`bynk login`](#bynk-login) | Sign in to a registry's accounts with GitHub. |
| [`bynk logout`](#bynk-logout) | Sign out of a registry's accounts. |
| [`bynk org`](#bynk-org) | Claim and block an organisation, manage its members, and mint and revoke its publish tokens. |
| [`bynk registry`](#bynk-registry) | Build and maintain a package registry in a directory, and serve it with a publish API. |
| [`bynk explain`](#bynk-explain) | Explain a diagnostic code — what the rule is, why it exists, how to fix it. |

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
├── .gitignore           # /.bynk
├── .gitattributes       # *.bynk text eol=lf
└── src/
    └── <name>.bynk      # context <name> — a GET "/" HTTP service
```

**Behaviour** — `bynk new` is pure, offline file-writing: it shells nothing,
compiles nothing, and reads no network, so it works before `bynkc`, Node, or
`wrangler` are installed.

1. Derive the project name from `PATH`'s final component (or `--name`) and
   validate it as a legal Bynk identifier — both `[project] name` and the
   starter's context use it.
2. Refuse to clobber: if the target exists and is non-empty, fail before writing
   anything. An empty directory is fine; VCS/OS cruft (`.git`, `.gitignore`,
   `.gitattributes`, `.DS_Store`, …) doesn't count as non-empty.
3. Write the scaffold and print next steps (`cd <path> && bynk dev`).

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
`bynkc compile` + `cd` + `wrangler dev` recipe. See the guide [Run your project
locally](/book/guides/projects-build-and-deployment/run-locally/) for a worked
walkthrough.

```text
bynk dev [PATH] [--context NAME]... [--base-port N] [--inspect] [--inspect-port N] [--local-worker NAME]... [--format human|json] [--log-events] [--rewrite-frames] [--ready-json FILE] [-- <wrangler args>]
```

`bynk dev` serves one project, so it refuses to run at a
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces):
run it inside a member.

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | `.` | A directory inside the project. The root is found by walking up for `bynk.toml`. |
| `--context NAME` | every context | Which context to serve. **Repeatable**; omit to serve them all with their service bindings wired. Accepts the dotted name (`a.b`) or its worker-directory form (`a-b`). |
| `--base-port N` | `8787` | First port of the per-context allocation: context *i* is served on `N + i`, in the order listed. |
| `--inspect` | off | Serve with the V8 inspector enabled (`wrangler dev --inspector-port`) so a JavaScript debugger can attach. Breakpoints set in `.bynk` sources resolve through the emitted source maps, composed into the worker bundle. Prints an inspector URL per context on start. |
| `--inspect-port N` | `9229` | First inspector port, allocated per context exactly as `--base-port` is. Used under `--inspect`, and also whenever several contexts are served: wrangler always starts an inspector, so each context gets its own port rather than all racing for 9229. A range that overlaps the `--base-port` range is an error. |
| `--local-worker NAME` | none | A Worker outside the project, named by a [`Service` instance](/book/guides/effects-and-capabilities/call-a-foreign-worker/) (`@worker("NAME")`), to reach in your own locally running `wrangler dev` of it instead of the stand-in `bynk dev` binds by default. **Repeatable.** A name no served context binds is warned about. |
| `--format human\|json` | `human` | `human` is the readable stream on stderr. `json` adds one JSON lifecycle event per line on stdout (see [Lifecycle events](#lifecycle-events)), and moves `wrangler dev`'s own stdout to stderr so stdout carries only events. |
| `--log-events` | off | With `--format json`, also write each line a worker prints as a `log` event (see [Worker logs](#worker-logs)). |
| `--rewrite-frames` | off | Rewrite stack frames to `.bynk` locations even on a terminal (see [Worker logs](#worker-logs)). Implied by `--log-events`. |
| `--ready-json FILE` | none | Once every served context answers an HTTP probe, write `{ "<worker>": "<url>" }` to `FILE` (see [Readiness](#readiness)). A stale `FILE` is removed at start, and `FILE` is removed when the session stops. |
| `-- <wrangler args>` | — | Everything after `--` is forwarded to `wrangler dev` verbatim (e.g. `-- --var K:V`). Ports are the driver's to allocate: passing `-- --port` or `-- --inspector-port` against an allocation is an error naming the flag that owns it. |

**Behaviour**

1. Locate the project root and read `[paths] include`.
2. Pre-flight the `deploy` capability (`bynkc`, Node, `wrangler`) exactly as
   [`doctor`](#bynk-doctor) does; a missing required tool fails here, before any
   build, with doctor's remedy text.
3. Compile to the managed **`.bynk/dev/`** build directory (gitignored
   automatically; the `workers/` tree is cleared before each build).
4. Select the workers: every context, or the `--context` subset. An unknown
   name fails and lists the available contexts.
5. Run **one `wrangler dev` per context**, each from inside its own worker
   directory, in local mode (Miniflare) — **no namespace provisioning is
   needed** and `wrangler.toml` is served untouched. The processes discover each
   other through wrangler's dev registry and wire the generated `[[services]]`
   bindings between themselves, so a cross-context call resolves locally.
   A context that binds a Worker outside the project (a `Service` instance)
   gets a local stand-in for it in its own `wrangler dev`, passed as a second
   `-c` config and written under `.bynk/dev/stand-ins/`. It runs as
   `bynk-stand-in-<context>-<worker>`, so it never answers another local
   session's binding to the real Worker, and the context is served from
   `wrangler.dev.toml`, a copy of its `wrangler.toml` bound to the stand-in. It
   answers every request with `503` and a body naming itself, and is listed on
   start.
   `--local-worker NAME` drops it, so the dev registry connects the binding to
   your own `wrangler dev` of that Worker. `-- --remote` runs no stand-ins, and
   a `-- -c`/`--config` while stand-ins run is an error.
   When `wrangler` resolves via `npx` and several contexts are served, it is
   installed once (`npx --yes wrangler@4 --version`) before they start, so their
   first-run installs do not race in npm's cache.
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
  (project-local `node_modules/.bin` → `PATH` → `npx`). An `npx` resolution is
  surfaced as a notice — it downloads on first use.

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
for colour back with `FORCE_COLOR=1` when its own stderr is a terminal (and
`NO_COLOR` is unset).

With `--format json --log-events`, each line is also a `log` event (see
[Lifecycle events](#lifecycle-events)), classified by wrangler's own tags: a
line with `[ERROR]`, `✘`, `Uncaught` or an `…Error:` headline is `error`,
`[WARNING]` or `▲` is `warn`, and the rest `info`. A frame takes the level of
the line it continues. The VS Code extension reads these for its
[Bynk Logs](/docs/tooling/vscode-bynk/) view.

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

---

## `bynk deploy`

Provision the Cloudflare resources each context declares — KV namespaces, queues,
and Durable Object migrations — set its secrets, and deploy every context's
Worker, in Service-Binding dependency order. See [Deploy to
Cloudflare](/book/guides/projects-build-and-deployment/deploy-to-cloudflare/)
for the workflow.

```text
bynk deploy [PATH] [--context NAME] [--dry-run] [--format short|json] [--yes]
            [--secrets-file PATH] [--secret NAME]... [--force] [-- <wrangler args>]
```

`bynk deploy` deploys one project, so it refuses to run at a
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces):
run it inside a member. Each member keeps its own deploy state.

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | `.` | A directory inside the project. The root is found by walking up for `bynk.toml`. |
| `--context NAME` | every context | Deploy this context alone, assuming the contexts it consumes are already live. A dependency that has never been deployed is named and refused rather than pushed into — as is one that is live but no longer provides the contract this context was compiled against, and a build of this context that no longer provides what a live caller was compiled against (`bynk.deploy.contract_skew`; see [Contract skew](/book/guides/projects-build-and-deployment/contract-skew/)). Accepts the dotted name (`a.b`) or its worker-directory form (`a-b`). |
| `--dry-run`, `--plan` | off | Print the per-context plan and resolved order, then exit without changing Cloudflare or `bynk.deploy.lock`. Works offline — it never authenticates. |
| `--format FORMAT` | `short` | Plan output: line-oriented `short` or machine-readable `json`. |
| `--yes` | off | Skip the confirmation required before a resource is created or a Worker is published. Required for non-interactive calls. |
| `--secrets-file PATH` | — | Read secret **names and values** from a dotenv-style `NAME=value` file. Never committed, never persisted: values move to `wrangler secret put` and are dropped. |
| `--secret NAME` | — | Set this named secret, taking its **value** from the environment (or a prompt). Repeatable. Needed only for a `bynk.Secrets` name — an actor's declared `auth` secret needs no flag. |
| `--force` | off | Overwrite a secret that is already set. The default sets only the missing ones, so a re-deploy does not cut a fresh Cloudflare secret version for every secret every time. |
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
[TypeScript client](/book/reference/typescript-client/#pins)).

---

## `bynk check`

Type-check a `.bynk` file or project without writing output — the same behaviour
as [`bynkc check`](/docs/cli/#bynkc-check), through the driver. Runs the compiler
pipeline **in-process**: no separately-installed `bynkc` is required.

```text
bynk check [INPUT] [--format rich|short] [--locked] [--offline]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | A `.bynk` file, or a project root directory (a `bynk.toml` or `src/` subdir selects project mode; otherwise the directory is itself the source tree). |
| `--format` | `rich` | `rich` is the source-context rendering; `short` emits one terse `path:line:col: severity[category]: message` line per diagnostic, for the VS Code problem-matcher, CI, and scripts. In both, `path` is the file as you'd type it from the working directory (the input you passed, joined with the file's place in it), the same path `fmt` reports. `rich` is coloured only when stderr is a terminal and `NO_COLOR` is unset or empty; `FORCE_COLOR` or `CLICOLOR_FORCE` set to anything but empty or `0` colours it without a terminal (for `less -R` or a CI log); `0` doesn't turn colour off on a terminal, `NO_COLOR` does, and it wins over both. `rich` cuts a source line longer than 400 bytes to a window around its labels, marked `…`. |

**Exit code** — `0` when the input type-checks (warnings are surfaced but do not
fail the build, per the [diagnostics rule](/docs/cli/#exit-codes-and-diagnostics));
non-zero on any error-severity diagnostic.

**At a workspace root** — `INPUT` naming a
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces)
checks every member, in path order. `rich` heads each member's report with a
`── packages/money ──` line; `short` adds no headers, and its paths are relative
to the working directory, so they stay unambiguous across members. Every member
is checked even when an earlier one fails, and the exit code is non-zero if any
member fails.

---

## `bynk fmt`

Format `.bynk` source files in place — the same behaviour as
[`bynkc fmt`](/docs/cli/#bynkc-fmt), through the driver, run **in-process**.

```text
bynk fmt <INPUTS>... [--check] [--indent tab|spaces] [--indent-width N] [--max-line-width COLUMNS] [--trailing-comma|--no-trailing-comma] [--no-config]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUTS` | *(required)* | Files or directories to format. A directory formats the `.bynk` files [`bynk check`](#bynk-check) reads for it: a project root's `[paths] include` trees minus `exclude`, or any other directory walked recursively (hidden directories skipped). Pass `-` to read from stdin and write the formatted result to stdout. |
| `--check` | off | Report files that are not already canonically formatted **without writing changes**. Exits non-zero if any file would change. For CI. |
| `--indent` | `[fmt] indent`, else `tab` | Indent with tabs or spaces. Tabs are the default so each reader sets their own width in their editor. |
| `--indent-width N` | `[fmt] indent_width`, else `2` | Spaces per nesting level. Rejected when the run resolves to tabs, where it would have no effect. |
| `--max-line-width COLUMNS` | `[fmt] max_line_width`, else `100` | Soft target line width. A construct wider than this wraps across lines where the grammar allows; one with no break point in it (a long string literal) is left long. |
| `--no-trailing-comma` | off | Omit the trailing comma in multi-line records, sums, list literals and `exports` clauses. `--trailing-comma` is its opposite (and overrides a project's `trailing_comma = false`); the later of the two wins. |
| `--no-config` | off | Ignore the project's `bynk.toml` `[fmt]` section and format to the canonical style plus whatever flags this run passes. |

**Behaviour** — each file is formatted and rewritten only when it changes; a file
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
of a sum type moves onto its own line above the first variant.

**Where the style comes from** — three layers, each overriding the one before:
the canonical defaults, then the project's `[fmt]` section in
[`bynk.toml`](/docs/manifest/), then the flags this run passes. The manifest is
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

Discover and run a project's test declarations — the same behaviour as
[`bynkc test`](/docs/cli/#bynkc-test), through the driver. Unlike `check` and
`fmt`, `test` **delegates** to the resolved `bynkc` (it orchestrates external
`tsc`/`node` anyway), forwarding every flag verbatim. Requires `tsc` (with
Node.js) or `tsx` on `PATH`, exactly as `bynkc test`.

```text
bynk test [INPUT] [-o OUTPUT] [--no-run] [--format rich|json|ndjson] [--inspect] [--seed HEX] [--case NAME]... [--suite NAME]... [--coverage] [--against TARGET [--skip-unsupported] [--settle DURATION]] [--allow-skew]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | Project root directory. |
| `-o, --output OUTPUT` | `<input>/out` | Where to write the compiled TypeScript test-runner modules. |
| `--no-run` | off | Skip the runner. With `--format rich`, emit the generated test files; with `--format json`, emit a discovery document listing every suite and case without running them. |
| `--format` | `rich` | `rich` is the grouped ✓ / ✗ human output; `json` is a single pinned JSON document of results, for tooling; `ndjson` is the same results as a stream of one JSON event per line, written as each suite finishes (see [Streaming results](#streaming-results)). `ndjson` is refused with `--coverage` and `--no-run`, and across a workspace. |
| `--inspect` | off | Launch the runner under Node's inspector (`node --inspect-brk`) and print the inspector URL. Requires Node ≥ 22.18 (or ≥ 23.6 unflagged). Does not run `tsc`. |
| `--seed HEX` | random | Root seed for generative `property` tests (e.g. `0x5f3a`). A failing property prints the seed it used; re-running with `--seed <hex>` reproduces the run byte-for-byte. |
| `--case NAME` | — | Run only the test cases named `NAME`, across suites — the filter behind the editor's per-case *▷ Run Test* lens. Repeat the flag to run several cases in one process. See [Selecting tests](#selecting-tests). No effect with `--no-run`. |
| `--suite NAME` | — | Run only the cases of the suite `NAME`, as the JSON document reports it (an integration suite without its prefix). Repeat the flag to select several suites. See [Selecting tests](#selecting-tests). No effect with `--no-run`. |
| `--coverage` | off | After the run, report statement/line coverage attributed to `.bynk` source (rich table, or a `coverage` block under `--format json`). Requires the `tsc → node` path — incompatible with `--inspect` and `--no-run`. |
| `--against TARGET` | — | Run the `system` suites against a real runtime instead of in process: `dev`, a local `bynk dev` the runner starts itself, or a target named under [`[test.targets]`](/docs/manifest/#test) in `bynk.toml`. See [Running against a deployment](#running-against-a-deployment). Incompatible with `--inspect` and `--coverage`. |
| `--skip-unsupported` | off | With `--against`, skip each `system` case a real runtime can't honour, naming it and why, instead of refusing the run. |
| `--settle DURATION` | `5s` | With `--against`, how long a failing `expect` is retried before it fails, replaying the reads before it (see [Waiting for a real system](#waiting-for-a-real-system)). `500ms`, `5s`, `2m`, or `0` for no retry. |
| `--allow-skew` | off | Run even when the resolved `bynkc` is a different **major** version from `bynk` (see [skew](#version-skew)). Also settable as `BYNK_ALLOW_SKEW=1`. |

**Exit code** — follows the runner's own process status: `0` when every case
passed, non-zero on a failing case, a `--case` or `--suite` value that selected
nothing, a compile error, or a missing runner. It is
also non-zero, without running anything, when the resolved `bynkc` is majorly
skewed from `bynk` and skew isn't allowed.

**At a workspace root** — `INPUT` naming a
[workspace root](/book/guides/projects-build-and-deployment/layout/#workspaces)
tests every member in path order, printing a `── packages/money ──` line before
each member's output. Every member runs, and the exit code is non-zero if any
fails. `--format json`, `--output` and `--against` each belong to one project,
so they are refused at a root: run them inside a member.

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
  (`bynkc test: no case matched --case "typo"`, `no suite named --suite "x"`) and
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

### Streaming results

`--format ndjson` writes one JSON event per line as the run produces them, so a
consumer sees each suite when it finishes instead of everything at the end. It
carries the same information as `--format json`, which stays the single document
and the right choice for CI that wants one result.

| Event | Fields, in order |
|---|---|
| `run-begin` | `type`, `suites` |
| `suite-begin` | `type`, `name`, `kind`, `tests` |
| `case` | `type`, `suite`, then the document's case object: `name`, `outcome`, `message`, `location`, `retries`, `tier`, `kind`, `seed`, `counterexample`, `violation`, `refusals`, each omitted when absent, then `durationMs` (omitted for a skipped case) |
| `suite-end` | `type`, `name` |
| `run-end` | `type`, `passed`, `failed`, `skipped` and `transport` (a remote run only) |
| `error` | `type`, then the document's `error` object: `kind` (`compile` or `runtime`), `message`, `diagnostics`, `stderr` |

- The stream ends with **exactly one** of `run-end` or `error`. A project that does
  not compile is a single `error` with `kind: "compile"`; a runner that dies before
  `run-end` ends the stream with an `error` with `kind: "runtime"` and its stderr;
  a failing case is not an error, and the exit code is non-zero for it as before.
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
an `if` or `match` around an address, a `~>` send, or a store write. Only
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
- **Only through `bynk`.** `bynkc test --against dev` is refused: only the
  `bynk` driver can start `bynk dev`.

Under `--format json` the groups are reported as one document, with
`"transport": "remote:dev"`.

### Running against a deployment

`--against <name>` runs the same `system` cases, unchanged, against Workers
already running somewhere: a preview deployment, or anything else that serves
them. Each driver's request goes to the Worker's URL from
[`[test.targets.<name>]`](/docs/manifest/#test) instead of the in-process
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
  case creates by [`run.id`](/book/reference/testing/#run-id), which is fresh
  for each `--against` run, so a second run doesn't meet the first run's data.
- **The clock is the Workers' own.** In process the harness clock starts at the
  epoch; a real runtime's Workers read wall-clock time, and the runner can't
  move it. A `system` case whose `expect` compares an `Instant` built from a
  constant (`Instant.fromEpochMillis(0)`) gets a
  `bynk.test.remote_clock_dependent` warning: it can pass in process and fail
  against a real runtime, and no `--settle` retry fixes that.

Under `--format json` the document gains `"transport": "remote:<name>"`, and
a case that `--settle` retried carries `"retries"`.

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

## Dependencies: `bynk.lock`, `--locked` and `--offline`

Before `bynk check`, `bynk test`, `bynk dev` and `bynk deploy` compile, they
resolve the project's [`[dependencies]`](/docs/manifest/#dependencies) (and
`[dev-dependencies]`) against [`bynk.lock`](/docs/manifest/#bynklock), fetch any
git dependency missing from the store, re-check every fetched dependency
against its locked checksum, and rewrite the lock if the dependencies changed.
In a workspace, the members share one lock at the workspace root.

| Flag | Meaning |
|---|---|
| `--locked` | Fail (`bynk.package.lock_out_of_date`) instead of changing `bynk.lock`: build exactly what was committed. |
| `--offline` | Fail (`bynk.package.not_fetched`) instead of fetching: every git dependency must already be in the store. |

Both flags are accepted by `check`, `test`, `dev`, `deploy` and `fetch`. CI
typically runs `bynk fetch` (or restores a cached store) and then
`bynk check --locked --offline`. `bynkc` never fetches or writes the lock: a
dependency missing from the store is one `bynk.package.not_fetched` diagnostic.

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
bynk fetch [PATH] [--locked] [--offline]
```

Resolve, fetch and lock, and nothing else: warm the store before an
`--offline` step, or update `bynk.lock` after editing `[dependencies]`. Prints
each package it fetched and whether it wrote the lock. **Exit code** — `0` on
success; non-zero on any resolution, fetch or checksum failure.

When the lock it writes has npm packages awaiting approval, `fetch` (like
`add` and `update`) asks about each on a terminal, and otherwise lists them
with the command to run; it succeeds either way.

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

**Exit code:** `0` when every selected instance woke; non-zero otherwise.

## `bynk add`

```text
bynk add <ORG/PACKAGE>[@RANGE] [--as ALIAS] [--path PATH]
```

Add a registry dependency: the highest version the package's registry offers
(or one matching `RANGE`), written to `[dependencies]` as
`<alias> = { package = "…", version = "^<version>" }` without reformatting the
rest of `bynk.toml`, then resolved and locked like `bynk fetch`. The alias
defaults to the package segment; an alias already in `[dependencies]` is
refused.

## `bynk publish`

```text
bynk publish [--path DIR] [--dry-run]
```

Publish the package at `DIR` (default: the project holding the current
directory) to the registry its organisation routes to in `[registries]`. It
resolves and checks the package first — a package that doesn't check isn't
published — refuses one without a `version` or with a `path`/`git` dependency,
packs it, and uploads it to the registry's [publish API](/docs/registry-protocol/#publish-api).
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
nothing. A refusal is `bynk.registry.publish_refused`, with the registry's
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
and the [Registry protocol](/docs/registry-protocol/).

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
| `init` | Writes `<DIR>/config.json` and a new ed25519 signing key (default `./<ID>.key`, refused inside `<DIR>`), and prints the `key = "…"` line consumers pin. |
| `add` | Signs with the newest chain key or a valid, unrevoked delegate. Packs the package (its sources; never `out/`, `node_modules/` or hidden files), signs its index line with the registry's current key, and appends it. A version already present with a different tree, or a `path`/`git` dependency, is refused; the identical tree again changes nothing. |
| `yank` | Marks a version yanked (`--undo` clears it): kept for locked builds, never newly chosen. Nothing is deleted or re-signed. `--undo` refuses a withdrawn version. |
| `withdraw` | [Withdraws](/docs/registry-protocol/#withdrawals) a version — a takedown: yanks its line, records `--reason` (one line, at most 500 characters) as its `withdrawn`, and deletes its archive, unless another version names the same tree (kept, with a warning). Nothing is re-signed. Re-run after a failure, it finishes deleting the archive. Final: it otherwise refuses a version already withdrawn, and `add`, the publish API and `yank --undo` then refuse that version too. |
| `serve` | Serves `<DIR>` over HTTP with the [publish API](/docs/registry-protocol/#publish-api) at `/api` (recorded as `"api"` in `config.json`), signing uploads with `--key` and authorising them with `--tokens`. It binds `127.0.0.1:8730` by default and handles one request at a time, so writes never interleave. It speaks plain HTTP: to serve beyond the machine, put a TLS-terminating proxy in front. The key and tokens files must be outside `<DIR>`. |
| `rotate` | Appends a link to the [key chain](/docs/registry-protocol/#trust): a new key (written to `--key-out`, refused inside `<DIR>`), signed by the current chain key. Clients pinned to an older key follow the chain, and every valid delegation and revocation is re-signed with the new key, so a client pinned to it accepts them too. |
| `delegate` | Creates a key that may sign index lines and nothing else, delegated by the newest chain key (so every client, whichever link it trusts, accepts it); use it as `serve`'s or `add`'s `--key` and keep the chain key offline. Makes the config protocol 2. |
| `revoke` | Revokes a delegate, signed by the newest chain key. Refuses when a line would be left with no valid signer — `resign` first — unless `--force`. |
| `resign` | Re-signs every index line with the given key (the newest chain key or a valid delegate); only each line's `sig` changes. A line no key the registry knows signed (chain, delegate, or revoked delegate) is refused, never laundered. Don't run it while a server is publishing to the same directory: a line appended mid-run would be lost. |
| `conformance` | Checks a registry against the [protocol](/docs/registry-protocol/#conformance): each check passes, fails or is skipped, and any failure is `bynk.registry.nonconformant` (exit 1). A full run publishes one throwaway package under `--org`, using the token for `--name` (default `conformance`, so `BYNK_REGISTRY_CONFORMANCE_TOKEN`), and leaves it yanked; `--read-only` publishes nothing. |
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
pointer to the [diagnostic index](/book/reference/diagnostics/) rather than a
full explanation. The same compiler-owned mapping powers the editor's clickable
diagnostic-code links (`codeDescription`) and the language server's
`bynk/explain` request, behind VS Code's **Explain** quick fix, so the CLI and
the editor never disagree.

**Exit code** — `0` for any code the compiler recognises (explained or not);
non-zero for an unrecognised code.

---

## Which `bynkc`?

`bynk` locates the compiler it needs — for `test`, and for the `check`/`fmt`
escape hatch below — in this order:

1. the **`BYNK_BYNKC`** environment variable, if set (an explicit pin);
2. **`bynkc` on `PATH`**;
3. a **`bynkc` sibling** of the running `bynk` binary (how a paired install ships).

`bynk check` and `bynk fmt` run in-process and need none of this — **unless**
`BYNK_BYNKC` is set, in which case they shell that pinned compiler so an
externally-managed `bynkc` still governs the result. `bynk doctor` reports this
resolution and any driver↔compiler version skew.

### Version skew

When `bynk` is about to run a separate `bynkc` (`bynk test` always does, and
`bynk check`, `fmt`, `dev` and `deploy` do under `BYNK_BYNKC`), it compares
versions first, as `bynk doctor` does. A patch difference is ignored. A
**minor** difference prints a warning (once per run, so a `bynk dev` session
doesn't repeat it on every rebuild) and runs anyway. A **major** difference
refuses to run, since the two don't share a contract. Pass `--allow-skew` to
`bynk test`, or set `BYNK_ALLOW_SKEW=1` for any of these commands, to run anyway
with a warning. The usual
cause is a `cargo install bynkc` on `PATH` beside a different `bynk`. Align
them, or pin one with `BYNK_BYNKC`.

---

## `bynk doctor`

Survey the toolchain — grouped by capability — and print the exact remedy for
anything missing. Documented in full in the guide [Check your environment with
`bynk doctor`](/docs/editor-and-tooling/doctor/).

```text
bynk doctor [PATH] [--only CAPABILITY] [--strict] [--format human|short|json]
```

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | `.` | Project directory, for project-local `node_modules/.bin` resolution. |
| `--only CAPABILITY` | — | Scope the check — and the exit code — to one of `compile`, `test`, `deploy`, `editor`, `build`. |
| `--strict` | — | Treat every warning (optional gaps, `npx` provisionability, minor skew) as a failure. For CI. |
| `--format` | `human` | `human` is a grouped table; `short` and `json` are the stable scriptable surface. |

**Exit code** — Bare `bynk doctor` is informational: it exits `0` unless `bynkc`
itself is unusable. `--only <capability>` gates on that capability; `--strict`
fails on any warning.
