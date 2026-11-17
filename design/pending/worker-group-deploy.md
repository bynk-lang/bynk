---
level: minor
changelog: `bynk deploy` deploys a `[deploy.workers.<key>]` group as one Worker, in dependency order with the others, and `--worker KEY` selects a Worker by its directory; the ledger records a group Worker's contexts, and a member cannot be deployed alone (#3562, G4-1)
---

`bynk deploy` no longer refuses a project with a `[deploy.workers]` table (`bynk revert`
and `bynk dev` still do, until G4-3 and G6). A group is one Worker keyed by its
directory: it is planned, provisioned, pushed and recorded in the ledger under its
key, in Service-Binding dependency order with every other Worker, and a call between
its members is no edge (it is the Worker binding to itself).

- `--worker NAME` deploys one Worker by its directory name under `workers/` (a
  group's key, or a context's dashed name). Like `--context`, it assumes the Workers it
  binds to are live and refuses one that was never deployed. `--worker` and `--context`
  are exclusive.
- `--context X` for a member of a group is refused, naming the Worker that holds it
  (`context shop.a is in Worker core, which deploys as a unit: use --worker core`).
- The ledger records a group Worker's `contexts` and, when a group's `name` renames its script, the script name (additive; a ledger from before groups
  reads and is written back unchanged), and a removed group is reported as an
  orphan with its members (`orphan worker core (contexts shop.a, shop.b)`).
- A dependency's contract, an accepted older contract and an email forward's Worker are
  looked up under the Worker that holds the context, so a group's contracts are
  checked like any Worker's.
- `--confirm-delete` names a removed agent class by its Worker's key
  (`core.Cart`), as the plan prints it.
