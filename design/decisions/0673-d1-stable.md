# 0673 — D1 graduates to stable on its own gate

- **Status:** Accepted (v0.458)

**Context.** `D1` shipped experimental (ADR 0662 H) until its schema
migrations landed. ADR 0664 G kept it experimental after that, because two
language follow-ups would change what a call site looks like: decoding `Bool`
from `0`/`1`, and literal-only SQL. ADR 0666 closed both and left graduation as
a separate decision.

The other experimental items in `bynk.cloudflare` (`Service`, `R2`,
`RateLimit`, `EdgeCache`, `Queue`, `Analytics` and the new `Kv` operations)
wait for a different bar: the first-party registry (#2308), or another
first-party service, using them in production. The registry's design doesn't
use `D1`.

**Decision.**

- **A. `D1` is stable.** It leaves `CLOUDFLARE_UNIT.experimental`, so the
  capability, its `all`/`first`/`run`/`batch` operations and its `WorkersD1`
  provider come under the promise (§5 R-17, R-18). Its types (`SqlValue`,
  `SqlStatement`, `SqlRun`, `SqlError`) and `Json.decodeSql` were never on
  the list. `SqlError` stays extensible, so new D1 error kinds can still
  arrive in a minor release.
- **B. The deploy and migration contract is part of the promise.** That means
  ledger-provisioned databases, and `migrations/<database>/NNNN_<name>.sql`
  applied by deploy, dev and tests (ADR 0664).
- **C. A capability may graduate on its own gate.** When a capability's ADRs
  name the conditions for leaving experimental and those conditions are met,
  it graduates without waiting for first-party use. `D1`'s surface was settled
  across three slices, and its test double is a real SQLite database rather
  than a stub. The other capabilities keep the registry bar, because their
  ADRs set it.

**Consequences.**

- Projects drop `"bynk.cloudflare.D1"` from `[unstable] allow`. A leftover
  entry warns (`bynk.project.unstable_not_experimental`) rather than failing.
- Reshaping a `D1` operation now needs ADR 0116's deprecate-then-remove path.
  A typed `all[T]`, when a codec-carrying generic op exists, would be added
  beside the text-returning operations, not replace them.
- Cloudflare withdrawing or changing D1 still falls under D2's platform
  carve-out.
