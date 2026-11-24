# 0904 — A wrangler that exits before reading its piped stdin is reported by its exit, not by the broken pipe

- **Status:** Accepted (v0.652.1)

**Context.** `bynk deploy` and `bynk migrate` pipe a secret's value to `wrangler secret put` (and the sweep key
to `secret put`/`delete --name`). When wrangler failed early (auth, config, a bad Worker name) it exited without
reading stdin, the write failed with EPIPE, and `bynk` reported `Broken pipe (os error 32)` in place of what
wrangler said and how it exited (#4103).

**Decision.** `Wrangler::feed` treats a `BrokenPipe` from the stdin write as wrangler's to explain: it goes on to
`wait_with_output` and reports `WranglerError::exited(..)` with wrangler's streams, rendered as for any failed
exit (stderr, then stdout, then the per-call fallback). If wrangler exits 0 without having read the whole value,
the broken pipe is still an error, because the value may not have been set. Any other write error is unchanged.

**Consequences.** The text of a failure case changes from a generic I/O error to wrangler's own. The pinned
message fixture (`cloudflare_messages.txt`) is unchanged: its stand-ins read stdin. A deterministic adapter test
(a stand-in that never reads a value larger than the pipe buffer) covers the new shape.
