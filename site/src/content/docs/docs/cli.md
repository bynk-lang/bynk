---
title: "CLI (`bynkc`)"
---
<!-- GENERATED FILE — do not edit by hand.
     Source: bynkc/src/cli.rs (`render_markdown`).
     Regenerate with: BYNK_BLESS=1 cargo test -p bynkc --test cli_reference -->

The Bynk compiler

Run `bynkc <command> --help` for the authoritative help text.

## Exit codes and diagnostics

A diagnostic's **severity** decides whether it fails a build (v0.89). An **`Error`** rejects the program: `bynkc compile`/`check` exit non-zero and produce no output. A **`Warning`** is surfaced but does **not** fail the build: these commands still **succeed (exit 0)** and emit their output, with warnings reported alongside. The build-failure gate counts error-severity diagnostics only. See the normative rule in the [specification](/book/spec/diagnostics/) and the [diagnostic index](/book/reference/diagnostics/) (warning-severity codes are marked *(warning)*).

## `bynkc check`

Type-check a `.bynk` file or project without writing output

```text
bynkc check <INPUT> [--format <FORMAT>]
```

| Argument | Required | Default | Description |
|---|---|---|---|
| `INPUT` | yes | — | Input `.bynk` file or project root. A workspace root checks every member in turn |
| `--format` | no | `rich` | Diagnostic output format. `rich` (default) is the ariadne source-context rendering; `short` emits one terse `path:line:col: severity[category]: message` line per diagnostic, for tooling (the VS Code problem-matcher, CI, scripts). In both, `path` is the file as you'd type it from the working directory (the input you passed, joined with the file's place in it), the same path `fmt` reports (one of: rich, short) |

## `bynkc compile`

Compile a `.bynk` file (single-file commons) to a TypeScript file, or a directory project to a tree of TypeScript files mirroring the source layout

```text
bynkc compile <INPUT> --output <OUTPUT> [--target <TARGET>] [--platform <PLATFORM>] [--emit <EMIT>]
```

| Argument | Required | Default | Description |
|---|---|---|---|
| `INPUT` | yes | — | Input `.bynk` file, or directory project root. A workspace root is refused: compile each member |
| `--output` (`-o`) | yes | — | Output `.ts` file (for single-file input) or output root directory (for project input) |
| `--target` | no | `bundle` | Build target. `bundle` (default) produces a single deployment unit; `workers` produces one Cloudflare Worker per context with Service Binding plumbing (v0.8) (one of: bundle, workers) |
| `--platform` | no | `cloudflare` | Deploy platform selecting the `bynk` surface binding (v0.17). A new axis, distinct from `--target`: `cloudflare` (default), `node`, or `browser` (the in-browser playground binding; `Bundle` topology only) (one of: cloudflare, node, browser) |
| `--emit` | no | `ts` | Artefact language (v0.108). `ts` (default) writes typed TypeScript; `js` writes the same modules with types stripped — a JavaScript artefact that runs with no `tsc` in the loop (ADR 0137) (one of: ts, js) |

## `bynkc fmt`

Format `.bynk` source files in place. Passing `-` reads from stdin and writes to stdout

```text
bynkc fmt [INPUTS] [--check] [--indent <INDENT>] [--indent-width <N>] [--max-line-width <COLUMNS>] [--trailing-comma] [--no-trailing-comma] [--no-config]
```

| Argument | Required | Default | Description |
|---|---|---|---|
| `INPUTS` | no | — | Files or directories to format. A directory formats the `.bynk` files `check` would read in it: a project root's `[paths] include` trees minus `exclude`, a workspace root's members, or any other directory walked recursively. Use `-` for stdin → stdout |
| `--check` | no | — | Check formatting without writing changes. Exits non-zero if any file is not already canonical |
| `--indent` | no | — | Indent with tabs or spaces. Defaults to the project's `[fmt] indent`, or tabs (one of: tab, spaces) |
| `--indent-width` | no | — | Spaces per nesting level, with spaces indentation. Defaults to the project's `[fmt] indent_width`, or 2. Rejected when the effective indentation is tabs, where it would have no effect |
| `--max-line-width` | no | — | Soft target line width in columns. A construct wider than this wraps across lines where the grammar allows; one with no break point in it (a long string literal) is left long. Defaults to the project's `[fmt] max_line_width`, or 100 |
| `--trailing-comma` | no | — | Emit a trailing comma in multi-line records, sums, list literals and `exports` clauses. Overrides a project's `trailing_comma = false`, and overrides an earlier `--no-trailing-comma` |
| `--no-trailing-comma` | no | — | Omit the trailing comma in multi-line records, sums, list literals and `exports` clauses. (Parameter and argument lists never carry one — the grammar rejects it — regardless of this flag.) |
| `--no-config` | no | — | Ignore the project's `bynk.toml` `[fmt]` section and format to the canonical style, plus whatever flags this run passes. For a script that wants one fixed rendering whatever project it is pointed at |

