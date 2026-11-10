# Benchmarks

Compile-time and editor-latency measurements ([#3168](https://github.com/bynk-lang/bynk/issues/3168)),
one `v<version>.json` per release. Not a per-PR gate: shared CI runners are too
noisy for a threshold, so a regression is read by comparing releases.

Record one with:

```sh
cargo xtask bench --record                    # sizes 10,50,200 by default
cargo xtask bench --sizes 10,50,200,500 --record
```

It builds the release `bynk` and `bynk-lsp`, generates a synthetic project of N
independent contexts (each with three groups of a refined type, a record, an
agent with two handlers and an http service) under `target/bench-project-N`, and
reports, per size, median / p95 / max in milliseconds:

| Metric | What it times |
| ------ | ------------- |
| `cli_check` | `bynk check`, a fresh process each run (cold compiler caches) |
| `cli_recheck` | the same after a one-line edit; nothing survives between runs today |
| `lsp_first_round` | open one file; wait for the server's first project-wide diagnostics |
| `lsp_edit` | a one-line edit in the open buffer to its diagnostics arriving, warm: **edit-to-diagnostics** |

The LSP runs use `[lsp] diagnostics_debounce_ms = 0`, so `lsp_edit` is compute
time; the default 300 ms debounce is added on top for a user. Each edit toggles
a field type between valid and unknown so that every round changes the
published diagnostics (the server publishes only changes). Run it on a quiet
machine: the `host` block records the CPU so records from different hardware
are not compared by accident. The `keystroke_latency` row of
`design/greenfield-status.md` reads the newest record.

A record measures the tree at its `commit`, while the workspace was at `version`
(the file is named for the latter): it is not the released binary of that
version. The default `--sizes` is `10,50,200` for a quick run; pass
`--sizes 10,50,200,500` for a release record, so the 500-context row (which the
`keystroke_latency` reading shows) is not dropped.

## The two shapes

A record holds two project shapes (`--shape independent|linked|both`, both by
default):

- **independent** (`sizes`): N unrelated contexts with small handler bodies. The
  budget below is stated on this shape.
- **linked** (`linked_sizes`): contexts that consume each other, and larger
  handler bodies (a cross-context call, branching, a filter, a map and a sum per
  handler). Context `i` consumes context `(i-1)/4`, a 4-ary tree, so the
  contexts near the root are consumed by many and a project of N contexts is
  about log4(N) consumes-levels deep. This is closer to a real project than the
  independent shape; it is still synthetic.

Both edit `ctx0` (the root of the linked tree, so its dependents are the most
affected ones).

## The budget

The accepted editor-latency budget (#3168): the language server's
**edit-to-diagnostics p95 stays under 100 ms on the 50-context project**, with
the debounce excluded. It is checked against each release record, not gated in
CI. `cargo xtask bench` prints a warning when the 50-context p95 reaches it, and
the `keystroke_latency` row of `design/greenfield-status.md` states whether the
newest record meets it. A record that breaches the budget is the trigger for the
incremental-scheduler decision, [#1523](https://github.com/bynk-lang/bynk/issues/1523).
The 50-context size must be in a record for the budget to be checked, which the
default `--sizes` includes.

The run-time cost of compiled Bynk (bundle size, start-up and request latency
against hand-written TypeScript twins, [#3177](https://github.com/bynk-lang/bynk/issues/3177))
is recorded separately, in [`runtime/`](runtime/), with `cargo xtask bench-runtime
--record`. See [`benchmarks/runtime/`](../../benchmarks/runtime/README.md).

## Per-level attribution

The language-server runs set `BYNK_PHASE_TIMING=1`, which makes the server write
one line per analysis round to stderr (`bynk-check/src/phase_timing.rs`; off and
free when the variable is unset). The harness reads it and reports, as medians of
the timed edits:

| Field | What it holds |
| ----- | ------------- |
| `lsp_edit_levels` | `file`, `unit`, `definition`, `project`: where a warm edit's analysis time goes, by R3.13's query levels |
| `lsp_first_round_levels` | the same for the cold first round |
| `lsp_edit_outside_analysis_ms` | the edit-to-diagnostics time not spent in the analysis (the server's own work around it) |

The levels partition the analysis call. *file* is discovery and parse through the
shared cache; *unit* is grouping, symbol tables, `uses`/`consumes` resolution and
each unit's prelude; *definition* is resolving and checking declaration bodies
(and test bodies); *project* is the whole-project validations and assembling the
result. The marks sit at the pipeline's own phase boundaries, so this is an
attribution, not a profile. It is the evidence #1523's trigger asks for.

## The scheduler trigger

The incremental-scheduler decision (S2 of
[#3243](https://github.com/bynk-lang/bynk/issues/3243), the deferred
[#1523](https://github.com/bynk-lang/bynk/issues/1523)) was deferred by the
maintainer with this trigger: **edit-to-diagnostics p95 above 250 ms at 200
contexts, on either shape, in a release record.** `cargo xtask bench` warns when a
record reaches it, and the `keystroke_latency` row of
`design/greenfield-status.md` says whether it is reached. The 200-context size
must be in a record (the default `--sizes` includes it).
