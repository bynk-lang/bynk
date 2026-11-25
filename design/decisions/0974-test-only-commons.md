# 0974 — A test-only commons is how suites share code

- **Status:** Accepted (v0.710)

**Context.** Suite helpers, types and constants were private to their suite file, so a project with many suites over one API repeated the same request builders, claims, fixtures and row types in each (#3984).

**Decision.** The maintainer accepted the issue's decisions. A: a commons may be test-only. B: the marker is a contextual suffix on the commons header, `commons x.y for tests`; inferring it from a file's location under `tests/` was rejected because moving a file would change what it means, and a new keyword (`testkit x`) because it adds a declaration kind for one flag. C: a suite and another test-only commons may `uses` it, and it may `uses` production commons; a context, production commons or adapter that `uses` one is `bynk.uses.test_only_commons`, reported before the build leaves the commons out so it is not also reported as unknown. D: it holds a commons' items, checked as a suite helper is, so `Val[T]`, `RequestHeaders.of` and `TokenClaims.of` work in it; a commons has no services, agents, providers or `stub`s to refuse. E: a deployable build omits a test-only commons right after parsing, as it does a suite; `bynk test` and `bynk check` keep it, and the test module graph emits it as it does any commons.

A suite reaches the commons through its target's view: the `uses` map the test pass reads adds each test-only commons a suite `uses` to that suite's target, and only the test pass reads it, so no deployable module imports a test-only commons.

**Consequences.** Departures, both limits of this change. First, an effectful helper with `given` (#3983) is not available in a test-only commons: a commons has no one capability set, so `given` there is still `bynk.given.fn_outside_context`; effectful setup stays in the suite. Second, a test-only commons is added to the target's view for every suite of that target, so a suite sees the names of a test-only commons another suite of the same target `uses`, even without its own `uses`. The tree-sitter grammar, the keyword table, hover and the keyword semantic token cover the `for tests` marker.