## `bynkc test`

Discover and run test declarations in a project. Compiles the project (including all generated `tests/*.test.ts` modules), then invokes Node.js on the aggregated runner script. Requires `tsc` and `node` to be on PATH

```text
bynkc test [INPUT] [--output <OUTPUT>] [--no-run] [--format <FORMAT>] [--inspect] [--seed <SEED>] [--case <NAME>] [--suite <NAME>] [--partial-selection] [--coverage] [--against <TARGET>] [--skip-unsupported] [--settle <DURATION>]
```

| Argument | Required | Default | Description |
|---|---|---|---|
| `INPUT` | no | `.` | Input project root directory. Defaults to the current directory |
| `--output` (`-o`) | no | — | Where to write compiled TypeScript test runner modules. Defaults to `<input>/out` |
| `--no-run` | no | — | Skip the runner invocation. With `--format rich` this emits the generated test files (for CI flows that drive the runner separately); with `--format json` it emits a discovery document listing every suite and case (each `outcome: "discovered"`) without running them — a pure compile, no `tsc`/Node |
| `--format` | no | `rich` | Output format. `rich` (default) is the grouped ✓ / ✗ human output; `json` is a single pinned JSON document of results, for tooling; `ndjson` is the same results as one JSON event per line, written as each suite finishes. `ndjson` cannot be combined with `--coverage` or `--no-run` (one of: rich, json, ndjson) |
| `--inspect` | no | — | Compile a debug build and launch the test runner under Node's inspector (`node --inspect-brk`), printing the inspector URL for a JavaScript debugger to attach. The emitted `.ts` runs directly under Node's line-preserving type-stripping, so source maps resolve breakpoints back to `.bynk`. Requires Node ≥ 22.18 (or ≥ 23.6 unflagged). Does not run `tsc` |
| `--seed` | no | — | The root seed for generative `property` tests, as hex (e.g. `0x5f3a`). A failing property prints the seed it used; re-running with `--seed <hex>` reproduces that run byte-for-byte. Omitted, each run draws a fresh random seed |
| `--case` | no | — | Run only the test cases named `<name>`, skipping the rest — the filter behind the editor's per-case `▷ Run Test` lens. Matches by exact case name across suites. Repeat the flag to run several cases in one process; values of the same flag combine as "or", and with `--suite` as "and". A name that selects no case fails the run. Omitted, every case runs. No effect with `--no-run` (discovery lists all cases regardless) |
| `--suite` | no | — | Run only the cases of the suite named `<name>`, as the JSON document reports it (`suites[].name`, without an integration suite's prefix); any suite kind matches. Repeat the flag to select several suites. A suite that is not selected is left out of the output entirely. A name that selects no suite fails the run. No effect with `--no-run` |
| `--partial-selection` | no | — | For a caller that runs one selection as several processes (`bynk test` across a workspace's members, `--against dev` across `env` groups): this run sees only part of the `--case`/`--suite` selection, so a value it selects nothing for is not an error here. The caller judges the values |
| `--coverage` | no | — | After the suite runs, report statement/line coverage attributed to `.bynk` source (a rich summary table, or a `coverage` block in `--format json`). Requires the `tsc → node` path: incompatible with `--inspect` and `--no-run`, and errors if only `tsx` is available |
| `--against` | no | — | Run the `system` suites against a real runtime instead of in process: `dev`, a local `bynk dev` the runner starts itself (through `bynk test` only), or a target named under `[test.targets]` in `bynk.toml`. A bare URL is refused, and so is a target marked `production = true`. A named target's secrets are read from the environment. Only the `system` suites run, and a case that uses what a real runtime can't honour (an `on call` address, `advance Clock`, a cron or queue address, an asset fetch) is refused unless `--skip-unsupported` is passed |
| `--skip-unsupported` | no | — | With `--against`, skip each `system` case a real runtime can't honour, naming it and why, instead of refusing the run |
| `--settle` | no | `5s` | With `--against`, how long a failing `expect` is retried before it fails, for each retried stretch: the addresses since the last unsafe one (anything but `GET`, `HEAD` or `OPTIONS`) are re-run before each retry, so a real system's latency doesn't fail a case. A duration such as `5s`, `500ms` or `0` (no retry) |
