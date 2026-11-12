---
level: patch
changelog: The toolchain's `tsc`, `tsx` and `wrangler` fallbacks and install hints follow the detected package manager (`pnpm dlx`, `bunx`, `pnpm add -g`, and so on), not `npx` and `npm install -g`; `bynk doctor` names the runner it used
---

## ADR: package-manager-tool-runners
title: The toolchain's tool fallbacks and install hints follow the detected package manager
summary: tsc, tsx and wrangler run through the detected tool's runner, and hints give its install commands

**Context.** The package manager detection that installs a build's npm packages
(npm, pnpm, bun or Yarn classic, by a fixed order) left the toolchain's own tools
hard-wired to npm: `tsc`, `tsx` and `wrangler` fell back to `npx`, and every hint
said `npm install -g` or `npm install --save-dev`. A pnpm- or bun-only user was
told to use npm and had no fallback when `npx` was absent.

**Decision.** The same detection (`npm_install::detect`, unchanged, with no second
detector) chooses the tool for these fallbacks and hints. It runs over the same
injected `Project` and `Toolbox` views, so each rule is unit-tested without a real
tool.

- A package's binary is run with `npx --yes -p <pkg> <bin>` (npm),
  `pnpm dlx --package <pkg> <bin>` (pnpm) or `bunx --package <pkg> <bin>` (bun).
- Hints use `npm install -g` / `npm install --save-dev`, `pnpm add -g` / `pnpm add -D`,
  `bun add -g` / `bun add -d` and `yarn global add` / `yarn add -D`.
- Yarn classic has no runner. Its fallback is `npx` when that is on PATH, and
  otherwise the install command alone, which is today's message. A detected tool
  that is not installed falls back to `npx` the same way. With nothing detected,
  or an unknown setting, the tool is npm, as before.
- `Provenance::Npx` becomes `Provenance::Runner(PackageManager)`, so `bynk doctor`
  reports the runner it used ("provisionable via pnpm dlx"). In `--format json`
  the provenance token is `npx`, `pnpm-dlx` or `bunx`; the shape is unchanged
  and `npx` is still the value for npm, so existing consumers keep working. The
  once-only warm-up of `bynk dev` (#1957) applies to every runner, and the
  stale-cache hint stays npx-only, because only npx keys its cache on the spec.

Verified for real in a container (pnpm 10.28.0, bun 1.4.4, npm, yarn 1.22.22):
`pnpm dlx --package typescript@5 tsc --version`, `bunx --package typescript@5
tsc --version` and `npx --yes -p typescript@5 tsc --version` each printed
`Version 5.9.3`, so a pinned version works through both new runners without a
`--yes`; `pnpm dlx --package tsx tsx --version` and `bunx --package tsx tsx
--version` each ran `tsx`. The unit test `real_runners_run_a_pinned_tsc` repeats
this when `BYNK_REAL_RUNNERS=1` is set, and skips a tool that is absent. The
install commands in the table are the tools' documented forms and were not run,
since they alter global state.

**Consequences.** A pnpm or bun user sees and gets their own tool. Yarn classic
users keep `npx` where they have it. Hand-typed `npx wrangler secret put` and
`npx wrangler delete` hints in `bynk deploy` output, and the same in the book's
adoption and deploy guides, still say `npx`: they are copy-paste commands for a
user's shell, outside the fallbacks and hints this covers. Hover, completion,
semantic tokens and signature help are unchanged: no language surface changes.
