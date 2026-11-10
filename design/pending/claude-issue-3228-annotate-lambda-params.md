---
level: patch
changelog: The emitter annotates every lambda parameter with its checked type, so a lambda handed to a kernel method (`append`, `map`, `filter`, `flatMap`, `any`) no longer fails `tsc --strict` with an implicit `any` ([#3228](https://github.com/bynk-lang/bynk/issues/3228))
---

## ADR: annotate-every-lambda-parameter
title: The emitter annotates every lambda parameter with its checked type
summary: A lambda parameter is annotated unconditionally, not only where it has no source annotation and a pattern

**Context.** `lower_lambda` printed a parameter's type only when the source annotated it, or (#2712) when it was a record pattern. Any other parameter was a bare name, which TypeScript types from context. Several kernel lowerings give none: `acc.append((x) => …)` lowers to a spread element, and `map`, `filter`, `flatMap`, `any` and `count` lower to an immediately invoked arrow, `((x) => …)(__x)`. Neither position is a contextual type, so `tsc --strict` rejected the emitted code with TS7006 (implicit `any`) for a program the Bynk checker had accepted (#3228, found in #3193).

**Decision.** Annotate every lambda parameter with its checked type, from the lambda's own `Fn` type, which the checker already records and the record-pattern case already used. This chooses annotating always over guessing, per call site, where TypeScript can infer: whether a position gives context depends on how each kernel is lowered, so a rule that tracked it would have to change with every kernel lowering and would fail silently when it drifted. A source annotation still wins where there is one. The change is local to `lower_lambda`; no kernel lowering is touched.

**Consequences.** Emitted TypeScript is slightly more verbose: each unannotated lambda parameter now carries `: T`. Emitted output is already allowed to change within 1.x, and a golden churns wherever a lambda is emitted (a mechanical, annotation-only diff). A lambda parameter whose checked type cannot be written in TypeScript would now surface as a `tsc` error where it was previously an implicit `any`; none exists in the fixture corpus, and the emit-check gate would catch one. No grammar, checker or runtime change, and no change to hover, completion, semantic tokens or signature help.
