# 0499 — The Bynk feature installs after the Node feature instead of installing Node

- **Status:** Accepted (v0.351.2)

**Context.** `bynk test` and `bynk dev` need Node 22 or newer, `tsc` (or
`tsx`) and `wrangler@4`. The official Node feature already installs and
maintains Node well.

**Decision.** The feature does not install Node. It declares `installsAfter:
["ghcr.io/devcontainers/features/node"]` and installs `wrangler@4` and
`typescript@7` (the majors bynk is verified against, `WRANGLER_MIN` and
`TYPESCRIPT_MAJOR_TESTED`) with npm. Each is behind a boolean option that
defaults to true. TypeScript is not in the proposal's option list. It is added
because `bynk doctor --strict` counts an npx-provisionable `tsc` as a warning,
and an all-green `--strict` is the proposal's acceptance check. If either tool
is on and `npm` is missing, the install fails and names the Node feature.

**Consequences.** A container with both features passes `bynk doctor --strict`,
and the feature test asserts it. A user who wants only the binaries sets both
options to false and needs no Node.
