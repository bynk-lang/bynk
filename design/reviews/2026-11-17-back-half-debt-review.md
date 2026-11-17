# The back half after seven weeks of features — a technical-debt review

**Reviewed at:** v0.639.1, 17 November 2026 (`e43b4487`)
**Baseline:** v0.291.0, 1 October 2026 (`76af4893`), the commit the
[1 October review](2026-10-01-language-implementation-review.md) was taken against
**Scope:** the back half of the toolchain: `bynk-model`, `bynk-emit` (with its TypeScript runtime),
`bynk-ts`, `bynk-driver`, and the `bynk` CLI's deploy, dev and migrate code. The front half
(`bynk-syntax`, `bynk-check`) appears only where the back half leans on it.
**Question:** is technical debt accumulating in the back half faster than it is being paid down?
And are the repository's own debt gauges telling us?
**Leads to:** [`../tracks/back-half-paydown.md`](../tracks/back-half-paydown.md).

---

## How this review was produced

The tree was built and tested first, on the pinned toolchain (`rustc 1.95.0`, Node 22):

- `cargo build --workspace --all-targets`: clean.
- `cargo test --workspace --no-fail-fast`: **5,312 passed, 1 failed, 1 ignored** across 144 test
  binaries.
  - The failure was `bynk/tests/dev_inspect_agents.rs`. npm reported `EJSONPARSE` on an empty
    `package.json` while `npx` was fetching wrangler.
  - Run on its own (`cargo test -p bynk --test dev_inspect_agents`), it passed 4/4.
  - It is a parallel-run flake around the shared npx cache, not a code fault. It is noted under
    §6.4 because it is a symptom of the unshared tool-spawning code.
- `cargo xtask greenfield-status`: *table current (or every move acknowledged)*.
- `cargo xtask fn-lengths`: within ceilings.

As in the earlier reviews, **no finding below is a broken build, a failing test or a tripped gate.**
This review asks a different question: what the gates *let through*, and whether their recorded
numbers are true.

Method:

1. **Growth and churn.** Every number is measured at both `76af4893` and `e43b4487`, using
   `git show <rev>:<path>`. There is no recollection in it.
2. **The repository's own probes,** run live and compared with the committed
   `design/greenfield-status.md` and its history. Every commit that changed a gated value since
   1 October was classified as "up" or "down".
3. **Three read-throughs,** each of one region: the core code generator (`emitter.rs`, `lower.rs`,
   `emit.rs`, `serialisation.rs`), the Workers target and `bynk-driver`, and the CLI's deploy code.
   Each headline figure taken from a read-through was re-measured by hand before it went in.
   Where the two counts differed slightly, because one method counted comment lines or test
   modules and the other did not, this document quotes the hand measurement.

The appendix has the commands.

---

## Headline

**Production code in the back half has roughly tripled in seven weeks. The structural debt is
growing with it. And the gauges this repository built to catch that are no longer holding it
back.**

