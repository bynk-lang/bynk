# 0627 — The editor approves pending npm requirements per package, found in driver output

- **Status:** Accepted (v0.416)

**Context.** `bynk approve` is how a project admits the npm packages a git or
registry dependency's adapter brings into the build (#2296). In the editor a
pending requirement was only discovered when a build stopped on it, and
approving it meant a terminal. Approval is a supply-chain decision: the next
`npm install` runs whatever an approved package runs.

**Decision.** (A) The review approves per package. Approve-all exists but is a
secondary button behind a modal confirmation that names the packages, and is
absent when only one package is pending. It approves by name (`bynk approve
<package>` for each), never `bynk approve --all`, after re-listing and finding
exactly what the review showed: a `fetch` elsewhere while the panel was open
cannot slip an unseen package in behind the confirmation. (B) Pending requirements are detected
from the driver's own output during `check`, `dev` and the deploy plan (a
`bynk.requires.unapproved` diagnostic), never by polling; on a hit the extension
asks the driver for `bynk approve --list --format json`, so the driver stays the
single source of truth and the extension reads no lock file to decide what is
pending. The listing marks each pending row `new` or `range_changed` so a review
shows what changed since the last approval, not everything; the editor also
remembers the ranges it last saw approved (the lock drops a superseded approved
row), to say "was `^5.2.0`".

**Consequences.** One new driver surface, `approve --list --format json`
(`--format` requires `--list`). A real deploy runs in a terminal the extension
cannot read, so a pending requirement is found by the plan that precedes it. A
`bynk: check` task's output also goes to a terminal, so its failure only
prompts a listing. The language, checker, emitter, runtime and `bynk-lsp` are
unchanged.
