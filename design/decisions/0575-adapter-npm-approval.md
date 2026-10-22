# 0575 — A fetched dependency's adapter npm packages enter the build only once the project approves them, per package and range, enforced by the compile

- **Status:** Accepted (v0.386)

**Context.** Since slice 3 (#2255) a dependency's adapter `requires` fold into the consumer's `out/package.json`,
and the next `npm install` runs whatever those packages run. ADR 0020 set only a direction — dependencies are
pinned and "surfaced for review" — and left open what a consumer must approve. In practice the `requires` line
sits in a fetched tree no one opens, and a `bynk update` or a new version in range can add an npm package
silently. Proposal #2296, decisions A–F.

**Decision.**

- **Per row (A).** `bynk.lock`'s `[[npm]]` rows gain `approved = true`, keyed by the npm `package` and `range`.
  A new package or a changed range is a new, unapproved row; another adapter requiring an approved
  `(package, range)` keeps it approved, with a notice. Per-adapter approval (noise without new risk) and
  per-name approval (lets a major-version range change through) were rejected.
- **The compile enforces it (B).** A **reached** adapter of a fetched (git or registry) package whose requirement
  isn't approved is `bynk.requires.unapproved`, raised where dependencies are folded into the compile, with the
  shortest dependency chain and `bynk approve <package>` as notes. So `bynk check`/`test`/`dev`/`deploy`,
  `bynkc` and the LSP agree, with or without `--locked`. An unreached adapter can add nothing, so its pending
  row is no error. Enforcing only in `bynk`'s resolve step (or only under `--locked`, as track §7.2 first
  read) was rejected: `bynkc` and the LSP never run it.
- **Own code is exempt (C).** The project's and workspace members' own adapters need no row, as before. A row
  every requirer of which is a path package — confined to the root and workspace (D12), so the author's own
  files — needs no approval and none is written, so a lock from before this slice reads back byte-identical
  and `--locked` still holds (amending #2296 C's "written approved", after review). Fetched code needs
  approval.
- **`bynk approve` (D).** Lists each pending row with its range, chain and npm page; asks `[y/N]` per row on a
  terminal (refusing off one); `<package>` approves that package's rows, `--all` everything pending, `--list`
  shows rows as pending, approved or own. It rewrites only `approved` and never resolves. After `bynk add`,
  `fetch` or `update` writes pending rows, the same prompt runs on a terminal, or the rows and the command are
  printed; the command succeeds either way. `check`, `test`, `dev` and `deploy` never prompt.
- **Adoption (E).** An absent `approved` reads as `false`, so a slice-3 lock reaching a fetched adapter fails
  until approved — `bynk approve --all` adopts it in one step. Grandfathering existing rows was rejected:
  approving on the user's behalf is what this replaces. The lock stays version 1; `approved` is an optional key.
- **The boundary (F).** An approval says which npm packages, at which ranges, someone else's code may add to
  `out/package.json`. It is not an audit: tarball contents and integrity belong to npm's lockfile, install
  scripts to npm's and the user's policy, and the adapter's TypeScript binding ships inside a checksummed
  package the project already chose. Because approval is per `(package, range)`, a fetched package that later
  requires a pair the project already approved for another fetched package enters with only a notice.

**Consequences.** No npm package from fetched code reaches a build unreviewed, and the review is recorded once, in
the committed lock, for CI and teammates. Projects already consuming a dependency's adapter take one
`bynk approve` on upgrade. ADR 0020 is superseded.
