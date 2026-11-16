---
level: minor
changelog: A `[deploy.workers.<key>]` group has one events fan-out for every member that publishes, so several members of a group may publish events; `bynk.deploy.multiple_publishers` is removed (#3562, G3-1)
probes: diagnostic_coverage, emit_diagnostics, explained_codes, ts_writes
---

Each publishing context had its own `__EventsFanout` class, so two publishing
members of a group would have exported two classes of one name, and a group was
limited to one publisher (`bynk.deploy.multiple_publishers`, temporary). A group
now has one fan-out. A member emits none of its own: its compose names the member
on each event it hands over (`publisher`), because two members may declare an
event of one name. The group's `events_fanout.ts` routes by `<publisher>/<type>`,
signs each delivery as the publisher the event names, and the group's entry
exports `__EventsFanout` once, so the one `wrangler.toml` declares the class and
its binding once. A context on a Worker of its own is unchanged.

The fan-out signs a delivery as the publisher its event names, so it signs only for
a member it was built for: an event naming any other publisher, or none, is logged
(`EventsFanout refused an event from an unknown publisher`) and dropped, never
signed. No Durable Object migration is needed for the class change, as no group
has ever been deployed.
