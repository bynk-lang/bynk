---
level: patch
changelog: The playground moves to its own repository, [bynk-lang/playground](https://github.com/bynk-lang/playground), served at [playground.bynk-lang.dev](https://playground.bynk-lang.dev) from a pinned release's browser artefacts
---

## ADR: playground-own-repository
title: The playground is extracted to its own repository and served from bynk-lang.dev
summary: bynk-lang/playground builds from a pinned release's bynk-web tarball, deploys as two Workers, and is checked by the org canary

**Context.** The playground lived in `playground/`, and `deploy-playground.yml`
built the compiler from source on every qualifying `main` commit, then
published it to two Cloudflare Pages projects (`playground.bynk-lang.org` and
`sandbox.bynk-lang.org`, ADR 0140). So every UX change to the app went through
this repository's review and CI, and the public playground ran whatever
compiler `main` had, released or not. #2285 prepared the split. Each release now
attaches `bynk-web-<tag>.tar.gz` (the bynk-wasm module, its glue and types, the
grammar wasm and highlight query), and the app can build from that tarball with
no Rust toolchain. The org canary gained a `kind: app` for the playground
(bynk-lang/.github, with bynk-lang/compat-board#16 and bynk-lang/examples#10).
On 2026-10-24 the maintainer asked for the playground to move to its own
repository, integrated with the canary, so its UX can iterate on its own. They
also reserved `bynk-lang.org` for the language and its documentation. The
playground's new origins were the maintainer's calls.

**Decision.**

- **A. `bynk-lang/playground`, with its history.** `playground/`'s commits were
  split out of `main` at f62dd6c64 and pushed as the new repository's first
  `main`. Their `#NNN` references were rewritten to `bynk-lang/bynk#NNN`, so
  they still name this repository's issues.
- **B. A pinned release, never a source build.** `BYNK_VERSION` names one final
  release (first v0.403.3). The app's build stages that release's `bynk-web`
  tarball, checked against the release's `SHA256SUMS`. The source-build scripts
  (`build:wasm`, `build:grammar`) did not move.
- **C. New origins, on Workers.** The app is served at
  `playground.bynk-lang.dev` and the execution sandbox at
  `sandbox.bynk-lang.dev`. They are two assets-only Workers, `playground` and
  `playground-sandbox` (on `bynk-lang.workers.dev`), replacing the two Pages
  projects. ADR 0140's two-origin boundary is unchanged; only the hostnames move.
  `bynk-lang.org` is now the language's and the documentation's.
- **D. The canary checks every release.** `bynk-lang/playground` is enrolled as
  `kind: app`. At each release it runs its own checks: type-check, unit tests,
  every gallery example compiled and run through that release's wasm, and the
  bundle. It passes their result to the org's `canary.yml` as
  `upstream-result`, with `bynk-ci` on its share service. When a newer release
  passes, it opens a pull request moving `BYNK_VERSION`. Merging that pull
  request is what deploys the new compiler.
- **E. The interface is the tarball, checked on both sides.** Here, ci.yml's
  `web-assets` job checks that the tarball holds what the playground stages, and
  that its glue still exports what the app calls (`init`, `bynk_compile`,
  `bynk_analyze`, `bynk_hover`, `bynk_complete`). The `Playground (type-check)`
  job, which type-checked the app against a debug bynk-wasm build, is gone with
  the app. A wider break, such as an export whose shape changes, now fails the
  playground's canary after the release instead of a PR here before it.
- **F. The order of the move.** The repository was created and its history
  pushed. Its CI went green on v0.403.3, and its first `main` push deployed both
  Workers. Only then did this repository delete `playground/`,
  `deploy-playground.yml`, the CI job, the npm-audit tree and the Dependabot
  entry. The docs site's playground links move to `playground.bynk-lang.dev`.
  The `.org` Pages projects are retired by hand.

**Consequences.** Playground changes are pull requests on `bynk-lang/playground`
and deploy on their own merge. A compiler change reaches the public playground
only in a release that has passed its canary, never straight from `main`. A
bynk-wasm change that breaks the app is caught here only at the tarball's
surface, and otherwise by the canary after the release. That is the cost of the
app moving on its own cadence. The share service (`share/`) moved too and is
still not deployed. ADRs 0140, 0241 and 0242 keep the old `playground/` paths
and `.org` hostnames as history.
