---
level: patch
changelog: Bynk supports TypeScript 7 only (#3420): the TypeScript 5 floor is dropped, `bynk doctor` warns on a `tsc` below 7, and CI verifies under 7 on every leg
---

## ADR: typescript-7-only
title: Bynk verifies against TypeScript 7 only
summary: The TypeScript 5 floor is dropped; every leg, the fallbacks and `bynk doctor` use 7

**Context.** Emitted output was verified under two TypeScript majors (#1672): 5, the floor, on every CI leg, and 7, the current one, in a dedicated job. Both cost something. The tsc 5 legs ran the corpus-wide type-checks and every `bynk test` under the slower JavaScript compiler (about 4x tsc 7 on the behavioural fixtures: 182 s against 45 s on 4 cores, found while fixing the slow tests in #3387 / #3413), and the TypeScript 7 job repeated parts of the same work. Meanwhile the `npx` fallbacks, the devcontainer feature and the runtime package already used 7.

**Decision.** Bynk dictates TypeScript 7 throughout. The generated code is meant to be readable, but which TypeScript version or tooling a user reads or runs it with is not something Bynk supports across majors. `bynk_emit::TYPESCRIPT_MAJOR_FLOOR` is removed; `TYPESCRIPT_MAJOR_TESTED` (7) is the one supported major. Every CI and release leg installs `typescript@7`, and the separate "Type-check under TypeScript 7" job is retired. `bynk doctor` warns on a `tsc` below 7 (`below supported (≥ 7)`) and still reports one newer than 7 as untested but `ok`.

**Consequences.** A user on `tsc` 5 or 6 gets a `bynk doctor` warning with the `npm install -g typescript@7` fix; nothing in the emitter is changed, so such a toolchain may keep working but is no longer verified. CI runs the corpus type-check and the behavioural fixtures once, under 7, which shortens the Linux system tier and the release job. The per-leg install is `typescript@7`, so a new TypeScript major still needs a deliberate bump of `TYPESCRIPT_MAJOR_TESTED` and the workflows together.
