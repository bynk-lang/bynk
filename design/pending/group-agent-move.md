---
level: patch
probes: diagnostic_coverage, explained_codes
changelog: `bynk deploy` refuses to put a context that hosts agents on a different Worker script than it was last deployed to (into or out of a `[deploy.workers.<key>]` group, between groups, or after a group is renamed or re-keyed), since its agents' state would start empty; `--accept-fresh-agent <context>` starts it fresh and the old script is then reported as holding agent data (#3562, G5)
---

A Durable Object class's state belongs to its Worker's script, so moving an agent-owning
context to another script redeclares its classes where Cloudflare creates empty namespaces and
says nothing. The deploy plan now compares each context's planned script with the script its
ledger record names and refuses the move before pushing anything, `--dry-run` included
(`bynk.deploy.agent_move_refused`). A group stages `bynk-agents.json` (each member's agent
classes) so the plan can tell which members own agents; a context's own Worker is read from its
exported classes. `--accept-fresh-agent <context>` (repeatable, each context named) lets a move
start fresh: it warns, names the old script, records in the ledger that the script still holds
the agent state, and the orphan report then says that script is not empty. It cannot be used to
move a context out of a group that stays. ADR 0874 records the state-preserving move (the
three-step `exports` transfer, its outage window and open limits) as future work.
