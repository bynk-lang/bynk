# 0579 — A registry is held to the protocol by `bynk registry conformance`, run against its URL

- **Status:** Accepted (v0.387)

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
