# 0907 — A cron handler can take the schedule that fired it

- **Status:** Accepted (v0.655)

**Context.** A handler could not see which schedule fired it, since only the
generated entry sees `event.cron`, so a run key for `Idempotency.claim` either
repeated the schedule literal by hand or left the schedule out.

**Decision.** A. A cron handler takes up to two parameters, told apart by type, in
either order: one may be `Instant` (the scheduled time) and one may be
`CronSchedule` (the schedule). A second parameter of the same type, or any other
type, is `bynk.cron.bad_params`. A single `Tick { at, schedule }` record is
rejected (Cloudflare's event carries nothing else for it to grow into, and it makes
the common one-parameter case wordier), as is a fixed positional order (one more
rule for no gain). B. The parameter's type is exactly the prelude's `CronSchedule`, not `String`,
not another `Cron` refinement and not a type the unit declares under the same
name (that is `bynk.cron.bad_params`, since the entry hands the parameter a
schedule string): one spelling, and a `CronSchedule` widens to `String` wherever
it is read. C. The value is `event.cron`; under the single-space
rule it equals the expression as written. At `unit` and `integration` the test
address passes its own first argument; at `system` the driver already sends it, so
the address's shape is unchanged and `schedule` is not an argument of it. The
entry hands each handler `event.scheduledTime` and/or `event.cron` in declared
order; the brand is a type, so the plain string is cast to it at the call.

**Consequences.** While a handler has exactly one literal schedule the parameter
always holds the same value; it still removes a repeated literal from run keys, and
becomes the only way to tell runs apart once a schedule can come from a constant or
a handler can carry several. In a worker group two members may share an expression
and both run, in one Worker and so one idempotency store (the
`<worker>-idempotency` database): a key built from `schedule` and `at` alone
collides, so the recipe puts the service name in the key.
