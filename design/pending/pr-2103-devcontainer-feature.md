---
level: patch
changelog: A Dev Container feature, `ghcr.io/bynk-lang/features/bynk`, installs `bynk`, `bynkc` and `bynkc-lsp` from one checksum-verified release plus `wrangler@4` and TypeScript, and the repo ships a `.devcontainer/` for contributors
---

## ADR: devcontainer-feature-in-repo
title: The Dev Container feature lives in this repo and publishes after a release
summary: features/src/bynk, tested by the devcontainer CLI, published to ghcr.io/bynk-lang/features after a successful Release run

**Context.** Codespaces and Dev Containers users start with no Bynk tools, and
the tools they need (`bynk`, `bynkc`, `bynkc-lsp`, Node, `wrangler`, `tsc`)
must agree on versions. Release binaries and a `SHA256SUMS` already ship with
every GitHub Release. A Dev Container feature can live in its own repository
(the usual layout) or in this one (#2103).

**Decision.** The feature lives in this repository, in the devcontainer CLI's
project layout: `features/src/bynk/` (`devcontainer-feature.json`, `install.sh`)
and `features/test/bynk/` (scenarios run by `devcontainer features test`).
`devcontainer-feature-publish.yml` publishes it to
`ghcr.io/bynk-lang/features/bynk` from `workflow_run` on a successful,
tag-pushed Release run, or from a manual dispatch on `main`. It never runs on a
pull request. `on: release` cannot be the trigger: release.yml creates the
Release with the workflow token, and events that token causes start no
workflows. The feature carries its own semver (`1.x`); its `version` option
chooses the Bynk release, so a new Bynk release needs no new feature version.
The install script resolves one release tag (`latest` follows the github.com
redirect, so it is not subject to the API rate limit), downloads that release's
`bynk-<tag>-<target>.tar.gz` and `SHA256SUMS`, refuses an archive whose
checksum is missing or wrong, and installs all three binaries from it. It
supports linux x86_64 and aarch64, the release matrix's Linux targets.

**Consequences.** Feature changes are reviewed and CI-tested with the code
whose releases they install. CI runs the scenarios when `features/`,
`.devcontainer/` or a workflow changes, and weekly, because a new release or
npm major can break the feature with no repo change. The first publish creates
a private GHCR package that an org admin must make public once. A feature
change users should receive needs a `version` bump in
`devcontainer-feature.json`, because the publish skips versions GHCR already
has.

## ADR: devcontainer-feature-node-handling
title: The Bynk feature installs after the Node feature instead of installing Node
summary: installsAfter ghcr.io/devcontainers/features/node; npm tools are pinned majors; bynk doctor --strict is the acceptance check

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

## ADR: devcontainer-repo-dogfooding
title: The repo's .devcontainer runs the feature's install script from the working tree
summary: Node and Rust features plus features/src/bynk/install.sh on create, because the spec rejects local features outside .devcontainer/

**Context.** DECISION C of #2103 asks for a contributor `.devcontainer/` that
uses the feature plus a Rust toolchain. The Dev Containers spec accepts a local
feature reference only inside `.devcontainer/`. The CLI rejects
`../features/src/bynk` and a symlink to it. The GHCR reference does not resolve
until the first publish, and it would test the published feature rather than
the branch.

**Decision.** `.devcontainer/devcontainer.json` uses the Node and Rust features
and runs `features/src/bynk/install.sh` as root in `onCreateCommand`, with the
options a features entry would pass as environment variables. Then
`postCreateCommand` runs `bynk doctor`.

**Consequences.** The contributor container always exercises the install
script as it is on the branch. The `devcontainer-feature.json` metadata
(`installsAfter`, the bundled extension) is exercised by the feature test
instead.
