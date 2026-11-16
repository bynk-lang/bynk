---
level: patch
changelog: A fixed asset in one member of a `[deploy.workers]` group that sits at the path of a literal HTTP route in another member is refused (`bynk.asset.shadows_route`, extended across a group's members) (#3562, G2b)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

A group's members share one assets directory, and the platform answers an
asset's path before the Worker runs, for every method. So one member's fixed
asset makes another member's literal route at that path as unreachable as it
makes the same member's own, which `bynk.asset.shadows_route` already refused.
The check now runs across the members: one error per route, at the route's
handler, naming the member whose asset shadows it. A parameterised route that
matches stays out of scope, as it is within a context, and a context's own
Worker (no group) is unchanged.
