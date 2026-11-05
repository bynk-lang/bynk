---
level: minor
changelog: A suite's or case's `env` entry may name a `fixture` of the same suite instead of a `String` literal (`APP_PRIVATE_KEY = appKey`), so a long test-only value such as a private key lives in one file under `fixtures/`; the text is verbatim, trailing newline included, and a use in `env` counts for `bynk.fixture.unused` (#2956)
---

## ADR: suite-env-fixture-value
title: An `env` entry's value may be a fixture of the same suite
summary: Amends ADR 0524's literal-only values; the text is verbatim and stays in the test module once

**Context.** ADR 0524 made an `env { NAME = … }` value a `String` literal only,
matching what a Worker's `env` holds. ADR 0720 then added `fixture`, a suite's
compile-time `String` constant read from `fixtures/`, but an `env` entry could
not name one. A long shared value, such as the RSA key `bynk-lang/github-hooks`
uses for its GitHub App tests, had to be pasted inline into every suite whose
participants read it through `Secrets.get`, which is the duplication fixtures
were meant to remove (#2956).

**Decision.**
- **Values.** An `env` entry's value is a `String` literal or the bare name of
  a `fixture` the same suite declares, in the suite's `env` or a case's.
  Anything else, including another suite's fixture, stays
  `bynk.suite.env_not_literal`, whose message now names both forms.
- **The text is verbatim.** The value is the fixture's text under ADR 0720's
  content rules (BOM dropped, newlines normalised), so a file's trailing newline
  is part of it. There is no trimmed form: the reading code trims, or the file
  is saved without one.
- **Emission.** A case runner sets `__bynkEnv` with a fixture entry naming the
  file's module-level constant, so the text is in the test module once, however
  many cases use it. Comparisons that need the value (`bynk dev` grouping by
  distinct map, a named target's `vars` check) use the resolved text.
- **Usage.** A fixture named only in an `env` entry counts as used for
  `bynk.fixture.unused`.

**Consequences.** A test key lives in one file under `fixtures/` and reaches
every tier's participants. Under `bynk dev`, each value is passed to the
Workers as one `--var` argument, so on Linux a value is limited to 128 KiB.
