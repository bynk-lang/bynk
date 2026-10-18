# 0498 — The Dev Container feature lives in this repo and publishes after a release

- **Status:** Accepted (v0.351.2)

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
