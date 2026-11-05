---
level: patch
changelog: Record the published-set rule (publish only what an installed binary reaches; retired crates get a final onward-pointing release); `bynk-grammar` announces its final crates.io release
---

## ADR: published-set
title: Publish only what an installed binary reaches; retired crates point onward
summary: H-4 and H-5 of the crate-hygiene track; amends ADR 0099 and ADR 0100

**Context.** The crate-hygiene track (#2775, `design/tracks/crate-hygiene.md`) found the published set wider than what installs. `bynk-grammar` is published at every release, first in the release order, though only `bynkc`'s integration tests use it. ADR 0099 held it up as the model leaf crate. Each such crate is public surface the project promises to keep, and removing one without a rule risks stranding or breaking downstream pins.

**Decision.**
- **H-4, publish only what installs.** A crate is published only if an installed binary's normal dependency tree reaches it. Test and docs tooling is `publish = false` with a path-only workspace entry, as `bynk-testkit` is.
- **H-5, retired crates point onward.** A crate that leaves the published set gets a final release whose README and `description` say where it went. It is never yanked, so existing pins keep resolving. Because a crates.io README changes only by publishing, the onward-pointing release must ship before the crate is marked `publish = false` and removed from the release workflows' crate lists.
- **Amends ADR 0099.** `bynk-grammar` is no longer cited as the model leaf. The layering principle (dependencies point down; extraction is downward) is unchanged; `bynk-syntax` is the leaf the layering rests on.
- **Amends ADR 0100.** `bynk-render` is published because `bynk-driver` reaches it. That status is judged by H-4 like any crate's, so a later slice may unpublish it.
- It decides the fate of no other crate. Each slice that changes the published set cites this ADR.

**Consequences.** `bynk-grammar` ships a final release carrying the pointer, then becomes `publish = false` (slice C1). Later slices (the `bynk-ir` rename, D2, E3) cite H-4/H-5 rather than re-arguing them. The release workflows' hardcoded crate lists change in each slice that adds, removes or renames a published crate.