Since 1 October, `main` has taken **about 1,220 non-stamp commits in 47 days**. Of those, 64 carry
a `feat` prefix, 23 `fix` and **7 `refactor`**; most subjects carry no prefix at all. Five of the 7
refactors were steps inside one feature (the Worker-group track, #3562).

Production lines (excluding `#[cfg(test)]` modules and test files):

| Crate | 1 Oct | 17 Nov | Change |
|---|---:|---:|---:|
| `bynk-emit` | 21,172 | 36,894 | +74% |
| `bynk-driver` | 3,104 | 20,172 | ×6.5 |
| `bynk-ts` | 5,288 | 6,690 | +27% |
| `bynk-model` | — | 2,097 | new |
| `bynk` (CLI) | 7,272 | 44,401 | ×6.1 |
| **Back half** | **36,836** | **110,254** | **×3.0** |

Part of `bynk-driver`'s growth is a move rather than new code. `bynk-emit/src/project.rs` and
`tests_emit` went to the driver in D1 (#2982). The CLI's growth is almost all new deploy code.

Three findings carry the review:

- **Finding 1: the gates now record growth rather than stop it** (§1).
  - The gated `ts_writes` probe went from 822 to **1,502**. It was raised in **165** commits and
    lowered in 15.
  - `emit_abi_shapes`, the probe guarding ADR 0310's code-generation freeze, went from **1 to 104**.
  - The trend rows in `design/greenfield-status.md` have not been refreshed since **16 October**.
    The table says 356 wildcard arms; the probe reads **747**.
  - Raw-text statements (`TsStmt::raw`) went from 50 to **112**, and no probe counts them.
- **Finding 2: most generated TypeScript is still text** (§2).
  - Expression lowering returns `Lowered { pre: Vec<String>, expr: String }`, as it did on
    1 October.
  - `lower.rs` builds **no** `TsExpr` nodes. Its `format!` count rose from 315 to 517.
  - Because the output is text, import headers are *predicted* by hand-kept predicates instead of
    read off a tree. Two build targets compose the same semantics in two places.
- **Finding 3: the deploy code has no model of a resource** (§5).
  - Each Cloudflare resource kind is threaded by hand through config, plan, ledger, provisioning
    and revert.
  - The computed `Plan` is not what apply executes.
  - Cloudflare is reached two ways, wrangler subprocesses and REST, with no shared error or retry
    layer.

None of this is yet a defect a user would see. It is debt in the strict sense: each new slice
costs more than the last, and the costs land in the places the gates do not look.

---

## 1. The gauges

### 1.1 Gated probes have become a change log

The greenfield probes were built (T0.0, #999; P7.0, P7.5) to make regressions visible. They are
"gated" in a specific sense: the gate fails when the live value **differs** from the committed
row. That forces a PR that moves a value to acknowledge the move. It does not force anyone to
refuse it.

Under feature velocity, acknowledging a move has become routine:

| Probe | 4 Oct | 17 Nov | Commits up / down since 1 Oct |
|---|---:|---:|---|
| `ts_writes` | 822 | 1,502 | 165 / 15 |
| `emit_abi_shapes` | 1 | 104 | rose in 27 of 28 distinct moves |
| `ts_any` | 26 | 38 | — |
| `verbatim_sites` | 11 | 15 | — |
| `span_keyed_maps` | 4 | 6 | — |

`emit_abi_shapes` matters most. ADR 0310 froze the shapes generated code may reach into the
vendored bindings for, and the probe exists to make a leak visible. A hundred and three leaks
later, it is a list, not a guard.

### 1.2 The trend rows are stale

`design/greenfield-status.md` is generated, but its trend rows are only rewritten when someone
runs `--apply`. Nothing requires that, because trend rows are not gated. The last change to the
`wildcard_arms` row was `2f9d1ba2` (#1974, 16 October), and 296 later versions of the file carry
the same 356.

Live values today:

| Probe | Committed | Live |
|---|---:|---:|
| `wildcard_arms` | 356 | **747** |
| `keep_in_sync` | 202 | 222 |
| `fixture_kinds` | error=690, run=125 | error=1,492, run=377 |

The `test_density` row still lists the deleted `bynk-ir`, `bynk-lower`, `bynk-render` and
`bynk-grammar` crates.

The probe itself has not changed since 1 October. It runs clippy with the lint forced on and
parses the JSON, so the doubling is real.

Wildcard arms by crate (clippy with `-W clippy::wildcard_enum_match_arm`):

| Crate | Arms |
|---|---:|
| bynk-check | 328 |
| bynk-emit | 192 |
| bynk-ide | 53 |
| bynk-driver | 52 |
| bynk-syntax | 37 |
| bynk-lsp | 30 |
| bynk | 20 |
| bynk-project | 10 |
| **bynk-ts** | **8** |
| bynk-fmt | 6 |
| **bynk-model** | **5** |
| others | 6 |

The workspace lint is still `warn` and **no crate opts in** (`[lints] workspace = true` appears in
no manifest). `bynk-ts` and `bynk-model` could opt in after an afternoon's work.

### 1.3 What the probes do not see

- **`TsStmt::raw`.** It is a second whole-statement text escape, separate from `Verbatim`.
  - It has **112** production sites: bynk-emit 52, bynk-driver `tests_emit` 60. On 1 October it
    had 50.
  - `verbatim_sites` looks only for `TsStmt::verbatim(` and `TsExpr::VerbatimExpr(`, so it
    reports 15.
  - The Worker-group emitter added on 16 November (`workers_group.rs`, #3649) builds its whole
    program with `format!`/`push_str` and inserts it as one `TsStmt::raw` (line 339).
- **Strings carried in typed-looking nodes.** `TsExpr::Ident` wraps whole expressions, for
  example `emit.rs:1302` (`Ident(format!("({cache} ??= ...).v"))`). `TsType::named` takes
  arbitrary type text.
- **`bynk-driver/src/project/tests_emit/`.** This is 12.6k lines that generate the test-harness
  TypeScript. `ts_writes` and `verbatim_*` scan `bynk-emit/src` only, so none of it is counted.

### 1.4 The ratchet that works

`cargo xtask fn-lengths` (#3167) works the other way round. A ceiling may go down freely, and
raising one is the review conversation. Its history since it landed is mostly *down*:
`emit_worker_entry` went 1,313 → 1,109 and `build_output` 767 → 360. Seventy functions are still
over the 300-line threshold, the longest being `emit_system_http_support` (1,157) and
`emit_worker_entry` (1,121).

**This is the model the other probes should follow.**

---

## 2. Code generation is still text

### 2.1 Lowering

```rust
// bynk-emit/src/emitter/lower.rs:1418
pub(crate) struct Lowered {
    pub pre: Vec<String>,
    pub expr: String,
}
```

This struct is byte-identical at `76af4893`, so this is long-standing debt that keeps growing,
not new debt:

- `lower.rs` has grown from 6,391 to 10,888 lines.
- It contains no `TsExpr::` construction.
- Its production `format!` count went from 315 to 517.

Phase 7 of the trajectory gave the program *skeleton* a typed tree. The *expressions* inside it
never left strings.

What it costs:

- **Precedence is decided by looking at text.** `ts_array_elem` (`emitter.rs`, near `ts_ty`)
  decides parenthesisation by checking for `=>`, `|` and `&` in a string. A comment at
  `lower.rs:836-840` says nesting "has to travel inside the string".
- **Text goes round in circles.** `ts_ty` renders a `TsType` to a `String` for its 44 callers in
  `lower.rs`. `emit.rs:4581-4592` builds nodes from strings, prints them, and wraps the result as
  a node again.
- **Global state stands in for missing context.** Two `thread_local!`s were added after 1 October,
  `FAULT_SOURCE` and `type_renames::SPELLINGS` (`emitter.rs`). The doc comment on the second
  justifies it by `ts_ty` having "~45 callers and no module context".

### 2.2 Imports are predicted, not derived

Because the body is text, `bynk-emit` cannot ask the tree which names it uses.
`bynk-ts/src/name_closure.rs` can answer that question for a typed program. Instead,
17 functions named `*needs*` / `*uses*` / `*mentions*` guess the answer ahead of time, and each must
agree exactly with what the body later emits. Two examples:

- `file_mentions_*`, `emitter.rs:984-1013`.
- `agent_needs_rehydrate`, `emit.rs:5128`. Its own doc says it "mirrors the per-field validation…
  a mismatch is an unused/undefined import".

The workspace's `keep_in_sync` count (comments saying *in sync*, *mirrors*, *parity* or *must
match*) is 222. Each one is a rule taught in review, not by the compiler.

### 2.3 Two compose paths

There are two build targets (`BuildTarget::Bundle` / `Workers`, `bynk-check/src/project_model.rs`).
Each composes the application in a different crate:

- Bundle: `emit_composition_root`, in `bynk-driver/src/project.rs:5345` (447 lines).
- Workers: `emit_worker_compose`, in `bynk-emit/src/emitter/workers.rs:300` (846 lines).

The same rules are derived twice from different sources:

- **The event-subscriber envelope rule:** `project.rs:5551` and `workers.rs:1348`.
- **Per-subscriber failure isolation:** `project.rs:5531-5560` "mirrors" the Cloudflare fan-out
  Durable Object in `events_fanout.rs`.
- **`append_missing_bindings` exists three times:** `workers.rs:248`, `workers_entry.rs:872`,
  `tests_emit/system_support.rs:119`.
- **Cross-context calls:** `lower_workers_cross_context_call` (`emit.rs:4488`) is the
  Workers-mode twin of `lower_cross_context_call` (`lower.rs:2670`).
- **Small node builders** (`str_lit`, `ident`, `member`, `call`) are redefined in about four
  files.

A semantic fix to composition must be made, and tested, twice.

### 2.4 Re-derived semantics and silent fallbacks

- **Fallback policy is inconsistent.** When a checker-recorded type is missing, some paths emit
  `any` (5 production `"any"` literals in `lower.rs`, for example `lower.rs:5909`). Others panic.
  `lower.rs` has 22 production `panic!` / `unreachable!` / `expect` sites, most of them "the
  checker should already have typed this" guards.
- **Name matching stands in for resolution.** Resolved names are compared against string literals in many places,
  for example `lower.rs:2689-2696`
  (`cap == "Idempotency" && consumed == "bynk" && method.name == "claim"`) and the store-kind
  match in `emit.rs:5135-5216`. This is resolution the checker could record once.
- **Context is passed as flags and big structs.** `EmitProjectCtx` (`emit_model.rs`) has
  33 public fields. `LowerCtx` has 27. bynk-emit's production code carries 13
  `#[allow(clippy::too_many_arguments)]`, and `emit_free_fn_in` (`emit.rs:1059`) takes three
  `bool` flags.

---

## 3. `bynk-driver` is mostly a code generator

About 60% of `bynk-driver`'s production code generates TypeScript:

- `project/tests_emit/*`: about 12.4k lines.
- `project/test_runtime/*.ts`: 948 lines, embedded.
- `emit_composition_root`, the `WorkersEmit` per-Worker orchestration
  (`project.rs:3770-4595`) and `emit_worker_handlers_barrel`.

Fourteen driver files import `bynk_ts`. The crate's name says it orchestrates. Its contents say it
generates code too.

Consequences:

- **The layering runs backwards in tests.** `bynk-emit`'s dev-dependencies include `bynk-driver`.
- **Two runtimes are unchecked.** `test_runtime/*.ts` and `bynk-emit/src/emitter/http_client_runtime.ts`
  (545 lines, `include_str!`) are outside the runtime's `tsconfig.json` and `bundle.mjs --check`.
  The runtime's version-skew guard covers everything else. The test-runtime files are script
  fragments that use free names (`ExpectationError`), so they cannot be type-checked on their own.
- **Test support is split across two places.** About 900 of `storage.ts`'s 1,230 lines are test
  fakes (`makeIntegration*`, `makeTestState`) shipped in the production runtime, while the rest of
  the test runtime lives in the driver.
- **The driver edits generated TypeScript as text after the fact.** It has a hand-written TS lexer,
  `names_ident` (`system_support.rs:19`), which `integration_module.rs:678` uses to decide imports.
  `merge_cookie` (`system_support.rs`) runs `strip_prefix("\"cookie\": ")` over emitted text.
- **Coordinating functions are long.** `run_checks` (`project.rs:2752`) has 23 parameters and
  951 lines, and `CompileOptions` has grown from 7 fields to 21.

---

## 4. The emitter and the CLI both own wrangler configuration

- **The emitter writes it.** `bynk-emit/src/emitter/wrangler.rs` grew from 386 lines to 3,027,
  the fastest-growing file in scope.
- **The CLI parses it again into its own types.** `bynk/src/deploy/config.rs:15-60` declares its
  own serde `WranglerConfig` with 13 binding structs.
- **The CLI then partly regenerates it.** `synthesise_environment_block` (`config.rs:1609`,
  271 lines) copies tables into `[env.*]` from a hand-written list (`config.rs:1797`).
  - #3577 and #3586 (a binding table missing from `[env.*]`) were exactly this gap. A test now
    guards that case (`config.rs:2619`).
- **Six sidecar JSON manifests** (contracts, secrets, vectorize, containers, hyperdrive, agents)
  are written by one side and read back by the other.
- **The driver re-derives bindings too.** `worker_bindings.rs` re-derives the same binding inputs
  so that its duplicate-binding check "sees exactly the bindings the build would write".

So a binding kind is known in at least three places, with no shared type between them.

---

## 5. Deploy

### 5.1 No resource-kind model

`Resources` (`bynk/src/deploy/config.rs:559`) has about 20 per-kind fields. Each kind is then
handled separately at every stage:

- **The ledger** keeps a separate map per kind (`ledger.rs:44-146`) and per-kind `record_*` /
  `forget_*` methods (`record_kv`, `record_r2`, `record_vectorize`, `record_d1`,
  `record_hyperdrive`, …).
- **`find_orphans`** (`ledger.rs:1553`, 204 lines) repeats one block per kind. Five of its
  comments read "likewise, with its Worker or on its own".
- **Provisioning** repeats the pattern per kind: `create_kv`, `provision_r2_buckets`,
  `provision_d1_databases`, `create_d1`, `create_r2_bucket`.

Mentions of three kinds, in production code:

| Kind | config.rs | plan.rs | ledger.rs | provisioning.rs | revert.rs |
|---|---:|---:|---:|---:|---:|
| KV | 43 | 61 | 52 | 56 | 1 |
| D1 | 26 | 35 | 31 | 37 | 22 |
| Hyperdrive | 46 | 43 | 33 | 3 | 0 |

Adding a resource kind means a coordinated edit across five files plus the emitter.

### 5.2 The plan is computed, then decided again

- **The plan is pure,** which is good: `derive_plan` (`plan.rs:2777`).
- **But apply does not receive it.** `deploy_one` (`provisioning.rs:73`) takes `Resources` and
  the ledger, has 17 parameters and `#[allow(clippy::too_many_arguments)]`, and makes its own
  decisions. An example is the `trust_recorded` logic at `provisioning.rs:88-109`.
- **The ledger is written part-way through,** from six sites (`provisioning.rs:134, 202, 262,
  988, 1029`; `plan.rs:1454`).
- **Orchestration is one function:** `plan.rs::run`, 744 lines.

What `bynk deploy --plan` shows and what `bynk deploy` does are therefore separate code that is
kept in agreement by hand.

### 5.3 Two channels to Cloudflare

| | wrangler subprocess | REST (`ureq`) |
|---|---|---|
| Kinds | KV, D1, R2, queues, Vectorize | Hyperdrive, secrets store, AI Search, mTLS, email |
| Call sites | 18 `wrangler_command` in deploy (22 across the CLI) | spread across deploy and migrate |
| Credential | `wrangler login` | API token |
| Outcome detection | matching error text (153 `contains("…")` in deploy) | HTTP status and JSON |
| Retry / backoff | none | none |
| Test seam | fake `#!/bin/sh` scripts on disk; 55 `cfg(unix)` gates in `provisioning.rs` | traits with fakes (`HyperdriveApi`, `SecretsStores`, …) |

Some further details:

- The REST client lives in `bynk/src/migrate/live.rs:88`, so deploy depends on migrate, and
  several call sites construct it separately.
- Errors are mostly `String`: deploy's production code has 99 `Result<_, String>` signatures.
- `wrangler@4` is pinned in two places (`bynk/src/dev.rs`, `bynk/src/workers.rs`).

### 5.4 What is in good shape

- The REST lookups are behind traits with fakes, and the deploy tests use them well. Deploy
  carries roughly as much test code as production code.
- `derive_plan` is a pure function, which is the right foundation for §5.2's remedy.

---

## 6. Module size and tests

### 6.1 Module size

Source files of a given size or more:

| Size | 1 Oct | 17 Nov |
|---|---:|---:|
| ≥ 2,000 lines | 28 | 50 |
| ≥ 3,000 lines | 17 | 33 |
| ≥ 5,000 lines | 7 | 18 |

In the back half, the largest are:

| File | Lines |
|---|---:|
| `emit.rs` | 11,069 |
| `lower.rs` | 10,888 |
| `emitter.rs` | 8,441 |
| `bynk-driver/src/project.rs` | 7,044 |
| `bynk-ts/src/printer.rs` | 6,838 |
| `bynk/src/deploy/plan.rs` | 6,177 |

The file layout does not match the responsibilities:

- `LowerCtx` and `ModuleCtx` are defined in `emitter.rs` but used by `lower.rs`. The
  `impl LowerCtx` block alone runs about 1,340 lines.
- `lower.rs` has 13 `emit_*` functions.

There is a function-length ratchet but no module-size one.

### 6.2 Unit tests per crate

| Crate | `#[test]` in `src/` | In own `tests/` | Unit tests per 1k production lines |
|---|---:|---:|---:|
| bynk-ts | 192 | 0 | 28.7 |
| bynk-model | 38 | 0 | 18.1 |
| bynk-project | 149 | 24 | 17.1 |
| bynk (CLI) | 682 | 332 | 15.4 |
| bynk-driver | 306 | 13 | 15.2 |
| bynk-syntax | 251 | 0 | 7.5 |
| bynk-check | 612 | 25 | 7.3 |
| **bynk-emit** | **261** | **15** | **7.1** |

The weight sits in `bynk-tests`:

- 5,627 `.bynk` fixture programs (153 MB under `bynk-tests/tests`).
- 756 test functions, most of which sweep many fixtures.
- `fixture_kinds` live: error=1,492, run=377, warnings=24, diagnostics=17.

The [9 October unit-test review](2026-10-09-unit-test-coverage-review.md) measured
`bynk-emit`'s own-test coverage at 38.6% against 95.6% for the full suite. The 7.1-per-1k density
today suggests that has not improved. Coverage was not re-measured for this review because
`cargo-llvm-cov` is not installed on this host.

The structural reason ties back to §2. **A lowering that returns a `String` can only be tested by
comparing printed text,** which is a golden test by another name. Seams that return typed values
are what make unit tests worth writing.

### 6.3 Comments as an issue log

Comment lines as a share of all lines (approximate, by line prefix):

| Crate | Comment share |
|---|---:|
| bynk-emit | 24% |
| bynk-ts | 28% |
| bynk-model | 30% |
| bynk-driver | 18% |
| bynk (CLI) | 14% |

Deploy production code carries 665 `#NNNN` issue references. Typical forms are "Slice 5, DECISION
B" and "P5 (#2323)". There are 875 ADRs and about 67k lines of design documents.

The record is unusually thorough. But comments in the code increasingly explain *how this came
to be* rather than *what this does now*, and every change must keep both true.

### 6.4 Smaller items

- **Dead-code shims:** `#[allow(dead_code)] fn _unused_hashmap` (`emitter.rs`) and
  `_ensure_components_used` (`bynk-driver/src/project.rs`).
- **Pinned tool versions are scattered.** `npm_install.rs` docs and tests still say
  `typescript@5`, while `TYPESCRIPT_MAJOR_TESTED = 7` (`bynk-emit/src/lib.rs`).
- **The Rust/TypeScript environment-variable protocol is unwritten.** 24 `BYNK_*` variables
  connect the Rust and TypeScript sides as scattered literals, for example `BYNK_TEST_SETTLE_MS`
  in `test_runner.rs` and `settle.ts`.
- **The parallel-run `npx` flake** above. There is no single place where tools are spawned that
  could serialise or pre-warm wrangler.

---

## 7. Recommendations

The sequence, freeze policy and exit criteria are in
[`../tracks/back-half-paydown.md`](../tracks/back-half-paydown.md). In brief:

1. **Make the gauges hold** (§1).
   - Turn the growth-type gated probes into ratchets that can only go down.
   - Count `TsStmt::raw`, and widen the TS-writing probes to `bynk-driver`.
   - Refresh the trend rows at stamp time.
   - Opt `bynk-ts` and `bynk-model` into the workspace lints.
2. **Make lowering typed** (§2.1).
   - `Lowered { pre: Vec<TsStmt>, expr: TsExpr }`.
   - Then derive imports from the tree and delete the prediction predicates (§2.2).
3. **One compose model for both targets** (§2.3), and **move TypeScript generation out of the
   driver** (§3).
4. **One typed binding model** shared by the emitter, the driver's checks and the CLI (§4).
5. **Deploy** (§5):
   - a resource-kind abstraction;
   - apply executes the `Plan`;
   - one Cloudflare client with typed errors and retries;
   - a wrangler fake.
6. **Split modules along the seams 2–5 create,** with a module-size ratchet (§6.1).
7. **Rebalance tests,** unit tests at each new seam, then thin the corpus where a fixture only
   re-proves a unit-tested rule (§6.2).

---

## Appendix — reproducing the numbers

All commands run from the repository root. `OLD=76af4893`.

**Production lines per crate.** This counts everything above a file's first
`#[cfg(test)] mod …`, and skips files whose path names a test:

```sh
python3 - "$OLD" bynk-emit bynk-driver bynk-ts bynk-model bynk <<'EOF'
import sys, subprocess, re
rev, crates = sys.argv[1], sys.argv[2:]
def ls(c):
    return subprocess.run(['git', 'ls-tree', '-r', '--name-only', rev, c],
                          capture_output=True, text=True).stdout.split()
def show(f):
    return subprocess.run(['git', 'show', f'{rev}:{f}'],
                          capture_output=True, text=True).stdout.splitlines()
for c in crates:
    prod = 0
    for f in ls(c):
        if not f.endswith('.rs') or '/src/' not in f: continue
        if re.search(r'(^|/)tests?(\.rs|/)|_tests?\.rs$|tests_', f): continue
        t = show(f)
        i = next((i for i, l in enumerate(t) if l.strip() == '#[cfg(test)]'
                  and i + 1 < len(t) and re.match(r'\s*mod \w+', t[i + 1])), len(t))
        prod += i
    print(c, prod)
EOF
```

Run it again with `HEAD` in place of `$OLD`. `bynk-driver/src/project/tests_emit/` is production
code despite its name. The figures in the headline table add it back in. Measure it with
`git ls-files bynk-driver/src/project/tests_emit | xargs wc -l`.

**Commit mix:**

```sh
git log --format=%s $OLD..HEAD | grep -v 'chore(stamp)' | wc -l
git log --format=%s $OLD..HEAD | grep -v 'chore(stamp)' | grep -oE '^[a-z]+' | sort | uniq -c | sort -rn
```

**`ts_writes` moves:** walk `git log --reverse --since=2026-10-01 -- design/greenfield-status.md`,
read the `ts_writes` row at each commit, and count the rises and falls. `emit_abi_shapes` is
measured the same way.

**Live versus committed probes:**

```sh
cargo xtask greenfield-status
git log -1 -S'| `wildcard_arms` | no (trend) | 356' -- design/greenfield-status.md
```

**Wildcard arms per crate:**

```sh
cargo clippy --workspace --message-format=json -- -W clippy::wildcard_enum_match_arm
```

Count `compiler-message` records whose `code.code` is `clippy::wildcard_enum_match_arm`, grouped by
the top-level directory of the primary span.

**Raw-text statements:**

```sh
git grep -h 'TsStmt::raw' -- bynk-emit/src bynk-driver/src | wc -l
```

For the 1 October figure, run `git show $OLD:<f> | grep -c 'TsStmt::raw'` over
`git ls-tree -r --name-only $OLD bynk-emit/src`.

**Lowering is text:**

```sh
grep -c 'TsExpr::' bynk-emit/src/emitter/lower.rs
git show $OLD:bynk-emit/src/emitter/lower.rs | grep -A3 'struct Lowered'
```

The first should print 0. For `format!` counts, strip the comment lines and the test module first.

**Duplicates:**

```sh
grep -rn 'fn append_missing_bindings' bynk-emit bynk-driver
grep -n wants_envelope bynk-driver/src/project.rs bynk-emit/src/emitter/workers.rs
```

**Deploy:** `grep -c wrangler_command`, `grep -c 'Result<.*String>'` and `grep -c 'contains("'`
over `bynk/src/deploy`, with test modules excluded as above.

**Module sizes:**

```sh
git ls-files '*.rs' | grep '/src/' | xargs wc -l | sort -rn
```

Do the same at `$OLD` with `git ls-tree` and `git show`.

**Unit-test density:** count `#[test]` under `<crate>/src` and divide by production lines. The
`tests/` columns count `#[test]` under `<crate>/tests`.
