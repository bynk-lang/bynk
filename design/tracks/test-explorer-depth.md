# Test Explorer depth: language-integrated testing in the editor

- **Status:** Settling. Spine issue [#2257](https://github.com/bynk-lang/bynk/issues/2257)
  is open. Every question in §3 is open, each with the investigation done and a
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
  `bynk-emit/src/emitter/test_runtime/`, against the ADRs above. Nothing was run; claims
  about runtime behaviour are marked where they are unverified.

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

## 1. The theme

Testing is part of the language: three suite tiers (`unit`, `integration`, `system`),
seeded generative `property` tests, history properties over generated call histories,
`stub` doubles, a harness-owned clock, deterministic platform doubles, and `system` suites
that run against a named deployment. The Test Explorer uses almost none of that
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
run without changing the hierarchy. This needs no JSON change.

### 3.2 How do results stream, and what is the supported format? — OPEN

**Investigation.** Three separate facts block live results.

1. **The protocol is internal.** [ADR 0098](../decisions/0098-bynkc-test-json.md) D2 makes
   NDJSON a runner-to-driver protocol, "not a consumer contract", and says it leaves "a
   clean path to live streaming later".
2. **The runner does not stream.** In the generated `tests/main.ts`, case events are emitted
   in a loop over a collected `results` array *after* the suite's cases have run
   (`bynk-emit/src/project/tests_emit.rs`, the `for (const r of results)` block). The
   driver then captures stdout whole and parses it once the process exits
   (`parse_ndjson(&stdout)` in `test_runner.rs`).

3. **`suite-begin` is also emitted after the run.** It carries `tests: results.length`, and
   `const results = await m.run(only)` comes before it (`tests_emit.rs`, the same block).
   An emit-as-you-go runner would therefore not know a suite's case count when it begins
   the suite. The driver's `parse_ndjson` does not read `tests`, but a supported stream
   invites consumers who pre-size progress from it.

So publishing the stream alone would not make results arrive earlier. Streaming needs the
runner to emit each case as it finishes, the driver to forward lines as they arrive, and the
extension to consume them. Events also carry no duration; a `durationMs` on the `case` event
is additive.

**Recommendation.** Settle a supported `--format ndjson` whose documented events are
`suite-begin`, `case` and `run-end`, with additive optional fields (`durationMs`, and the
§3.3 fields). The pinned document stays the surface for `--format json`. Treat the emission
order change as part of the same slice. For `suite-begin.tests`, count the cases that pass
the runner's `want()` filter before running the suite, so the field keeps its meaning and
the stream ADR can promise it. The alternative is to make `tests` optional or move the
count to a `suite-end` event; the review is asked to choose. Open for review: whether to
cap per-case captured output, since an unbounded `stdout` per case is a memory risk in a
long run.

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
give that property the same draw stream as the full run. That is read from the source, not
yet confirmed by a run; Slice C confirms it with one run before offering "re-run this
property with the seed", and no wider verification is needed. It also means a seed
reproduces a failure only while the property keeps its ordinal (§3.4).

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
module changes what a pinned root seed draws for the later ones. A pin is therefore only
meaningful against the source it was taken from. (Whether the ordinal is counted per module
or per project is read from `prop_runners` being local to the module emitter and is not yet
confirmed.)

**Recommendation.** Pinning is **editor-local**: kept in workspace state, passed as
`--seed`, never written to `bynk.toml` or source. It is shown in the tree item's description
so it is never invisible, and cleared by one command. A pin records the file's content at
the time it was taken, and is dropped with a visible note when that module's source changes,
rather than silently drawing a different stream. Capturing a regression permanently is
a language question (a literal example in the `property`), not an editor one, and is out of
scope here.

### 3.5 Per-test coverage — OPEN

**Investigation.** [ADR 0514](../decisions/0514-test-coverage-covered-lines.md) keeps
coverage whole-run because attributing lines to tests needs one run per case. But the
driver already has `--case` and `--coverage`, so the *editor* can offer on-demand per-case
coverage with no driver change: run that one case with `--coverage` and show the result.
The cost is a `tsc → node` run per request ([ADR 0271](../decisions/0271-bynkc-test-coverage.md)
(C)), which is why it must be on demand rather than automatic.

**Recommendation.** Ship "Run with coverage" for a selected case or file (extension only).
Defer a persistent line-to-tests index and affected-only reruns until Slice E shows whether
the on-demand form is enough. Do not change the whole-run coverage contract.

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
  profile over `[test.targets]` plus `dev`; `transport` shown in the results. Extension
  only, and it must call `bynk test --against <name>` (§6).
- **Slice C — Reproducible property failures.** §3.3 fields, §3.4 pinning, "Re-run with this
  seed", and the counterexample as a structured message. Small runner and driver change.
  Needs one confirming run of the §3.3 seed derivation.
- **Slice D — Streaming and durations.** §3.2: emit-as-you-go in the runner, a supported
  `--format ndjson`, per-case output and `durationMs`, and the extension consuming the
  stream.
- **Slice E — Continuous runs and on-demand coverage.** §3.7 and §3.5: continuous run on
  save, per-case and per-file coverage. Affected-only is deferred.
- **Slice F — Explain failures.** §3.6, starting with the spike. Provisional.

Slices A and B can start before the doc settles in the sense that they depend on no open
question except §3.1 and §3.8(a). Slice B also relies on §6's rule that the extension only
ever invokes `--against <name>`; that is a settled constraint, not an open question.

## 5. Front-loaded ADR candidates

Numbers are taken at merge, not pre-allocated.

- **Extending the JSON document** with optional trailing `Case` fields (§3.3), because the
  document's field order is a pinned contract ([ADR 0098](../decisions/0098-bynkc-test-json.md)).
  Lands with Slice C.
- **A supported result stream** (§3.2), because it promotes an internal protocol to a public
  one. It must also settle what `suite-begin.tests` means once cases are emitted as they
  finish. Lands with Slice D.
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

- [ ] Slice A — close the controller's gaps
- [ ] Slice B — tiers and run targets
- [ ] Slice C — reproducible property failures
- [ ] Slice D — streaming results and durations
- [ ] Slice E — continuous runs and on-demand coverage
- [ ] Slice F — explain failures

## 8. Done when

- The Testing view filters by tier, and runs the `system` suites against a named target.
- A selection of cases or suites runs exactly that selection, and a skipped case is reported
  as skipped.
- A failed property or history property shows its seed and counterexample as data, and
  offers to reproduce and pin it.
- Results appear as each case finishes, with durations and output.
- Saving a `.bynk` file reruns tests when a continuous run is on, and a case or file can be
  run with coverage on demand.
- Failures are explained in the language's own terms to the extent the §3.6 spike shows the
  runner can support.
