---
level: minor
changelog: A `suite` can declare pure helper `fn`s that its `case` and `property` bodies call, private to the suite and emitted into test output only
probes: emit_diagnostics, diagnostic_coverage
---

## ADR: suite-local-fn-helpers
title: A suite declares pure, private, test-only helper functions
summary: Suite bodies admit `fn`; helpers are pure, per-suite, test-only and may not shadow a name in scope

**Context.** A suite body admitted only `uses`, `stub`, `env`, `case` and `property`, so a fixture or request builder shared across cases had to be copied into every case. The only workaround was to put the helper in the target or a `commons`, which ships test-only code in the deployable unit and cannot build test-only values (`Val[T]`). Surfaced writing a GitHub webhook receiver as a dogfooding example (#2834).

**Decision.** A suite (fragment or block form) may declare free `fn`s.

- **Pure only.** A helper returning `Effect` is `bynk.suite.fn_effectful`; a method form is `bynk.suite.fn_method`. Effectful helpers that address services or capabilities raise tier-promotion and `stub`/`expect … called` attribution questions and are a follow-up.
- **Private to the declaring suite**, which is one `SuiteDecl` (one file or one block), as `stub` scoping already treats it (#1860). A helper is callable from that suite's cases, properties and sibling helpers, at `unit`, `integration` and `system`. Another suite, even of the same target, may declare a helper of the same name.
- **Test-only values allowed.** A helper body is checked with a narrow allowance for test-only values (`Val[T]`), not as a `case` body: `expect`, `advance`, observation and `run.id` stay case-only.
- **No shadowing.** A helper named like any value in a case's scope (a function, type, constant, embed, asset, service or agent of the target or a used commons) is `bynk.suite.fn_name_clash`; a repeated helper name is `bynk.suite.duplicate_fn`.
- **Test-only emission.** Helpers join the suite's view in a per-suite copy; the unit's own symbol table is never touched, so the deployable is byte-identical with or without them. Each test runner declares its own suite's helpers as local functions after the target's names are in scope, so a helper sees the same unqualified names a case does and two suites sharing one test module cannot collide. A helper body carries no source map, like a `stub` value.
- **Tooling.** A helper is indexed as a `fn` definition in a per-file suite namespace, so hover, go-to-definition, references and semantic tokens work; completion offers the buffer's own helpers; signature help resolves them.

**Consequences.** Shared test fixtures no longer widen the deployable. Each runner repeats its suite's helpers in the generated test output, a deliberate trade for privacy and scope without a mangling scheme. Sharing helpers across suites (a test-only `commons`) and effectful helpers stay open.
