---
level: minor
changelog: `bynk test` builds its JavaScript into `<output>/out-js`, inside the output directory, and the emitted `tsconfig.json` follows (`outDir` is `./out-js`, and `out-js` is excluded from the project), so concurrent runs whose `--output`s share a parent no longer overwrite one another's compiled tree
breaking: The compiled JavaScript moves from `<output>/../out-js` to `<output>/out-js`, and the emitted `tsconfig.json` now has `"outDir": "./out-js"` and excludes `out-js`; anything that read `../out-js` (a script running the `tsc`-built JavaScript, a Wrangler or CI step pointing at it) must read `<output>/out-js`, and a `tsconfig.json` copied from an earlier output needs the same two changes. `out-js` is now compiler-owned: a later `bynk build` or `bynk test` into the same `--output` removes the `.js`, `.map` and `.json` files it did not write, so copy `out-js` elsewhere (or run it) before the next build.
---
fix: build `bynk test`'s JavaScript inside the output directory (#4159)

Closes #4159.

## ADR: out-js-inside-the-output-root
title: The compiled JavaScript lives inside the output root
summary: `<output>/out-js` replaces the sibling `<output>/../out-js`, so a run's artefacts are private to it

**Context.** `bynk test` ran `tsc -p <output>/tsconfig.json`, whose `outDir` was `../out-js`, so the executed JavaScript lived beside the output directory. Two runs whose `--output`s shared a parent (every system test passing `--output <tmp>/<name>`, or two parallel `bynk test` invocations) shared one `<parent>/out-js`. Each run's `tsc` overwrote the other's tree while `node` was reading it, so a run could execute another project's JavaScript or a half-written tree. That was the root cause of the flaky `a_detached_failure_in_a_cron_or_queue_handler_is_counted` (#4000): the runner's stream ended without `run-end` and the run exited 0 with an incomplete report. Tests worked around it by giving each run its own parent directory, a convention each new test had to remember.

**Decision.** The compiled JavaScript is written to `<output>/out-js`. The emitted `tsconfig.json` sets `"outDir": "./out-js"` and excludes `out-js` from `include`, so the compiled tree is never re-read as input. The coverage variant of the config follows. `mutate`'s per-job output directories and every consumer of the tree (the coverage remap takes the root as a parameter) use the new location.

**Consequences.** A run's artefacts are private to its output directory, and concurrent runs cannot interfere whatever their parent is; a regression test runs four concurrent `bynk test` invocations with sibling `--output`s. The layout change is breaking for anything that read `../out-js`, which `upgrading.md` records. Because the tree is now under the output root it is compiler-owned: a later compile into the same `--output` prunes the `.js`, `.map` and `.json` files it did not write, which the old sibling tree survived (a `bynk test`, then `bynk build`, then run `out-js` sequence would break). Existing `.gitignore` entries for `out-js/` stay harmless (`out/` already covers the new location). The sibling layout was dropped rather than keyed per output directory because a per-run name beside the output would still leave the tree outside the directory a user passes and cleans, and would need its own naming and cleanup rules.
