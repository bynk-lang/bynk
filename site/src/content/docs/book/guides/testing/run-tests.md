---
title: Run your tests
---
**Goal:** run a project's `suite` and `integration` blocks, read pass/fail, and —
in your editor — click straight from a failing expectation to the line that failed.

Once you've [written some tests](/book/guides/testing/write-tests/), run them with `bynkc test`:

```sh
bynkc test            # from the project root
```

`bynkc test` compiles the project (every generated `tests/*.test.ts` module
included), then runs the aggregated runner under Node — type-checking as it goes,
so a type error stops you with the usual diagnostics. It also checks every event
against the committed `bynk.schema.lock`, without writing it, so a breaking event
change fails the run with `bynk.event.non_additive_schema_change` (see
[the schema registry](/book/guides/events/understand-events/#checking-against-the-lock-without-writing-it)),
and a breaking change to an agent's stored state fails it with
`bynk.agent.non_additive_state_change` (see
[Change stored state safely](/book/guides/agents-and-state/stateful-agent/#change-stored-state-safely)).
It exits non-zero if any case fails. To run only some tests, repeat `--case "<name>"` for
each case or pass `--suite <name>` for a suite; a name that matches nothing fails the run
instead of passing with nothing run (see
[Selecting tests](/docs/bynk-cli/#selecting-tests)). You need `node` and `tsc` (or `tsx`) on your `PATH`; check with
[`bynk doctor --only test`](/docs/editor-and-tooling/doctor/).

```text
commerce.money:
  ✓ accepts positive
  ✗ deliberate failure
    expect total == 900
      expected: total == 900
      actual:   950 == 900
    at tests/commerce/money.bynk:8:12

1 passed, 1 failed.
```

A failed `expect` reports the predicate and — for a top-level comparison — its
**expected-vs-actual** operands, plus the **`path:line:col`** of the line that
failed, for both unit and integration tests.

## Generative failures: seeds, shrinking, and `--seed`

A [`property`](/book/reference/testing/) runs its `for all` body over many
generated inputs. When one fails, the report tells you how many cases ran, the
run's root **seed**, and a **shrunk** counterexample — the smallest inputs that
still fail — with a copy-paste line to reproduce it:

```text
commerce.money › more discount, never a higher price
  property failed after 41 cases (seed 0x5f3a)
  shrunk counterexample:  p = 100, a = 10, b = 20
  expect discount(p, b) <= discount(p, a)
  reproduce: bynkc test commerce/money.bynk --seed 0x5f3a
```

Each run draws a **fresh random seed**, printed only on failure. Thread it back
with `bynkc test --seed <hex>` (e.g. `--seed 0x5f3a`) to make the run reproduce
byte-for-byte — the same generated inputs, the same shrink, every time. This is
what the reproduce line pastes for you, so a flaky-looking property becomes a
deterministic one you can debug.

In VS Code the Testing view shows the seed and the counterexample as part of a
failed property, and offers **Re-run with this seed** on it. **Pin this seed**
keeps the seed for that one test, so running the test again repeats the failure;
a pin is held back, and shown as stale, once the suite's property tests change (one
added, removed or reordered), because the seed could then draw a different stream.

## Machine-readable results: `--format json`

For CI and tooling, `--format json` emits a single, stable JSON **document**
instead of the human lines:

```sh
bynkc test --format json
```

```jsonc
{
  "passed": 10,
  "failed": 1,
  "suites": [
    {
      "name": "commerce.money",
      "kind": "unit",
      "cases": [
        {"name": "accepts positive", "outcome": "pass"},
        {"name": "deliberate failure", "outcome": "fail",
         "message": "expect total == 900\n  expected: total == 900\n  actual:   950 == 900\n  at tests/commerce/money.bynk:8:12",
         "location": {"path": "tests/commerce/money.bynk", "line": 8, "col": 12}}
      ]
    }
  ]
}
```

Each suite carries a `kind` (`"unit"` or `"integration"`) and a clean `name`; a
failing case carries a `message` and a structured `location` for click-through.
The exit code is unchanged — `0` only if the project compiled and every case
passed.

There are three shapes a consumer should handle, distinguished by `error`:

- **a normal run** — `suites` present, no `error` (it may still have `failed >
  0`);
- **a compile failure** — no `suites`; `error.kind` is `"compile"` and
  `error.diagnostics` carries the `path:line:col: severity[category]: message`
  lines (the same shape the editor's problem-matcher reads);
- **a crashed run** — the `suites` observed before the crash, plus `error.kind`
  `"runtime"` with the captured `stderr`.

The exit code always follows the runner's own process status, so a crash is
never reported as success.

For a consumer that wants results while the run is going, `--format ndjson` writes the
same information as one JSON event per line, each case when it finishes and with how
long it took (see
[Streaming results](/docs/bynk-cli/#streaming-results)). The VS Code Test Explorer
uses it, so a long project fills in case by case, and the Testing view shows each
case's duration.

## Coverage: `--coverage`

`--coverage` runs the suite as normal, then reports **statement/line coverage
attributed to your `.bynk` source** — not the generated TypeScript:

```sh
bynkc test --coverage
```

```text
✓ 12 passed  (rate-limiter)

Coverage
  src/limiter.bynk    ▓▓▓▓▓▓··   86%   (42/49 lines)
  src/decide.bynk     ▓▓▓▓▓▓▓▓  100%   (11/11 lines)
  src/entry.bynk      ▓▓▓▓▓···   71%   (20/28 lines)  uncovered: 34-38, 51
  ─────────────────────────────────────────────────
  total                          84%   (73/88 lines)
```

Coverage is collected out-of-band by the runtime (V8's `NODE_V8_COVERAGE`) and
remapped through the same source maps the debugger trusts, so the generated
`.ts`/`.js` layer is invisible: a covered line is a **`.bynk`** line. Generated
glue with no source origin (codec wrappers, capability-injection) is
out-of-scope — never counted as uncovered user code — and the measured set
excludes your `tests/` tree and the workers scaffold an integration suite stands
up, so what you see is the source you are actually testing.

`--coverage` needs the `tsc → node` run: it is **incompatible with `--inspect`
and `--no-run`**, and requires `tsc` and `node` on your `PATH` (the `tsx`
fallback is not supported for coverage). Any of those combinations fails with an
actionable message rather than silently producing wrong numbers. This increment
reports **line/statement** coverage; per-branch coverage is a planned follow-up.

With `--format json`, the same numbers arrive as a `coverage` block instead of
the table — for a CI artifact:

```jsonc
{
  "passed": 12,
  "failed": 0,
  "suites": [ /* … */ ],
  "coverage": {
    "covered": 73,
    "lines": 88,
    "percent": 84,
    "files": [
      {"path": "src/limiter.bynk", "covered": 42, "lines": 49, "percent": 86,
       "uncovered": [], "covered_lines": [3, 4, 5 /* … */]},
      {"path": "src/entry.bynk", "covered": 20, "lines": 28, "percent": 71,
       "uncovered": [34, 35, 36, 37, 38, 51], "covered_lines": [2, 3 /* … */]}
    ]
  }
}
```

The block is present only for a `--coverage` run that attributed lines; a normal
run omits it, so an existing consumer's document is byte-for-byte unchanged.
Every `path` is a project-relative `.bynk` file, whether you name the project
as `.` or by its absolute path. `uncovered` lists the 1-based source lines that
never ran, and `covered_lines` lists the ones that did, so the two together are
every executable line. (`covered` is the count.)

## In the editor: the Test Explorer

The [VS Code extension](/docs/editor-and-tooling/editor-support/) consumes that
JSON surface directly. Open the **Testing** view (the beaker icon): the tree
populates by **discovery** — `bynkc test --no-run --format json` lists your
suites and cases without running them, so each test links to its `.bynk` line
before you run anything (use the Refresh control to re-discover after edits).
Run from the tree, or invoke **Bynk: Run Tests** from the command palette;
results then show inline, a failing expectation links to its `.bynk` line, and a
compile failure lands in the Problems panel exactly as
[`bynkc check`](/docs/cli/) does. The extension resolves `bynkc` the
same way the check task does — the `bynk.compilerPath` setting, else `bynkc` on
`PATH`.

**Run with Coverage** in the Testing view runs `bynk test --coverage --format
json`. VS Code's **Test Coverage** view then lists each `.bynk` file's line
percentage, and the editor gutter marks the lines that ran and the lines that
didn't. Coverage is for the whole run, not per test. Where coverage can't run
(only `tsx` on `PATH`), the run reports the driver's reason and offers **Run
Doctor**.

## Related

- [Write tests and stub collaborators](/book/guides/testing/write-tests/) — the `suite` block, `expect`, and `stub`.
- [Test tiers](/book/guides/testing/integration/) — the `as <tier>` dial and a `system` flow over the real wire.
- Reference: [CLI (`bynkc`)](/docs/cli/) — every `bynkc test` flag and exit code.
