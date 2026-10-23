# 0593 — Withdrawn versions

- **Status:** Accepted (v0.395)

**Context.** Packaging promises that yanking never deletes, but a legal takedown
or confirmed malware must remove an archive. The only way was to delete the file,
and every client then failed on fetch as `bynk.registry.unavailable` ("has no
archive for …"), which can't be told from a misconfigured registry and gives no
reason. The first-party registry track (`design/tracks/first-party-registry.md`
§6, D8) settled on an operator tombstone: the line stays, yanked, with an
unsigned `withdrawn` reason, and the archive is deleted. This is its protocol
half (R5a.1); the hosted service's tombstone is R5a.2. It amends ADR 0563 (the
protocol's mutable fields) and ADR 0570 (the publish API's refusals).

**Decision.**

- **A. `withdrawn` is an unsigned, optional reason on the line, and implies
  `yanked`.** A line may carry `"withdrawn": "<reason>"`, at most 500 characters,
  rendered only when set. Like `yanked` and `sig` it is outside the signed
  payload, so a withdrawal re-signs nothing; ADR 0563's "only `yanked` may change"
  (already amended for `sig`) now admits `withdrawn` too. A withdrawn line must
  also be yanked, and its archive must no longer be served. The protocol version
  stays 1: an older client ignores the field, sees a yanked version, and fails on
  fetch as `unavailable` — no reason, but nothing wrong. Rejected: signing the
  withdrawal (a chain or delegate key at every takedown, for no real protection,
  since whoever serves the index can already withhold the archive).
- **B. The client never chooses a withdrawn version, not even a locked one.**
  `select::choose` skips withdrawn lines (whether or not a mirror kept them
  yanked) and, when the locked version still fits but is withdrawn, returns
  `NoVersion::Withdrawn` rather than silently moving the lock. The fetch then
  fails as the new `bynk.registry.withdrawn`, naming the package, version,
  registry and reason, with a note pointing at `bynk update <alias>`. A locked
  version whose tree is already in the store takes the no-network path and keeps
  building. This deliberately deviates from the track's D8 text, which named
  `bynk.registry.unavailable`: a distinct code lets scripts and CI tell a
  takedown from an outage.
- **C. A withdrawal is attributed, and its reason sanitised.** Read through a
  mirror, the failure adds "read through the mirror `<name>` — the origin wasn't
  consulted": a mirror can add an unsigned field, but it could already withhold
  the archive, so attribution keeps that from looking like the origin's word. The
  reason is untrusted text, printed with control characters removed, cut to 500
  characters (with `…`), and quoted.
- **D. `bynk registry withdraw <dir> <org/package> <version> --reason "…"`.**
  It sets `yanked` and `withdrawn` on the line through a temporary file and a
  rename, then deletes `dl/sha256/<hex>` unless another line not itself withdrawn
  names the same tree, in which case the archive is kept with a warning. The
  reason must be one line (no control characters) of at most 500 characters,
  stored trimmed. Everything that can fail (the index reads, the shared-tree
  scan, the checksum) runs before the index is written, so a refusal changes
  nothing. It is final: no `--undo`, and an already-withdrawn version is
  refused — except that when an interrupted run left its archive behind (the
  index written, the delete failed), running it again deletes the archive, so
  a takedown can always be finished. It works on any registry directory, but it
  rewrites the index from its own process, as `bynk registry yank` does: behind
  `bynk registry serve` it must run when no publish is in flight, or a publish
  landing mid-run loses its line.
- **E. Nothing revives a withdrawn version.** `admit` refuses one with the
  `withdrawn` refusal — before the same-tree retry of D10, which must not quietly
  restore a takedown — so `bynk registry add` fails and the publish API's `PUT`
  answers `410 withdrawn`. `yank_line` refuses to un-yank one, so `bynk registry
  yank --undo` fails and `/unyank` answers `409 withdrawn`. Yanking it again is
  harmless. The protocol page's refusal table gains both rows.
- **F. Conformance checks withdrawals.** A read-only check, `read.withdrawn`,
  runs on the `--package` a `--read-only` run reads: every withdrawn line, as
  served, must be yanked and its archive must answer `404`; a package with none
  passes with a detail saying so. A withdrawn line's archive may still be served
  when a live version of the same package names the same tree (D's shared
  archive); a tree shared with another package can't be seen by a one-package
  check, and is reported as still served. `read.archive` now fetches the newest version
  that isn't withdrawn, and is skipped when every version is. A full run can't
  create a withdrawal (it is operator-only, with no API endpoint), so the `410`
  and `409` are covered by the reference server's tests.

**Consequences.** A removal is explicit, explained and checkable, and static,
reference and hosted registries speak the same field. A user whose build
suddenly fails is told the version, the reason and the fix, and a tree already
in their store keeps building — the registry can't reach into a machine, by
design. A mirror or a compromised store can inject a takedown message, attributed
to it, but gains no power to substitute content. No lock, project model,
checker, emitter or runtime change; the client and the reference tooling
change only in `bynk/src/registry/` and `bynk/src/fetch.rs`.
