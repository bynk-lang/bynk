# 0757 — bynk test --require-tests makes "no tests" an opt-in failure

- **Status:** Accepted (v0.520)

**Context.** `bynk test` on a project with no tests printed `no test declarations found` and exited 0, so CI passed a project that never wrote a test (#3051, proposed in #3087).

**Decision.**
- **A. Opt-in flag, default unchanged.** `--require-tests` exits non-zero when no test is declared. A default change would break libraries, freshly scaffolded projects and monorepo CI that runs `bynk test` across mixed projects, and can be taken later as its own decision.
- **B. Wording.** Default: `warning: no tests found in <dir>` on stderr, exit 0. With the flag: `error: …`, exit 1. `--format json` gives an empty `suites` array by default and `error.kind == "no_tests"` under the flag (an additive kind).
- **C. `--no-run`.** Discovery is unaffected, except that `--require-tests` fails the same way on an empty discovery, so a discovery-only CI step can enforce it. At a workspace root the flag applies to each member.

**Consequences.** No language, checker, emitter or runtime change, and hover, completion, semantic tokens and signature help are unchanged, since nothing in the language surface moves. The VS Code Test Explorer already handles an empty list. A script that greps the old exact text must match `no tests found` instead.
