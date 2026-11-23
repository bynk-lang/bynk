# 0876 — A feature freeze on bynk until the back-half paydown's Phase 1 exits

- **Status:** Accepted (v0.639.7). **Lifted on 23 November 2026**, when every exit criterion in the back-half paydown track's §8 Phase 1 column read met (spine bynk-lang/bynk#3676).

**Context.** The back-half debt review (`design/reviews/2026-11-17-back-half-debt-review.md`) measured the back of the toolchain at v0.639.1 against v0.291.0. Over seven weeks, production code in `bynk-model`, `bynk-emit`, `bynk-ts`, `bynk-driver` and the CLI's deploy code roughly tripled, from about 37k lines to about 110k. The debt gauges meant to restrain that growth were being re-baselined instead. Only 7 of about 1,220 non-stamp commits were refactors. Paying the debt down while features keep landing means refactoring a moving target. Because the core language is largely settled, a pause is affordable. The maintainer paused feature development on 17 November 2026. This ADR records the terms and the condition for lifting it, so that lifting it is a decision rather than a drift.

**Decision.** From v0.639.1 until the exit criteria of the back-half paydown track's Phase 1 are met (`design/tracks/back-half-paydown.md` §8):

- **Allowed on `main`:**
  - the track's slices;
  - defect fixes, each with a test that fails without it. A fix to a documented feature that does not work counts, provided it adds no syntax or API;
  - security fixes and advisories;
  - dependency bumps;
  - CI, release and documentation changes;
  - retirements and status updates for tracks already built;
  - `vscode-bynk` work that needs no compiler change.
- **Not accepted for build:**
  - new language surface;
  - new standard-library APIs, including ones that need no emitter change;
  - new deploy resource kinds or binding kinds;
  - new build targets or service protocols;
  - new CLI subcommands.
- **Design work may continue.** Proposals and track documents can still be written and settled during the freeze. They are only not built until it lifts.

**Consequences.**

- **What lifts the freeze.** It lifts when Phase 1's exit criteria are met. That is a set of measured probe readings and structural conditions in the track's §8, not a date.
- **After the freeze.** Phases 2 and 3 of the track run alongside resumed feature work. The ratchets from Phase 0 then hold the line: a feature that needs a gated growth probe to rise must declare a reasoned `raises:` entry, which is recorded in `design/greenfield-status-raises.md`.
- **The cost.** Features that would have shipped in the meantime wait, and the language's public surface stands still for the freeze's duration.
- **Questions over scope.** If it is unclear whether a change is allowed, it is held for the maintainer's decision, not built.
