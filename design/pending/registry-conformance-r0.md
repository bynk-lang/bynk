---
level: minor
probes: diagnostic_coverage
changelog: First-party registry R0 — `bynk registry conformance <index-url>` checks any registry against the protocol, from `config.json` and its key chain through publish, read-back, refusals, auth, the upload cap, yank and a fresh client's resolution, each check citing the protocol section it holds the registry to (`bynk.registry.nonconformant` on a failure; `--format json` for CI; `--read-only` publishes nothing). A publish retried with the same tree is now idempotent — `200` with the existing line, where `bynk registry serve` used to answer `409` — and `bynk registry add` of an identical package changes nothing
---

## ADR: registry-conformance-suite
title: A registry is held to the protocol by `bynk registry conformance`, run against its URL
summary: First-party registry track R0 — a suite in the `bynk` CLI that judges a server with the client's own verifying code; one throwaway package per run, left yanked, or `--read-only`; stable check ids citing protocol sections; JSON for CI; the reference server passes it in CI

**Context.** The protocol's behaviour was pinned only by the `bynk` client's own integration tests, which drive
the client against fixtures; nothing checked a server from outside. A second implementation is coming — the
hosted first-party registry, written in Bynk (#2308) — and private operators may write their own. Proposal
#2311, decisions A, B, C, E and F.

**Decision.**

- **In the CLI (A).** `bynk registry conformance <index-url>` ships with `bynk`, so an operator can run it as
  easily as CI. It judges a server with the client's own code — `config.json` and key-chain reading, line
  verification, archive fetching and checksumming, version selection — and sends directly only what a
  client never would: no token, a bad or narrow token, a mismatched URL, a junk or traversing archive, a path
  dependency, an oversized body.
- **Throwaway publishes (B).** A full run publishes one package, `<org>/conformance_<utc>_<hex>`, under an
  organisation the operator names, and leaves every version it published yanked. Indexes are append-only and
  nothing is deleted, so the package stays; the report names it. `--read-only` publishes nothing: it checks
  `config.json` and reads back an existing `--package`. A cleanup endpoint was rejected: it would add a delete
  to a protocol whose integrity rests on never deleting.
- **The checks (C).** Stable ids in groups — `config`, `publish`, `refuse`, `auth`, `limits`, `consume`,
  `yank`, `errors` — each citing the protocol section it enforces, listed in the protocol page's Conformance
  table. `auth.scope` needs a narrow token (`--narrow-token-env`) and is skipped without one. The redirect
  rule (a client's duty) and caching (an operator's choice) aren't checked. Where the suite and the spec
  disagree, the spec wins.
- **Report (E).** One line per check, or `--format json` for CI; exit `1` when any check failed, each
  failure reported as `bynk.registry.nonconformant` with its check and section.
- **In CI (F).** The reference server (`bynk registry serve`) passes every check, and a proxy that breaks one
  rule — a conflicting retry, a refusal without JSON, a lying `201`, a corrupted signature, an altered tree —
  fails exactly that check.

**Consequences.** "Speaks the Bynk registry protocol" has one executable definition. The hosted service runs it
on every PR and before every deploy (#2308 §7), and each later protocol addition (`withdrawn`, key delegation
and revocation) extends it.

## ADR: idempotent-publish-retry
title: A publish retried with the same tree is `200` with the existing line; only a different tree is `409`
summary: Amends ADR 0570 — a `PUT` of an existing version whose upload has the line's canonical-tree checksum answers `200` with the existing line and changes nothing; `bynk registry add` of an identical package is a no-op; the hosted registry's write path relies on it

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
