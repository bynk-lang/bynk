# 0769 — Suite types and constants in stub values; local codecs for any generic shape

- **Status:** Accepted (v0.525.10)

**Context.** ADR 0759 recorded two limits: a suite `stub … returns` value could not name a suite `type` or `const`, and a suite type under a generic other than `List`/`Option` at a JSON boundary failed in `tsc`.

**Decision.** Both fit the accepted proposal (#3066) and need no new surface. (1) A stub clause carries its suite's types and constants like its fixtures and helpers; the checker adds them to the clause's view and the stub method declares them locally ahead of the value (and argument patterns). (2) A runner already declares codecs for the suite type and its `List`/`Option` wrappers; after the body lowers, it also declares the codecs for every other root the body reached that names a suite type, so no checker diagnostic is needed.

**Consequences.** No new diagnostic. A runner whose body reaches a suite type under `Result` or a nested container carries those codecs too; other output is unchanged.
