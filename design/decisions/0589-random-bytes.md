# 0589 — `Random.bytes`

- **Status:** Accepted (v0.392)

**Context.** `Random` offered only `uuid()` and `int(lo, hi)`, and `int` draws
from `Math.random()`. There was no way to get random bytes of real entropy for a
token, a nonce or a key.

**Decision.** `Random` gains `bytes(n: Int) -> Effect[Bytes]` over
`crypto.getRandomValues`, on all three platforms. `n` must be between 1 and
65,536 (Web Crypto's per-call limit); anything else fails the effect with an
error naming the bound, rather than clamping or looping. It stays on `Random`,
not `Crypto` (D6): it is the same capability family as `uuid()` and needs no new
`consumes`. `Random.int` is unchanged, and its docs now say it is not for
secrets and point at `Random.bytes`. Under test the `Random` double draws bytes
from the same fixed-seed generator as `int`, so a test is deterministic.

**Consequences.** Every emitted `bynk` surface binding and the `Random`
interface gain one operation; existing programs are unaffected. A program that
needs a secret now has the right primitive, and a reviewer can see `Random.bytes`
rather than `Random.int` at the call.
