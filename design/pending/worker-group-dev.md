---
level: minor
probes: diagnostic_coverage, explained_codes
changelog: `bynk dev` serves a `[deploy.workers.<key>]` group as one Worker on one port (`--context` naming a member serves the whole group), and `bynk dev inspect` lists and reads the agents of every member; `bynk.deploy.worker_groups_unsupported` is removed (#3562, G6)
---

A group is one Worker, so dev runs it as one `wrangler dev` with one port and one
inspector, named by the group's key. `--context` naming a member selects the Worker
that holds it and says so, since the rest of the group cannot be left out of a Worker.
A binding to a renamed group's script is a project Worker, not a foreign one to stand
in for.

`bynk dev inspect` reads one registry per Worker, so a single listing returns every
member's instances. Each request is still signed for the member that answers it (a
member decides ownership before it verifies, so a request for another member's agent
passes it by): the listing for the first member that hosts an agent, and a read for the
owner of the class. A dev build of a group stages `bynk-inspect.json` beside its entry
naming every member and the agent classes it hosts (a deploy build has none), and the session file
carries them. The listing's `context` is the member that owns each class. The
temporary `bynk.deploy.worker_groups_unsupported` refusal is removed, with the
function and its tests. The self-binding stays in the Worker's `wrangler.toml`
(G3-3 is not planned).
