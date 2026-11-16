---
level: patch
changelog: Two members of one `[deploy.workers]` group that declare the same HTTP route or consume the same queue are refused (`bynk.deploy.route_conflict`, `bynk.deploy.queue_consumer_conflict`); two members may still share a cron expression (#3562, G2b step 2c-3)
probes: diagnostic_coverage, emit_diagnostics, explained_codes
---

A group is one Worker with one `fetch` and one `queue` entry, so a route and a
queue each belong to one member. The checks mirror the rules a single context
keeps among its own services (`bynk.http.duplicate_route`), with the same keys.
An email address needs no group check: a project already refuses two contexts
claiming one (`bynk.email.duplicate_address`).
