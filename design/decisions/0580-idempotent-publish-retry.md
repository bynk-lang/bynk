# 0580 — A publish retried with the same tree is `200` with the existing line; only a different tree is `409`

- **Status:** Accepted (v0.387)

**Context.** ADR 0570 answered any `PUT` of an existing version with `409 version_exists`. A publish whose `201`
was lost — a dropped connection, a timeout — then reads as a refusal on retry, though nothing is wrong. The
first-party registry's write path (#2308 §3.3, D10) needs retries to be safe, because its index file is a
projection written after the commit. Proposal #2311, decision D.

**Decision.**

- A `PUT` of a version the index already lists, whose upload's canonical-tree checksum equals the line's,
  answers **`200` with the existing line** and writes nothing. A different tree is still
  `409 version_exists`. `201` stays the answer for a new line.
- `commands::admit` compares the re-packed tree's checksum with an existing line's before refusing, so
  `bynk registry serve` and `bynk registry add` apply the same rule; `add` reports an identical package as
  "already present, identical".
- `bynk publish` already accepts `200` and verifies the line it reads back, so the client is unchanged.

**Consequences.** Publishing is safe to retry. A published version still never changes: the same tree is the
same version, and anything else is refused. The conformance suite checks both (`publish.retry-same-tree`,
`publish.conflict`).
