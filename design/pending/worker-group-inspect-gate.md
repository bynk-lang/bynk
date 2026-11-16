---
level: patch
changelog: A Worker group's entry and members carry no `bynk dev` inspection route in a deploy build, and in a dev build a member answers only its own agents' inspect requests (#3562, G6 groundwork)
probes: emit_diagnostics, ts_writes
---

The dev-only `/_bynk/inspect/` route is emitted only for `bynk dev`, so a deploy
build of a group has no route in the group's entry or any member's, and no
registry; a test pins that absence across the Worker's directory. For the day
`bynk dev` learns groups, a member now decides ownership before it verifies a
signature, as the internal call, event and migrate routes do: it answers an agent
of its own class, the first member that hosts an agent also answers the listing
and the group exports the registry class once, and the group's entry answers a
request no member claims as an unsigned one to a standalone dev Worker is
answered (`405` for a method other than `POST`, `401` otherwise). `bynk dev`
still refuses groups until then.
