---
level: minor
changelog: A sum variant added inside an agent's stored state is an additive change in bynk.schema.lock
---

## ADR: schema-lock-variant-additive
title: A sum variant added inside stored agent state is additive
summary: The lock accepts a new variant, bumping the class version; removal and rename stay refused

**Context.** ADR 0682 made a record field added with a default additive but, in its Decision F, kept a new sum variant non-additive. Adding `| Paused` to a sum held in `store last: Cell[Option[Event]]` therefore failed with `bynk.agent.non_additive_state_change` (`last?|Paused variant added`), although every stored value decodes: a value written before the variant existed cannot name it. The only way past was a `migrate` step that changed nothing. Proposed and agreed in #2691.

**Decision.**

- **A. A new variant is additive.** In `compare_paths`, the topmost new path whose node is a `variant` is recorded as additive, with the payload fields beneath it, which need no default. The classifier appends `<path> variant added` to the additive notes, so the change bumps the class's version and the contract diff (ADR 0640) reports it. Removing a variant, renaming one (a removal plus an addition), and changing an existing variant's payload stay non-additive. Reordering variants is not a change. This supersedes ADR 0682 Decision F.
- **B. A schema rule about stored data only.** Exhaustiveness is unaffected: a `match` over the sum must still cover the new variant, and extensible enums (ADR 0468) are treated as any other sum.
- **C. Rollback relies on `__v`.** The change bumps the version, so an older build reading a record that holds the new variant faults it as `NewerSchema` and never writes it back (ADR 0545). The docs say to roll forward.
- **D. Events are unchanged.** The event registry records a named type by name only, so it never sees a variant added inside one.
- **E. Not carried to wire contracts.** For stored state only the newer build writes the variant and an older build is stopped by `__v`. A caller built against an earlier service contract has no such guard, so a variant added to a service-visible type stays a contract change, whatever the contract hash does for defaulted fields.
- **F. No diagnostic code is added or retired.**

**Consequences.** Adding an enum case to a state machine in agent state is a one-line change with no migration. Rollback after a record holds the new variant fails loudly instead of misreading it.
