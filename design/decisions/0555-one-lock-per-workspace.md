# 0555 — A workspace has one `bynk.lock`, and one version of each package

- **Status:** Accepted (v0.379)

**Context.** Track §4.2 gives a workspace one root lock; #2223's comment point 8 deferred whole-workspace
resolution to this slice.

**Decision.** A member's lock is its workspace root's `bynk.lock`, resolved across every member's
manifests, so members agree on one commit of each package; reaching one package at two revisions (or from
git and a path) is `bynk.package.version_conflict`. Each member's graph is still compiled per member. A
standalone project's lock sits beside its `bynk.toml`.

**Consequences.** Running any `bynk` command in any member keeps the whole workspace's lock current.
