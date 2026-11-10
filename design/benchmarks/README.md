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
