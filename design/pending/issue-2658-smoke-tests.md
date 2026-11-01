---
level: patch
changelog: "`bynk test --smoke` smoke-tests every `from http` route without a test written for it: each route is called with arguments `Val` fabricates and must not answer a 5xx or a body that doesn't decode as its declared type, a route with a JSON body must reject a malformed one, and a route behind an actor must refuse a call with no credential. It runs in process, against `dev`, or against a named target (`GET` routes only unless `--smoke-unsafe`); a route `Val` can't fill is skipped and named, and `--print` prints the synthesised suites. Also fixed: bare `Val[T]` of a record now honours an inline `where` on a field, so `qty: Int where Positive` is `1`, not `0`, and a field `where Matches(…)` needs a pin (`bynk.val.needs_pin`) as a `Matches` type does ([#2658](https://github.com/bynk-lang/bynk/issues/2658))"
---

## ADR: derived-smoke-suites
title: `bynk test --smoke` derives a `system` suite from a project's `from http` routes
summary: Smoke cases are synthesised Bynk source, checked and run through the ordinary test pipeline in place of the project's own suites; un-fabricable routes are skipped by an allowlist of checker codes; decoding is strict; the declared-status warning is deferred

**Context.** Every Bynk app that wanted an end-to-end check hand-wrote one, such
as `bynk-lang/example-template`'s `e2e/scenarios.sh`: a `curl` script asserting,
route by route, facts the compiler already holds (the routes, their params and
body types, their actors). ADR 0527 settled that end-to-end testing is the
language's, and the `--against` transport (ADRs 0526, 0528, 0530, 0531) runs
`system` cases on a real runtime. The proposal, #2658, asked for a smoke test
derived from the declarations, needing no author input.

**Decision.**

- **A. Synthesised suites, built in `bynkc test`.** For each context with a
  `from http` service the driver writes a `suite <context> as system` as Bynk
  source and adds it to the build's in-memory source overlay, beside the
  context file (`<stem>.smoke.bynk`), as doc examples do (#2461, the precedent).
  Everything the transport does (signing, `--settle`, `run.id`, the target
  guards, every output format) comes for free, and the checker checks the cases.
  The proposal placed the synthesis in the `bynk` driver behind a new
  `bynkc test --sources-stdin`. That was to reach the wire contract's response
  set, which only Decision E needed; with E deferred, the AST is enough, so the
  synthesis lives in `bynk-driver` and `bynk test --smoke` forwards the flag
  like any other.
- **The cases.** Per route, in existing test surface only: *answers* (every
  argument `Val[T]`, a `headers` parameter `RequestHeaders.of([])`, under the
  route's actor) passes unless the answer is a 5xx, even one the handler
  constructs, since its input is valid by construction; *rejects a malformed
  body* (a route with a JSON `body`) sends `Wire("{not json")` and passes on
  `Rejected(_)`, or `Handled(BadRequest(_))` on a service with `on rejected`;
  *refuses no credential* (an actor that isn't public) sends the call
  `by Nobody` and passes on `Rejected(Unauthorized)`. A route admitting
  `Visitor`, or an actor with `auth = None`, is called as it; otherwise as its
  first actor with `Val` of its identity type, a refinement actor taking its
  base's. Case names start `smoke: ` so they cannot collide with the author's.
- **Only the smoke runs.** A smoke run leaves the project's own suites and doc
  examples out of the build, so one that a real runtime refuses cannot refuse
  the smoke.
- **B. A flag on `bynk test`** (`--smoke`, with `--print` and `--smoke-unsafe`),
  sharing `--against`, `--settle`, `--format` and the exit contract.
- **C. Writes stay local.** In process and against `dev` every route is called;
  against a named target only `GET` routes are, unless `--smoke-unsafe`. Each
  route left out is reported as skipped with the reason.
- **D. Skip, never guess.** Whether a case can be built is decided by checking
  it: a case whose check reports an allowlisted code (`bynk.val.needs_pin`,
  `bynk.val.agent_not_generable`, the principal-construction codes,
  `bynk.test.remote_unsupported_*`) is dropped and reported as skipped with the
  checker's message, and the suite is checked again. Any other diagnostic in a
  synthesised suite is reported as a bug in `--smoke`, never as a skip, so a run
  cannot pass green by skipping everything; a run with no case to build fails.
  Skips are reported as remote skips are: a line on stderr, and an
  `outcome: "skipped"` case in `json` and `ndjson`.
- **Strict decoding.** Under `--smoke` the runner sets `BYNK_TEST_STRICT_WIRE`,
  and the test runtime's `responseToHttpResult` then fails a case on a status no
  `HttpResult` carries, or a JSON body that does not decode as the route's type,
  where an author's case gets the raw value. A typed handler cannot answer a
  body its own codec rejects, so this fires only on skew between a deployment and
  the run's build; it is tested directly against fabricated responses. A Worker
  never reads the variable.
- **E. Deferred: the undeclared-status warning.** The proposal recommended
  warning on a non-5xx status outside the route's body-walked response set.
  Making it a hard `expect` would fail on exactly the false positives E exists
  to avoid, and a warning has no channel from the runner to the report without
  changing the result documents' schema, which is a change of its own. This
  increment asserts no response-set membership.
- **F. The action is a follow-on.** The `smoke` input on `bynk-lang/bynk-dev`
  lands in that repository once a release ships `--smoke`.
- **A `Val` fix the smoke needed.** Bare `Val[T]` of a record ignored an inline
  `where` on a field (`qty: Int where Positive` fabricated `0`), so a smoke
  body failed its own boundary. A field's inline refinement now narrows the
  default exactly as a declared refined type's does, and an inline `Matches`
  on a field makes the record need a pin (`bynk.val.needs_pin`) as a declared
  `Matches` type does, rather than fabricating a value that fails it.

**Consequences.** `bynk test --smoke --against dev` replaces most of a
hand-written route-by-route script, and stays current as routes are added. A
failure's location names the synthesised file, which exists only in memory;
`--print` shows it. A smoke cannot know what an answer should be, or what a
sequence of requests does: the project's own suites stay the gate. The
response-set warning (E), `@example` pins for `Matches` types, a post-deploy
smoke in `bynk-lang/bynk-deploy` and the `bynk-dev` input are named follow-ons.
