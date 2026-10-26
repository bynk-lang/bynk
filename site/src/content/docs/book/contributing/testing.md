---
title: Testing & fixtures
---
The compiler's correctness rests on a large fixture suite plus a TypeScript
type-check gate. Both live under `bynkc/tests/`.

## The fixture suite

`tests/e2e.rs` discovers every directory under `tests/fixtures/positive/` and
`tests/fixtures/negative/` (currently 143 positive, 105 negative) and runs each
as one fixture. There are two shapes:

**Single-file**
- `input.bynk` — the source (a self-contained `commons`).
- `expected.ts` — the exact emitted TypeScript (positive), **or**
- `expected_error.txt` — expected diagnostics (negative).

**Project**
- `src/` — a source tree (one or more `context`/`commons` units).
- `expected/` — the emitted output tree to match (positive), **or**
  `expected_error.txt` (negative).
- `target.txt` — optional; `workers` selects the Workers target (default bundle).
- `bynk.toml` — optional; marks a project and configures `[paths]`.

`runtime.ts`, `tsconfig.json` and a `package.json` that is only the
`"type": "module"` marker are excluded from per-fixture comparison (they are
checked separately). A `package.json` carrying adapter dependencies is compared.

### How matching works

- **Positive** fixtures compare emitted files byte-for-byte against `expected/`
  (or `expected.ts`), trailing newlines included. A file that differs only in
  trailing whitespace fails like any other difference; the failure says so.
- **Negative** fixtures match by **substring**: each non-blank, non-`#` line of
  `expected_error.txt` must appear somewhere in the concatenated
  `"{code} {message}"` of the diagnostics. So a line is usually just a code, e.g.
  `bynk.refine.literal_violates`.

## How CI and the release run the suite

Every gate that runs the workspace suite runs it the same way:
`cargo nextest run --workspace --locked --profile ci`. That covers the PR gate
(`ci.yml`), the release gate (`release.yml`) and the bootstrap's verify job. The
`ci` profile in `.config/nextest.toml` runs each test in its own process, and
retries a failure once. A test that passes on the retry is listed as **FLAKY**
but doesn't fail the gate, at release as on a PR. A test still running after 2
minutes is named in the log, and one still running after 10 is stopped and
counts as a failure (then retried like any other), so a hang can't hold a gate
until its job times out. The policy is decided in that
one file, so a test can't pass CI and then fail the release because of the
harness. `xtask/tests/suite_harness.rs` checks that all three workflows run that
exact command, so they can't drift apart again unnoticed.

To reproduce a CI run locally, install
[nextest](https://nexte.st) and use the same command. A plain `cargo test` also
works, but it runs each binary's tests as threads in one process, with no
retry.

## The bless workflow

When you change the emitter (and the new output is correct), regenerate the
positive fixtures' expectations rather than editing them by hand:

```sh
BYNK_BLESS=1 cargo test -p bynkc bless_positive_fixtures
```

The `bless_positive_fixtures` test is a no-op unless `BYNK_BLESS` is set; with it
set, it recompiles each positive fixture and overwrites `expected/`. **Always
review the resulting diff** — blessing is how a regression silently becomes the
new "expected" if you are not careful. A project fixture's `expected/` is
deleted and rewritten, so a bless also removes the goldens of files the emitter
no longer writes. Because the comparison is byte-exact, a bless leaves the tree
clean unless emission actually changed, in content or in which files are
emitted.

`BYNK_BLESS` is the project's shared regenerate switch: the same run also
refreshes the generated reference pages (see [Working on the docs](/book/contributing/documentation/)).
Scope it to a specific test when you only mean to bless one thing.

## The `tsc` verification gate

`tests/tsc_verify.rs` (`emitted_typescript_passes_tsc_strict`) compiles every
positive fixture and runs `tsc --strict --noEmit` over the output. A
single-file fixture (`input.bynk`) is staged beside the runtime it imports, so
it is checked like a project. It is a backstop for emitter bugs that produce
TypeScript which round-trips our own comparison but does not actually
type-check. A golden matching the emitter byte for byte proves nothing about
whether either one type-checks.

It needs `tsc` on `PATH`, or falls back to `npx -p typescript@7 tsc`. CI runs it,
and the examples' `tsc --strict` check, under both TypeScript majors the output
is verified against: **5**, the floor, on every test leg, and **7**, the current
one, in a second pass on the Linux leg. The behaviour suites, which type-check
their fixtures before running them, run under 5 only in CI. A local run without
a global `tsc` uses 7 for them, through the fallback.
The two are `TYPESCRIPT_MAJOR_FLOOR` and `TYPESCRIPT_MAJOR_TESTED` in
`bynk-emit`, which `bynk doctor` and every `npx` fallback also read. Behaviour
when neither is available:

- locally — it logs a warning and passes (so a missing toolchain does not block
  you);
- in CI — set **`BYNK_REQUIRE_TSC=1`** to make a missing `tsc` a hard failure.

The same file checks that every emitted `.ts` is erasable by pure type-stripping
(ADR 0136), which `bynkc test --inspect` and in-browser evaluation depend on.
`embedded_runtime_strips_types_under_node` needs Node ≥ 22.6;
`all_emitted_typescript_strips_under_node` needs Node ≥ 22.13 (for
`stripTypeScriptTypes`). **`BYNK_REQUIRE_TSC=1` governs them too:** with it set,
a missing or too-old Node is a hard failure rather than a skip. CI's test legs
run Node 24, so both checks always run there.

A skip banner alone is not a gate: a test that prints `SKIPPED` and passes is
invisible in CI, because nextest never shows a passing test's output
(`success-output` defaults to `never`). That is why a required check fails
rather than skips.

## The behavioural gate

The golden comparison and the `tsc` gate prove what the compiler **emits**.
Neither runs it. A golden blesses whatever was emitted, so an emitted program
that type-checks but does the wrong thing passes both.

`tests/behaviour_fixtures.rs` closes that gap. A project-form positive fixture
whose `suite`s should **run** carries an `expected_run.txt`:

```text
# comments and blank lines are ignored
passed=3 failed=1
# #1649: enum state faults on reload
fail an enum Cell reads back after a reload
```

The gate copies each marked fixture to a scratch directory and runs
`bynkc test --format json` over it. It then checks:

- the case counts match `passed=`/`failed=`;
- every `fail <case name>` case **fails**;
- every other case **passes**;
- at least one case ran.

The check is strict in both directions. A listed case that starts passing fails
the gate, so the change that fixes a known defect must also delete its `fail`
line. Cite the tracking issue in a comment above each `fail`.

A suite-bearing fixture without the marker is type-checked by the `tsc` gate
but never run. Mark a fixture unless it exists only to pin emitted *shape*
rather than behaviour (for example `1402_stub_fails_and_single_outcome_sequence`,
whose cases can never pass by design).

The run uses the bundle target, because `bynkc test` has no `--target` flag.
Like the `tsc` gate, it skips locally without a TypeScript toolchain, and
`BYNK_REQUIRE_TSC=1` makes that a failure. The `fixture_kinds` row of
`design/greenfield-status.md` counts marked fixtures as `run=`.

## Adding a feature: the definition of done

A grammar increment is not complete until:

1. positive **and** negative fixtures cover it (and pass);
2. emitted output type-checks under the `tsc` gate;
3. any new diagnostic code is added to the registry in `diagnostics.rs`;
4. a change to what a program **does** at runtime is proved by a behavioural
   fixture (an `expected_run.txt` suite), not only by a golden;
5. the **docs** are updated in the same change — see
   [Working on the docs](/book/contributing/documentation/).
