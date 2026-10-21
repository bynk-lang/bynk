# 0559 — A reached dependency adapter's npm `requires` fold into the build, with exact-string conflicts

- **Status:** Accepted (v0.382)

**Context.** Slice 2a (#2223) refused a dependency adapter with `requires`, so a package could not wrap an
npm library. The fold that builds `out/package.json` (#2201) already refuses an unpinned range and two
different ranges for one package.

**Decision.**

- The refusal is lifted (an `asset` in a dependency stays refused). The existing fold runs over every adapter
  the build compiles, whichever package it came from. Slice 2a's reach-closure keeps only reached dependency
  units, so an unused dependency's adapter contributes nothing (P-5), and a dev-dependency's adapter reaches
  only a test build (#2240).
- The rules carry over unchanged: pinned ranges only (`bynk.requires.unpinned_dependency`), and two different
  range strings for one npm package — between the project's adapter and a dependency's, or two
  dependencies' — are `bynk.requires.conflicting_range`, naming both adapters as `org/package:adapter`.
- npm-style range intersection and a consumer override table are recorded as follow-ons, not built: the first
  needs a faithful npm-semver implementation, and the second hands the consumer a lever over code it didn't
  write, which is the adapter-trust slice's question (slice 5).

**Consequences.** One root `package.json` still serves every Worker. A conflict across packages is fixed by
aligning the ranges in source, upstream if need be.
