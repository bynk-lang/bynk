# 0500 — The repo's .devcontainer runs the feature's install script from the working tree

- **Status:** Accepted (v0.351.2)

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
