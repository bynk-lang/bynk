---
level: patch
changelog: `bynk test`, `bynk dev` and `bynk deploy` install the build's npm packages with your own package manager (`--no-install` opts out)
---

## ADR: automatic-npm-install-with-the-users-package-manager
title: Install a build's npm packages automatically, with the user's own package manager
summary: bynk test, dev and deploy install missing npm packages into the generated directory with the detected package manager; `--no-install` restores the preflight stop

**Context.** Since ADR 0799 C a generated build can depend on npm packages: the
toolchain's own (`postal-mime` for inbound email, `postgres`, `mysql2`) pinned
exactly (ADR 0808), and an adapter's, approved first (ADR 0575). Only `bynk test`
installed them, and only with `npm`; `bynk dev` and `bynk deploy` stopped with a
preflight naming `npm install`. Two decisions were open (#3350): whether bynk
installs automatically, and which package manager it uses.

**Decision A: install automatically.** `bynk test`, `bynk dev` and `bynk deploy`
share one routine (`bynk_driver::npm_install`) that installs into the generated
directory (`out`, `.bynk/dev`, `.bynk/deploy`), before wrangler runs, only when a
package the emitted `package.json` lists is missing from `node_modules`. A warm
re-run runs nothing and needs no network. Lifecycle scripts are always off.

- A failed install stops with the tool's error, the command and the directory, so
  the user can run it by hand.
- `--no-install` (and `BYNK_NO_INSTALL=1`, for CI) on `bynk dev` and `bynk deploy`
  restore the preflight stop, naming the *detected* command. `bynk test` always
  installs, as before.
- `--dry-run` never installs: it reports what it would install, and with which
  tool. It outranks `--no-install`.
- The lockfile question dissolves: the directory is generated and regenerated, so
  whichever tool installs writes its own lockfile there, nothing is committed, and
  `bynk.lock` stays the source of truth.
- ADR 0575's approval is unchanged and still comes first. An adapter's npm
  package that is not approved is a compile error, so the build directory (and its
  `package.json`) is never written for it and no package manager runs; first-party
  packages are exempt. A test over the real binaries proves it.

**Decision B: the user's own package manager.** Which tool downloads the packages
changes nothing else, so bynk uses the one the user has. Detection is a pure
function over an injected view of the project's files and environment and the
`Toolbox`'s PATH lookup; the first match wins:

1. `BYNK_PACKAGE_MANAGER`, then `bynk.toml`'s `[build] package_manager`
   (`npm`, `pnpm`, `bun`, `yarn`); an unknown value is an error.
2. The `packageManager` field of the project root's `package.json`.
3. A lockfile at the project root: `pnpm-lock.yaml`, `bun.lock`/`bun.lockb`,
   `yarn.lock`, `package-lock.json`.
4. `npm_config_user_agent`.
5. The first of npm, pnpm and bun on PATH (npm first: it ships with Node).

A `packageManager` field or user agent naming a tool bynk cannot install with is
skipped rather than refused, since neither is a setting bynk owns. Commands:

- npm: `install --no-audit --no-fund --ignore-scripts`.
- pnpm: `install --ignore-scripts --ignore-workspace`, so a `pnpm-workspace.yaml`
  above the generated directory does not adopt it as a member.
- bun: `install --ignore-scripts`.
- Yarn: `install --ignore-scripts --non-interactive`, **classic (v1) only**. Yarn
  Berry's default Plug'n'Play leaves no `node_modules`, which breaks wrangler's
  bundling and the test runner's `out-js/node_modules` link. Forcing
  `nodeLinker: node-modules` would mean writing a `.yarnrc.yml` and relying on
  Corepack to pick the Berry version, which could not be verified here, so
  Berry is refused (`yarn --version` of 2 or later) with a message naming the
  alternatives. Supporting it later is a contained change to one function.

pnpm, bun and Yarn classic were each run for real (installing `postal-mime` below a
`pnpm-workspace.yaml`) and the tests do so wherever the tool is installed.
`bynk doctor` reports the detected package manager and the rule that chose it as an
informational row (not a failure: only a project with npm dependencies needs one).

**Consequences.** `from email`, Postgres and MySQL projects run with no manual
`npm install`, with pnpm and bun users never needing npm. A user who provisions
`node_modules` themselves, or runs offline in CI, opts out with `--no-install`.
The first run of a build with a missing package needs network access. The `npx`
fallbacks for `tsc`, `tsx` and `wrangler`, and doctor's `npm install -g` hints, are
untouched; they could reuse this detection as a follow-up.
