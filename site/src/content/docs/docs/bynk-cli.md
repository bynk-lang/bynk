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
bynk dev [PATH] [--context NAME]... [--base-port N] [--inspect] [--inspect-port N] [--local-worker NAME]... [--format human|json] [-- <wrangler args>]
```

| Argument | Default | Meaning |
|---|---|---|
| `PATH` | `.` | A directory inside the project. The root is found by walking up for `bynk.toml`. |
| `--context NAME` | every context | Which context to serve. **Repeatable**; omit to serve them all with their service bindings wired. Accepts the dotted name (`a.b`) or its worker-directory form (`a-b`). |
| `--base-port N` | `8787` | First port of the per-context allocation: context *i* is served on `N + i`, in the order listed. |
| `--inspect` | off | Serve with the V8 inspector enabled (`wrangler dev --inspector-port`) so a JavaScript debugger can attach. Breakpoints set in `.bynk` sources resolve through the emitted source maps, composed into the worker bundle. Prints an inspector URL per context on start. |
| `--inspect-port N` | `9229` | First inspector port, allocated per context exactly as `--base-port` is. Used under `--inspect`, and also whenever several contexts are served: wrangler always starts an inspector, so each context gets its own port rather than all racing for 9229. A range that overlaps the `--base-port` range is an error. |
| `--local-worker NAME` | none | A Worker outside the project, named by a [`Service` instance](/book/guides/effects-and-capabilities/call-a-foreign-worker/) (`@worker("NAME")`), to reach in your own locally running `wrangler dev` of it instead of the stand-in `bynk dev` binds by default. **Repeatable.** A name no served context binds is warned about. |
| `--format human\|json` | `human` | `human` is the readable stream on stderr. `json` adds one JSON lifecycle event per line on stdout (see [Lifecycle events](#lifecycle-events)), and moves `wrangler dev`'s own stdout to stderr so stdout carries only events. |
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
bynk check [INPUT] [--format rich|short]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | A `.bynk` file, or a project root directory (a `bynk.toml` or `src/` subdir selects project mode; otherwise the directory is itself the source tree). |
| `--format` | `rich` | `rich` is the source-context rendering; `short` emits one terse `path:line:col: severity[category]: message` line per diagnostic, for the VS Code problem-matcher, CI, and scripts. In both, `path` is the file as you'd type it from the working directory (the input you passed, joined with the file's place in it), the same path `fmt` reports. `rich` is coloured only when stderr is a terminal and `NO_COLOR` is unset or empty; `FORCE_COLOR` or `CLICOLOR_FORCE` set to anything but empty or `0` colours it without a terminal (for `less -R` or a CI log); `0` doesn't turn colour off on a terminal, `NO_COLOR` does, and it wins over both. `rich` cuts a source line longer than 400 bytes to a window around its labels, marked `…`. |

**Exit code** — `0` when the input type-checks (warnings are surfaced but do not
fail the build, per the [diagnostics rule](/docs/cli/#exit-codes-and-diagnostics));
non-zero on any error-severity diagnostic.

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
`.bynk` file is an error, so a mistyped path cannot pass `--check`.

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
line to attach it, or make it a `--` comment. A `--` comment on the same line as
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
bynk test [INPUT] [-o OUTPUT] [--no-run] [--format rich|json] [--inspect] [--seed HEX] [--case NAME] [--coverage] [--against TARGET [--skip-unsupported]] [--allow-skew]
```

| Argument | Default | Meaning |
|---|---|---|
| `INPUT` | `.` | Project root directory. |
| `-o, --output OUTPUT` | `<input>/out` | Where to write the compiled TypeScript test-runner modules. |
| `--no-run` | off | Skip the runner. With `--format rich`, emit the generated test files; with `--format json`, emit a discovery document listing every suite and case without running them. |
| `--format` | `rich` | `rich` is the grouped ✓ / ✗ human output; `json` is a single pinned JSON document of results, for tooling. |
| `--inspect` | off | Launch the runner under Node's inspector (`node --inspect-brk`) and print the inspector URL. Requires Node ≥ 22.18 (or ≥ 23.6 unflagged). Does not run `tsc`. |
| `--seed HEX` | random | Root seed for generative `property` tests (e.g. `0x5f3a`). A failing property prints the seed it used; re-running with `--seed <hex>` reproduces the run byte-for-byte. |
| `--case NAME` | — | Run only test cases whose name matches `NAME` — the filter behind the editor's per-case *▷ Run Test* lens. No effect with `--no-run`. |
| `--coverage` | off | After the run, report statement/line coverage attributed to `.bynk` source (rich table, or a `coverage` block under `--format json`). Requires the `tsc → node` path — incompatible with `--inspect` and `--no-run`. |
| `--against TARGET` | — | Run the `system` suites against a deployment instead of in process: a target named under [`[test.targets]`](/docs/manifest/#test) in `bynk.toml`. See [Running against a deployment](#running-against-a-deployment). Incompatible with `--inspect` and `--coverage`. |
| `--skip-unsupported` | off | With `--against`, skip each `system` case a real runtime can't honour, naming it and why, instead of refusing the run. |
| `--allow-skew` | off | Run even when the resolved `bynkc` is a different **major** version from `bynk` (see [skew](#version-skew)). Also settable as `BYNK_ALLOW_SKEW=1`. |

**Exit code** — follows the runner's own process status: `0` when every case
passed, non-zero on a failing case, a compile error, or a missing runner. It is
also non-zero, without running anything, when the resolved `bynkc` is majorly
skewed from `bynk` and skew isn't allowed.

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
  case that relies on starting empty may not hold the second time.

Under `--format json` the document gains `"transport": "remote:<name>"`.

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
