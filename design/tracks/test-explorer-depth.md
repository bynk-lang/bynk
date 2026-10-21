# Test Explorer depth: language-integrated testing in the editor

- **Status:** Settling. Spine issue [#2257](https://github.com/bynk-lang/bynk/issues/2257)
  is open. Slices A ([#2260](https://github.com/bynk-lang/bynk/issues/2260)), B
  ([#2261](https://github.com/bynk-lang/bynk/issues/2261)), C
  ([#2266](https://github.com/bynk-lang/bynk/issues/2266)), D
  ([#2267](https://github.com/bynk-lang/bynk/issues/2267)) and E
  ([#2269](https://github.com/bynk-lang/bynk/issues/2269)) are cut as proposals, not yet
  accepted. Every question in §3 is open, each with the investigation done and a
  recommendation for review to accept, change or reject. Nothing here is a build
  authorisation.
- **Realises:** [`../bynk-tooling-roadmap.md`](../bynk-tooling-roadmap.md) §5.2 (the test
  runner) and the editor-currency work on test lenses. It builds on
  [ADR 0098](../decisions/0098-bynkc-test-json.md) (`bynkc test --format json`),
  [ADR 0238](../decisions/0238-vscode-check-test-debug-via-driver.md) (the extension shells
  the driver), [ADR 0271](../decisions/0271-bynkc-test-coverage.md) and
  [ADR 0514](../decisions/0514-test-coverage-covered-lines.md) (coverage), and
  [ADR 0527](../decisions/0527-end-to-end-testing-is-the-languages.md),
  [0528](../decisions/0528-test-against-a-named-target.md) and
  [0532](../decisions/0532-test-run-identity.md) (end-to-end testing).
- **Posture:** Feature track per [ADR 0076](../decisions/0076-feature-track-posture.md), run
  GitHub-native per [ADR 0167](../decisions/0167-feature-tracks-run-github-native.md). It
  qualifies on two axes (§2). It adds no language surface; its subject is tooling.
- **Evidence:** a read of `vscode-bynk/src/testing.ts`, `bynk-driver/src/test_json.rs`,
  `bynk-driver/src/test_runner.rs` and the runner emitters under
  `bynk-emit/src/emitter/test_runtime/`, against the ADRs above. The things run are the seed
  experiment in §3.3 and the timing, coverage and source-map measurements in §3.5, on a
  compiler built from `main`; other claims about runtime behaviour are marked where they are unverified.

## What's already true

**The extension** (`vscode-bynk/src/testing.ts`) is a `TestController` with three profiles
(Run, Debug, Coverage). It discovers with `bynk test --no-run --format json`, debounced
500 ms after any `.bynk` edit, and builds a two-level tree: suite, then case. A case
carries its declaration `location`, so it links to source before any run. A run is one
process (`bynk test . --format json`) whose whole document is reconciled onto the tree
afterwards. A failed case gets a `TestMessage` with the assertion's `path:line:col`.
`bynk.runTestCase` and `bynk.debugTestCase` back the per-case lenses.

**The driver** exposes `--no-run`, `--format json`, `--case <name>` (exact name, across
suites), `--seed <hex>`, `--coverage`, `--inspect`, and `--against <target>` with
`--skip-unsupported` and `--settle`. The JSON document carries `passed`, `failed`,
`skipped`, `suites[].cases[]` (`name`, `outcome`, `message`, `location`, `retries`),
`error`, `coverage` and `transport`.

**The runner** (`bynk-emit/src/emitter/test_runtime/`) draws property cases from a root
seed and, on failure, shrinks and reports a message of the form
`property failed after N cases (seed 0x…)`, then `shrunk counterexample: …`, then the first
assertion line, then `reproduce: bynkc test <file> --seed 0x…`. History properties report
`shrunk sequence:` in the same shape ([ADR 0155](../decisions/0155-history-properties-are-runner-only.md)).
All of it travels to the editor as one `message` string.

**The test tier is not in the JSON.** A case's tier is per case (an `as` clause on the `case`
or `suite` header, [ADR 0153](../decisions/0153-tier-is-a-dial-on-the-case-header.md)), but the
document's `Suite.kind` is only `unit` or `integration`. The emitted runner derives it from
the module name (`m.name.startsWith(PREFIX)`), a file-naming convention for
`tests/integration_*` files, so an `as system` case is indistinguishable in the document, and
the extension's own type admits only those two values.

## 1. The theme

Testing is part of the language: three test tiers (`unit`, `integration`, `system`, set per
case or as a suite default by an `as` clause, [ADR 0153](../decisions/0153-tier-is-a-dial-on-the-case-header.md)),
seeded generative `property` tests, history properties over generated call histories,
`stub` doubles, a harness-owned clock, deterministic platform doubles, and `system`-tier
cases that run against a named deployment. The Test Explorer uses almost none of that
vocabulary. It treats Bynk as a generic runner that prints pass or fail.

The end state when this track retires: the Testing view filters by tier and runs the
`system` suites against a named target; a failed property offers to reproduce and pin its
seed and shows its counterexample; results stream in with durations and output; tests rerun
on save; and a failure is explained in the language's own terms. Along the way it fixes the
current controller's gaps (§3.8).

## 2. Why a track (the ADR 0076 trigger)

- [x] **Multi-increment.** Six slices (§4), some extension-only and some touching the runner
  and the JSON document. They share one result model, so its additions are best settled
  together.
- [x] **Surface not yet settled.** The JSON additions, the result stream, seed pinning and
  per-test coverage all have genuine alternatives (§3).
- [ ] **Security/safety boundary.** Not met; see §6. `--against` already refuses unsafe
  targets in the driver, and this track must not route around that.

## 3. Design questions

Each is **OPEN**. The recommendation is what the settling review is asked to accept or
overturn.

### 3.1 What is the tree's hierarchy? — OPEN

**Investigation.** A `TestController` has exactly one hierarchy, so "group by file" and
"group by suite" cannot both be offered as a toggle. Today a suite item has no `uri`, and
the JSON's `Suite` has no `location`, but every case has a `location` whose `path` names
its file, so a suite's file is derivable with no driver change. What is *not* known is
whether a suite maps to exactly one file. [ADR 0160](../decisions/0160-multi-file-commons-test-barrel.md)
(the multi-file test barrel) suggests a suite can span files. The settling pass must check
this before any decision rests on it.

**Recommendation.** Keep suite as the parent. Give a suite item a `uri` when all its cases
share one file, and none otherwise. That rule is correct whichever way the suite-to-file
question falls, so Slice A does not wait on it. Provide "Run tests in this file" as a command that
builds a request from the cases whose `location.path` matches, which delivers the file-level
run without changing the hierarchy. A case's `location.path` is project-root-relative and
slash-normalised, so the command resolves it against the project root (as the controller
already does with `Uri.joinPath(root, path)`) before comparing it with a document URI. This
needs no JSON change.

### 3.2 How do results stream, and what is the supported format? — OPEN

**Investigation.** Four facts shape live results, and the first two correct an earlier
reading of this section.

1. **The protocol is internal.** [ADR 0098](../decisions/0098-bynkc-test-json.md) D2 makes
   NDJSON a runner-to-driver protocol, "not a consumer contract", and says it leaves "a
   clean path to live streaming later".
2. **The runner already streams, one suite at a time.** The generated `tests/main.ts`
   loops over the modules and, as each module's `m.run(only)` returns, emits that module's
   `suite-begin`, `case` and `suite-end` events (`bynk-emit/src/project/tests_emit.rs`, the
   NDJSON branch). What blocks live results is the driver: it collects the child's whole
   output (`cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).output()`,
   `test_runner.rs:805`, `:894`, the two `node` run paths; the `.output()` at `:544` is the
   preceding `tsc` compile step and is not on the result path) and renders the pinned
   document only once the process has exited. So suite-level streaming needs a driver change and a published format, and
   **no emitter change**. `bynk test` shells `bynkc test` with the child writing to the
   shared stdout, so streaming through `bynk` costs nothing extra.
3. **Within a module, nothing streams.** `m.run(only)` is built in one place
   (`run_dispatch_stmt` and `build_run_function` in `tests_emit.rs`): it pushes each case's
   result onto an array and returns it once, so the events for a suite's cases are emitted
   only after the whole suite has run. `suite-begin` is emitted after that too, carrying
   `tests: results.length`. Case-level streaming, and any per-case duration, therefore need
   the module `run` to report each case as it completes. That is an emitter change, and
   because the shape is embedded in every golden test module and runner, it regenerates all
   of them (186 tracked fixture files when this was written, 174 of them
   `expected/tests/main.ts`; the count grows as fixtures are added).
4. **There is no per-case output to capture.** The test `Logger` and `Events` doubles do
   nothing, `Fetch` faults, and the only `console` use in the test runtime is a cron-failure
   suppression (`bynk-emit/src/emitter/test_runtime/jobs.ts`). A case has nothing beyond its
   failure `message`, which the `case` event already carries.

**Recommendation.** Settle a supported `--format ndjson` whose documented events are the
ones the runner emits today (`run-begin`, `suite-begin`, `case`, `suite-end`, `run-end`)
plus a **new** driver-emitted `error` event (none exists today) for a compile failure or a
runtime crash, with additive
optional fields (the §3.3 fields and, later, `durationMs`). The pinned document stays the
surface for `--format json`. Deliver it in two stages: stage 1 is suite-level streaming (the
driver, the format, and the extension consuming it; no emitter change); stage 2 is
case-level streaming and `durationMs` (the emitter change and the regenerated goldens).
For `suite-begin.tests` in stage 2, count the cases that pass the runner's `want()` filter
before running the suite, so the field keeps its meaning and the stream ADR can promise it;
the alternative is to make `tests` optional or move the count to `suite-end`, and the review
is asked to choose. Per-case output is dropped from this track: if a case-output facility
is wanted, it is a separate language and runtime decision.

### 3.3 What structured data does a failed property carry? — OPEN

**Investigation.** Today everything is one `message` string, and the seed, counterexample
and reproduce line are text inside it. An extension could parse `seed 0x…` out of the
message as a stopgap, but that couples the editor to message wording. The seed is the
run's **root** seed, from which each property derives its own via `__bynkMix`; it is not a
per-property seed. Each property's seed is `__bynkMix(__bynkSeed, prop_ordinal)`
(`bynk-emit/src/project/tests_emit.rs`, the property and history runner emitters), where
`prop_ordinal` is the property's position among the module's generative properties
(`prop_runners.len()`), a constant fixed when the runner is emitted. It is neither a hash of
the name nor a runtime counter. So `--case` filtering, which only changes which cases
`want()` lets through, does not shift the ordinal, and `--case <name> --seed <root>` should
give that property the same draw stream as the full run. **Confirmed by a run** (a
three-property suite over a refined `Int`, the last property `expect p != 73`, seed
`0x5f3a`): the full run and the `--case` run both reported "failed after 74 cases" with the
same shrunk counterexample, so "re-run this property with the seed" is sound. The same
property and seed with one property fewer before it passed, so the stream does depend on the
ordinal, and a seed reproduces a failure only while the property keeps its ordinal (§3.4).

**Recommendation.** Additive, optional `Case` fields, appended last to preserve the pinned
field order: `kind` (`"property"`, `"history"`), `seed` (the root seed, hex), and
`counterexample` (the runner's rendered text, not structured values, since values are
rendered by the runner in Bynk syntax). For a history property, a `steps` list is a
follow-on (§3.6). Absent for non-generative cases, so every other document is unchanged.

### 3.4 Where does a pinned seed live? — OPEN

**Investigation.** Pinning makes a property deterministic, which changes what it checks: a
pinned run stops exploring. A seed committed to the repository would silently freeze
coverage of the input space for everyone. Because a property's stream depends on its
ordinal (§3.3), adding, removing or reordering an earlier generative property in the same
module changes what a pinned root seed draws for the later ones (confirmed by the run in
§3.3). A pin is therefore only meaningful against the source it was taken from. (Whether the
ordinal is counted per module or per project is read from `prop_runners` being local to the
module emitter, and the run in §3.3 used one module, so it is still not confirmed.)

**Recommendation.** Pinning is **editor-local**: kept in workspace state, passed as
`--seed`, never written to `bynk.toml` or source. It is shown in the tree item's description
so it is never invisible, and cleared by one command. Capturing a regression permanently is
a language question (a literal example in the `property`), not an editor one, and is out of
scope here.

Of the two ways to handle the ordinal dependence, this track picks **stale-pin detection**
and does not change the derivation. A pin is stored with the case's identity and a
fingerprint: the ordered list of case names in its suite, which discovery already returns.
When the fingerprint no longer matches, the pin is shown as stale in the tree item's
description and is not passed to `--seed` until the user re-pins or clears it, so a changed
suite never silently draws a different stream. Once Slice C adds `kind` to the JSON, the
fingerprint can narrow to the suite's generative cases, which avoids false staleness when
only an example-based case is edited.

The alternative is a name-stable derivation, mixing a hash of the property name into the
per-property seed. It is rejected here because it is a runner change that alters the stream
for every existing seed: every `reproduce: bynkc test … --seed 0x…` line already in a log or
a CI run would stop reproducing. It stays available if review prefers it, in which case it
is listed under Slice C and needs its own ADR.

### 3.5 Per-test coverage — OPEN

**Investigation.** [ADR 0514](../decisions/0514-test-coverage-covered-lines.md) keeps
coverage whole-run because attributing lines to tests needs one run per case. The driver
already accepts `--case` together with `--coverage` (ADR 0271 refuses `--coverage` only with
`--inspect` and `--no-run`), so the *editor* can offer on-demand coverage for one case with
no driver change. The cost is a `tsc → node` run per request ([ADR 0271](../decisions/0271-bynkc-test-coverage.md)
(C)), and coverage is unavailable where only `tsx` exists.

Two measurements, on `examples/orders` (two cases) with a debug build of the compiler on
one machine, qualify what that run delivers:

- **A rerun is seconds on a small project.** A plain run took 5.3 s cold and 1.7 s warm; a
  whole-run coverage run took 1.5 to 2.1 s, so coverage was not measurably slower. It grows
  with project size, since each run is a full compile plus `node`.
- **Per-case coverage does not compose into the whole-run report, and the cause is a
  bug.** Repeated twice, with identical results each time: the whole run covered **14 of
  29** lines; case 1 alone covered **15 of 29**, including line 94 (the first line of the
  `orphans` body), which the whole run does not cover; case 2 alone covered **7 of 29**. The
  executable lines are the same in all three. Raw V8 coverage shows the line executed in
  both runs, so the loss is in the remapping, and it is filed as
  [#2270](https://github.com/bynk-lang/bynk/issues/2270): the emitter's source map anchors a
  handler's header and prologue to the *previous* handler's body line (the never-called
  `linesFor` handler's first two generated lines map onto line 94), and
  `bynk-driver/src/coverage.rs` keeps the tightest V8 range per `.bynk` line whatever its
  count, so an uncalled neighbour's count-0 range overrides an executed line whenever it
  happens to be the narrowest. How narrow each range is depends on how V8 split its block
  ranges for that run, so the verdict depends on which tests ran. It is independent of the
  editor.

**Recommendation.** Ship "Run with Coverage" for a **single case** (extension only), and
name the run after the case so the Coverage view says what it measures. Anything else (a
suite, several cases, a file) runs the whole project with coverage, as today, and says so.
Do not offer per-file or multi-case coverage until the driver can run a set of cases in one
process (the repeatable `--case`/`--suite` of §3.8(b)). Do not merge coverage across
processes: as long as [#2270](https://github.com/bynk-lang/bynk/issues/2270) stands, the
reports differ for reasons unrelated to what the tests cover. That is a workaround, not a
permanent limit; once the bug is fixed, whether per-case reports then agree with the
whole-run report is to be measured again before the rule is relaxed. The bug does not block
this track. Defer a persistent line-to-tests index and affected-only reruns until Slice E
shows whether the on-demand form is enough. Do not change the whole-run coverage contract.

### 3.6 Which failures can be explained in the language's terms? — OPEN

**Investigation.** [ADR 0155](../decisions/0155-history-properties-are-runner-only.md)
shrinks a history to a short sequence and prints it as text. `invariant` and `transition`
are enforced at the commit boundary, so their failures originate in a declaration, not an
`expect`. The failure `location` the runner reports is currently the assertion site. What
the runner can report about *which* declaration fired is not established here.

**Recommendation.** Do not design this speculatively. Slice F starts with a spike that
records what the runner can attach (declaration location for an invariant or transition
failure; a step list with handler locations for a history), and only then proposes fields.
Until it reports, §4 keeps Slice F's scope as "provisional".

### 3.7 Continuous and affected-only runs — OPEN

**Investigation.** `createRunProfile` takes a `supportsContinuousRun` flag. A continuous run
triggered on save can reuse the existing coalesced `runDiscovery` pattern. Affected-only
reruns need a dependency relation the checker holds; whether it is reachable from the
extension (through the LSP) is unknown. They also need a way to select several cases or
suites, which `--case` cannot express (§3.8).

**Recommendation.** First version: continuous whole-project rerun on save of a `.bynk`
file, cancelling any run in flight. Affected-only is a separate step, gated on §3.8 and an
LSP request.

### 3.8 How is a multi-item selection run? — OPEN

**Investigation.** The run handler sets `--case` only when exactly one case is requested
(`request.include?.length === 1 && request.include[0].parent`). A suite, a multi-case
selection and the exclude list are ignored, so everything runs and every case is reported.
`--case` takes one exact name and matches across suites, so a name shared by two suites runs
both, and `findCase` picks the first match for the lens. The debug profile ignores the
request altogether. Separately, the controller treats any outcome other than `pass` as a
failure, so a `skipped` case from a remote run is shown as failed.

**Recommendation.** Two options, for review. (a) Extension only: run each selected case as
its own process, sequentially, and report only the requested items. Simple, but one process
per case repeats the compile. (b) Make `--case` repeatable, and add `--suite <name>`, so one
process runs the selection. I recommend (b) for the long term and (a) as the interim in
Slice A, so Slice A is not blocked on a driver change.

## 4. Slice decomposition

Provisional until the doc settles. Each slice is an increment proposal, opened as a
sub-issue of the spine.

- **Slice A — Close the controller's gaps.** The `skipped` outcome and the top-level
  `skipped`/`transport`/`retries` fields; honour `include`/`exclude` using §3.8(a); debug the
  selection; suite `uri` and "Run tests in this file" per §3.1. Extension only.
- **Slice B — Tiers and run targets.** `TestTag`s for the three tiers; a "Run against…"
  profile over `[test.targets]` plus `dev`; `transport` shown in the results. Not
  extension-only: the JSON carries no per-case tier (see "What's already true"), so the slice
  adds one optional trailing `tier` field on `Case`, ideally attached by the driver from the
  compile's case table so the emitted runner and its golden fixtures are untouched. It
  depends on Slice A for the `skipped` state, and it must call `bynk test --against <name>`
  (§6).
- **Slice C — Reproducible property failures.** §3.3 fields, §3.4 pinning, "Re-run with this
  seed", and the counterexample as a structured message, with stale-pin detection (§3.4).
  Small runner and driver change; the §3.3 seed derivation is confirmed.
- **Slice D — Streaming and durations.** §3.2, in two stages. Stage 1: a supported
  `--format ndjson` and the extension consuming it, so results appear as each suite
  finishes (driver and extension only, no emitter change). Stage 2: case-level streaming and
  `durationMs` (an emitter change that regenerates every golden carrying the module `run`),
  which may split into its own
  proposal. No per-case output.
- **Slice E — Continuous runs and single-case coverage.** §3.7 and §3.5: a Watch toggle that
  reruns on save or an external change, and on-demand coverage for one case. Extension only.
  Multi-case and per-file coverage and affected-only reruns are deferred, because they need
  the repeatable `--case`/`--suite` (§3.8(b)); coverage is never merged across processes
  while [#2270](https://github.com/bynk-lang/bynk/issues/2270) stands.
- **Slice F — Explain failures.** §3.6, starting with the spike. Provisional.

Slices A and B depend on no open question except §3.1 and §3.8(a). Slice B also relies on
§6's rule that the extension only ever invokes `--against <name>`, a settled constraint, and
on Slice A's `skipped` handling.

## 5. Front-loaded ADR candidates

Numbers are taken at merge, not pre-allocated.

- **Extending the JSON document** with optional trailing `Case` fields (§3.3), because the
  document's field order is a pinned contract ([ADR 0098](../decisions/0098-bynkc-test-json.md)).
  Lands with Slice B, whose `tier` is the first such field; Slice C (§3.3) extends it.
- **A supported result stream** (§3.2), because it promotes an internal protocol to a public
  one, and amends ADR 0098 D2. It pins the event set, its field order and a stability promise
  (consumers ignore unknown events and fields), and, for stage 2, settles what
  `suite-begin.tests` means once cases are emitted as they finish. Lands with Slice D.
- **Repeatable `--case` and `--suite`** (§3.8(b)), if chosen.

Seed pinning (§3.4) is editor-local and needs no ADR unless review moves it into the
manifest.

## 6. Threat model

None for the track as a whole: it adds no new trust boundary and no language surface.

The one sensitive surface is Slice B's "Run against…" profile, which can point a test run at
a real deployment. [ADR 0528](../decisions/0528-test-against-a-named-target.md) already
makes the driver refuse a bare URL and any target marked `production = true`, and runs only
named targets. The extension's constraint: it must invoke `bynk test --against <name>` with
a name read from `[test.targets]`, and never construct, rewrite or default an endpoint, so
the driver stays the single place that decides what is safe. A deployed target keeps state
between runs ([ADR 0532](../decisions/0532-test-run-identity.md)); the profile's description
should say so. If review decides the extension must do more than this, revisit this section.

## 7. Slice status

- [ ] Slice A — close the controller's gaps ([#2260](https://github.com/bynk-lang/bynk/issues/2260))
- [ ] Slice B — tiers and run targets ([#2261](https://github.com/bynk-lang/bynk/issues/2261)); depends on A
- [ ] Slice C — reproducible property failures ([#2266](https://github.com/bynk-lang/bynk/issues/2266))
- [ ] Slice D — streaming results and durations ([#2267](https://github.com/bynk-lang/bynk/issues/2267))
- [ ] Slice E — continuous runs and single-case coverage ([#2269](https://github.com/bynk-lang/bynk/issues/2269)); builds on A's run plan
- [ ] Slice F — explain failures

## 8. Done when

- The Testing view filters by tier, and runs the `system` suites against a named target.
- A selection of cases or suites runs exactly that selection, and a skipped case is reported
  as skipped.
- A failed property or history property shows its seed and counterexample as data, and
  offers to reproduce and pin it.
- Results appear as each suite finishes, and as each case finishes with its duration once
  stage 2 lands.
- Saving a `.bynk` file reruns tests when a continuous run is on, and a single case can be
  run with coverage on demand.
- Failures are explained in the language's own terms to the extent the §3.6 spike shows the
  runner can support.
