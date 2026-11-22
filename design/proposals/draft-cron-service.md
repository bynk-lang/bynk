# Draft: the `from cron` service — review and proposed increments

> **Status: long-form draft** (`design/proposals/README.md` §"Drafting
> long-form proposals"). Committed temporarily for line-by-line review on
> bynk-lang/bynk#3869, not as an exception to the untracked-draft rule; promote
> to issues and delete once reviewed. It covers one language feature end to
> end and cuts the work into five increments, each written to the
> increment-proposal template so it can be lifted into its own issue.
> Increments 4 and 5 depend on increment 3's type; 1, 2 and 3 stand alone. No
> version or ADR number is pre-allocated.

## The feature under review

```bynk,ignore
service <Name> from cron {
  on schedule("<schedule>") (at: Int) -> Effect[Result[(), E]] {
    …
  }
}
```

Cron shipped in v0.10 (ADR 0002), long before `Duration` (ADR 0112),
`Instant` (ADR 0114), named constants (ADR 0678, 0826), `IsoDate` (ADR 0727)
and `.detach()` (ADR 0825). Most of what follows is the feature catching up
with the language that grew around it.

## What exists today (grounded)

- **Parser.** `on schedule(` takes a string literal only
  (`bynk-syntax/src/parser/declarations.rs:4297-4304`).
- **Checker.** `validate_cron_handler`
  (`bynk-check/src/context_checks.rs:6743`): at most one parameter, which must
  be `Int` (epoch milliseconds); the schedule must split into exactly five
  whitespace-separated fields (per-field validation deferred since v0.10,
  "[DECISION 4]"); the return must be `Effect[Result[(), E]]`. Duplicate
  schedules within a context are refused by exact string
  (`context_checks.rs:3384`). In a worker group two members may share an
  expression and both run (`bynk-check/src/group_triggers.rs:15`).
- **Test address.** `svc.schedule("<expr>", at)` (`bynk-check/src/checker/calls.rs:4519`),
  with `at` an `Int`; at `system` it drives the Worker's own `scheduled` entry
  (ADR 0522; `bynk-driver/src/project/tests_emit/system_support.rs`).
- **Emitter.** `scheduled` switches on `event.cron` and passes
  `event.scheduledTime` to handlers that declare it
  (`bynk-emit/src/emitter/workers_entry.rs:2015-2090`). On `Err` it calls
  `console.error("cron <expr> failed", result.error)` and returns normally. It
  has no `try`/`catch`; `ctx` is in the signature only when a handler needs an
  execution context (fixture `3541_detach_cron_queue`). The `default:` arm
  returns silently. A group entry runs every member whose cron matches
  (`workers_group.rs:660`). Every expression lands in `[triggers] crons`.
- **Actor.** Cron handlers default to `Scheduler` (`Internal`, identity `()`).

### What Cloudflare does (the target the feature compiles to)

From the Cron Triggers, `scheduled` handler and limits pages:

- Five fields only; no seconds, no year, no `@daily`-style macros, no `?`.
- Minute `0-59`, hour `0-23`, day of month `1-31` (+ `L`, `W`), month `1-12`
  or `JAN`–`DEC`, day of week **`1-7` with `1` = Sunday** or `SUN`–`SAT`
  (+ `L`, `#`). Names are case-insensitive. `* , - /` are valid in every field.
  Examples on the page: `0 18 * * 6L`, `0 18 * * friL`, `59 23 LW * *`.
- `*/45 * * * *` fires at :00 and :45, not every 45 minutes.
- Schedules run in **UTC**.
- `controller.cron` is "the exact cron expression string from your
  configuration. It must match character-for-character, including spacing."
- "The first `ctx.waitUntil` to fail will be observed and recorded as the
  status in the Cron Trigger Past Events table. Otherwise, it will be reported
  as a success."
- Retry behaviour for a scheduled invocation that throws is **not documented**;
  `noRetry()` appears only in the local-testing docs.
- Trigger changes take up to 15 minutes to propagate; runs can be missed
  meanwhile.

A correction to the initial review: the exact-string duplicate check is
**right**, not a gap. Cloudflare dispatches on the exact string, so
`"0 0 * * *"` and `"0  0 * * *"` are two triggers with two dispatch keys and
nothing is ambiguous. Increment 3 still proposes admitting only single-space
separation (DECISION 3C), and says plainly that this breaks schedules that
compile and fire correctly today.

## Sequencing

| # | Increment | Depends on | Layers |
|---|---|---|---|
| 1 | The scheduled time is an `Instant` | — | checker, test address, docs |
| 2 | A failed run is visible to the platform | — | emitter, test double, docs |
| 3 | A `Cron` refinement predicate and `CronSchedule` | — | syntax (predicate), checker, runtime, std, tooling |
| 4 | The handler can take its schedule | 3 | checker, emitter, test address |
| 5 | A constant names a schedule | 3 | parser, checker, IDE |

Docs drift found in review (stale `on cron "expr"` wording in diagnostics and
docstrings, "bare, after `cron`" in the guide, "an `from cron` parameter" and
"An cron handler" in the troubleshooting page and spec, no mention of UTC)
rides with increment 1, which touches the same messages and pages.

---

## Increment 1 — The scheduled time is an `Instant`

### Summary

- **Scope:** checker (`validate_cron_handler`, the `schedule` test address),
  diagnostics wording, book. Emitter and runtime untouched: `Instant` erases to
  the same epoch-millisecond `number` (ADR 0114 D1).
- **Addresses:** `at: Int` is the one place left where the language hands an
  author a timestamp as a bare `Int`, which ADR 0114 was written to end. The
  reference's "Cron has no built-in clock" is also stale: `Clock.now()` returns
  `Effect[Instant]`.
- **Realises:** `on schedule("0 * * * *") (at: Instant)`, composing with
  `Duration` arithmetic (`at - 1.hours`) and `Instant` comparison with no
  `Instant.fromEpochMillis` at the top of every handler.

### The surface

```bynk,ignore
service sweeper from cron {
  on schedule("0 * * * *") (at: Instant) -> Effect[Result[(), SweepError]] {
    let windowStart = at - 1.hours
    …
  }
}
```

Test address: `sweeper.schedule("0 * * * *", Instant.fromEpochMillis(3600000))`.

### Decisions

**[DECISION 1A] `at: Int` becomes an error now (Recommended), or is
deprecated first.** Pre-1.0, mechanical to fix, and keeping both makes
`Instant` optional where it should be the only spelling. `bynk.cron.bad_params`
fires on `Int` with a note giving the replacement and, where the body needs the
number, `at.toEpochMillis()`. An `upgrading.md` entry carries the migration.

**[DECISION 1B] The test address takes an `Instant` only (Recommended).**
Accepting `Int` as well would keep the confusion in the one place authors
write a literal time. ADR 0522 D (the time stays required when the handler
declares it; `0` otherwise) and ADR 0523 (it is not defaulted to the harness
clock) stand unchanged; "fired at `0`" becomes "fired at the epoch".

### The deltas

- **Grammar / AST.** None.
- **Checker.** `validate_cron_handler` accepts `TypeRef::Base(BaseType::Instant)`
  and refuses everything else; the address's second argument is checked
  against `Instant`. Messages say `on schedule`, not `on cron`.
- **Emitter.** None (erasure). The emitted handler parameter stays `number`.
- **Runtime.** None.

### Risks & mitigations

- Every existing cron program with `at: Int` breaks (about 20 sources across
  fixtures, examples and the book) → one-line fix, a fix-it note, an upgrading
  entry; fixtures updated in the same PR.

### Docs delta

- Reference `cron.md`, guide `entry-points/cron.md`, troubleshooting
  `cron-errors.md`, spec static semantics (the "An cron handler MUST take…"
  sentence) and `testing.md` (the address).
- Drop "Cron has no built-in clock"; point at `Clock` for "now" and keep the
  reason to prefer `at` (schedule-aligned, stable across a late start).
- State that schedules run in UTC.
- Fix the drift listed under Sequencing.
- Changelog, version history, `upgrading.md`.

### Tooling delta

- **Hover:** unchanged, because the parameter's hover already shows its type.
- **Completion:** unchanged, because nothing new is named.
- **Semantic tokens:** unchanged.
- **Signature help:** the `schedule` test address shows `at: Instant`.

### Done when

- `at: Instant` compiles; `at: Int` is `bynk.cron.bad_params` with the note;
  the explain pair `bynk.cron.bad_params` uses `Instant`.
- A behavioural fixture computes with `at` (`at - 1.hours`) at `unit` and
  `system` and gets the same answer.
- Docs current; version bump; an ADR records 1A and 1B.

---

## Increment 2 — A failed run is visible to the platform

### Summary

- **Scope:** the generated `scheduled` entry (single-context and group), the
  test harness's execution-context double, book. No language surface change.
- **Addresses:** an `Err` is logged and the run ends normally, so Cloudflare's
  Past Events records a success (`workers_entry.rs:2081`). A throw escapes
  unlabelled. The log line carries the full error payload, unlike `.detach()`'s
  (ADR 0825). An unmatched `event.cron` is dropped silently.
- **Realises:** an `Err` or a fault marks the run failed in Past Events, with a
  labelled log line naming service, schedule and error variant, using the
  mechanism the `fetch` entry, the queue consumer and `.detach()` already share.

### Framing

The author already says "this run failed" by returning `Err`. Making the
platform agree is the compiler's job; doing it by hand needs `ctx`, which the
author cannot reach.

### The surface

None. Observable behaviour: a run returning `Err`, or throwing, shows as failed
in Cron Past Events and Workers Logs.

### Decisions

**[DECISION 2A] Mark failure by passing a rejected promise to `ctx.waitUntil`
(Recommended), or by throwing from `scheduled`.** `waitUntil` is the
documented route to the Past Events status, and it is `__bynkReportFault`, the
existing mechanism (#2192). Throwing depends on undocumented behaviour,
including whether production retries the run. Leaving it as logging only is the
status quo. With 2A, "a cron run has no retry channel" stays true.

**[DECISION 2B] Log the error's type and variant, never its payload
(Recommended), or keep logging the payload.** ADR 0825 chose not to log a
detached `Err`'s payload, which may hold user data; cron should match, using the
same structured `console.error` record (`{message, level, fields: {site, type,
variant}}`). This is a visible change: a payload that used to reach the log
does not. An author who wants it logs it with `Logger` in the handler.

**[DECISION 2C] On a throw: catch, log a labelled stack, report, rethrow
(Recommended).** Reporting gives Past Events its status through the documented
route. Rethrowing keeps today's uncaught-exception outcome, so nothing that
works now changes, and keeps ADR 0522's rule that a throw fails a test case.
Swallowing the throw would change both.

**[DECISION 2D] `ctx` is always in the `scheduled` signature (Recommended).**
Cloudflare always passes it; making it conditional only saved a parameter.

**[DECISION 2E] The `default:` arm logs one warning naming the unmatched
expression (Recommended).** Cloudflare matches character for character, so
this is the line that explains a trigger edited outside `wrangler.toml`.

**[DECISION 2F] The test double records a cron `Err` report without failing
the case or counting it as a detached failure (Recommended).** The address
already yields the `Result` (ADR 0522 B) and the case asserts on it; counting
it again would report one `Err` twice. ADR 0478 stands. A *throwing* run
fails its case exactly once: the rethrow is what fails it, as today, and the
report the catch path makes is recorded by the double but not counted as a
second failure or as a detached failure.

### The deltas

- **Grammar / AST / Checker.** None.
- **Emitter.** `workers_entry.rs` (the cron arm of the entry): `ctx` always in
  the signature; each case wrapped in `try`/`catch`; on `Err`, the structured
  log record plus `__bynkReportFault(ctx, new Error("cron `<expr>` (<service>)
  returned <Type>.<Variant>"))`; on a throw, labelled log, report, rethrow;
  `default:` warns. `workers_group.rs:660` gets the same per member. The
  test-build-only `__bynkResult` store (ADR 0522 B) is unchanged.
- **Runtime.** None beyond the existing `__bynkReportFault`.

### Risks & mitigations

- In a group, Past Events records only the first failed `waitUntil` → the
  status (failed) is still right; logs name every failing member. Documented,
  not designed around.
- An alert keyed on the old `cron <expr> failed <payload>` log line stops
  matching → changelog calls out the new record shape.
- A deployable build's entry changes → golden fixtures updated; the change is
  confined to `scheduled`.

### Docs delta

- Reference `cron.md` §Failure: an `Err` marks the run failed in Past Events
  and logs type and variant; a throw is labelled and reported; still no retry.
- Guide `entry-points/cron.md` §"Signal success or failure"; `testing.md`
  (what the double records). Worker groups page: the first-failure note.
- Changelog and version history.

### Tooling delta

- **Hover / Completion / Semantic tokens / Signature help:** unchanged,
  because there is no new surface.

### Done when

- An `Err` run, and a throwing run, each show as failed in a local
  `/cdn-cgi/local/scheduled?format=json` run; an `Ok` run shows as a success.
- A behavioural fixture (`expected_run.txt`) shows the double's recorded
  report for an `Err` run and that the case still passes on `expect r is
  Err(_)`; a throwing run still fails its case.
- Golden fixtures for single-context and group entries updated.
- Docs current; version bump; an ADR records 2A–2F.

---

## Increment 3 — A `Cron` refinement predicate and `CronSchedule`

### Summary

- **Scope:** a new predicate in the closed `String` set (`bynk-syntax`
  `PredKind`), its Rust rule and TypeScript twin, a prelude type, the checker's
  schedule validation, entailment and contract hash, OpenAPI export, tooling.
- **Addresses:** the schedule is checked for field count only (deferred since
  v0.10), so `"61 * * * *"` compiles and fails at deploy. Nothing can name "a
  valid schedule" as a type, which increments 4 and 5 need.
- **Realises:** `type CronSchedule = String where Cron`, admitting exactly
  Cloudflare's dialect, checked at compile time for literals and at `.of`,
  decoding and boundaries at run time; the `schedule(…)` position typed by it.

### Framing

A `Matches` regex cannot do this: it cannot check that a range runs low to high,
that a step is at least one, or where `L`, `W` and `#` may appear, and a
pattern that tried would trip `bynk.types.catastrophic_regex`. `IsoDate`
(ADR 0727) is the precedent for a domain predicate the regex family can't
express.

### The surface

```bynk,ignore
// in the core prelude
type CronSchedule = String where Cron

// user code
type Nightly = String where Cron      // any String refinement may use it
let s: CronSchedule = "0 2 * * MON-FRI"   // admitted at compile time
let bad = CronSchedule.of(input)          // Result[CronSchedule, ValidationError]
```

`on schedule("…")` checks its argument against `Cron`.

### Decisions

**[DECISION 3A] Names: predicate `Cron`, prelude type `CronSchedule`
(Recommended).** Mirrors `IsoDate`/`Day`. The type is exported from the `bynk`
adapter's `exports transparent` list beside `Claim` and `Uuid`.

**[DECISION 3B] The dialect is Cloudflare's, exactly (Recommended).** Fields
and characters as listed above; day of week `1-7` with `1` = Sunday; names any
case; no `?`, seconds, year or macros. If a second deploy target arrives, the
predicate becomes target-qualified then; naming it for a dialect now would
guess at a future that may not come.

**[DECISION 3C] Fields are separated by exactly one space, with no leading or
trailing space (Recommended).** The string then is what Cloudflare echoes in
`event.cron`, so no normalisation is needed anywhere, and the exact-string
duplicate check means what it says. Rejected: normalising in the emitter,
which would hand the handler (increment 4) a string the author didn't write.
This is a **breaking change**: today's check splits on any whitespace, so a
double space, a tab, or a leading or trailing space compiles, deploys and fires
correctly. Such a schedule becomes `bynk.cron.invalid_schedule`, with a fix-it
note giving the single-spaced form. The alternative that breaks nothing is to
keep accepting any whitespace and normalise in the emitter, at the cost above.

**[DECISION 3D] In `schedule(…)`, a violation keeps the code
`bynk.cron.invalid_schedule` (Recommended),** now naming the failing field
and why (`minute 61 is outside 0-59`); elsewhere it is
`bynk.refine.literal_violates`, as for any refinement. The existing explain
entry is updated, not replaced.

**[DECISION 3E] Two warnings ship with the predicate (Recommended):**
`bynk.cron.numeric_weekday` (a numeric day of week; the common mistake is
standard cron's `0`–`6`, where `5` is Friday but here is Thursday; suggests the
name) and `bynk.cron.uneven_step` (a `*/n` in minute or hour where `n` doesn't
divide the field's range; says when it actually fires). Warnings only: both
forms are valid. Each can be silenced by writing the name or an explicit list.

**[DECISION 3F] OpenAPI maps `Cron` to `type: string` with
`x-bynk-format: cron` (Recommended).** There is no standard `format` for cron.

### The deltas

- **Grammar / AST.** `PredKind::Cron` (`bynk-syntax/src/ast.rs`, beside
  `IsoDate` at :2955) and `is_cron_expression(&str) -> Result<(), CronFault>`
  beside `is_iso_date` (:2962), returning the failing field for 3D.
- **Checker.** Predicate on `String` only (`predicate_base_mismatch`
  otherwise); `validate_cron_handler` calls `is_cron_expression` in place of
  the field count; entailment: only `Cron` entails `Cron`, and it entails
  `MinLength(1)`–`MinLength(9)` (`"* * * * *"` is the shortest); conflicting
  with `MaxLength(n < 9)` is `empty_refinement`; contract-hash canonical name
  `Cron`. The two warnings.
- **Emitter / Runtime.** `__bynkIsCron` in the runtime (next to
  `__bynkIsIsoDate`, `bynk-emit/runtime/src/time.ts`, or a new `cron.ts`), used
  by `.of`, `Json.decode`, boundary codecs and `is`.
- **Std.** `CronSchedule` in `bynk-std/lib/core/bynk.bynk`.

### Risks & mitigations

- Cloudflare's docs are thin on edge cases (`L` alone in day of week, `#`
  ranges, `W` with ranges, steps on names) → pin each edge case against
  `wrangler deploy --dry-run` or the local scheduler before fixing the rule;
  where Cloudflare's behaviour can't be confirmed, refuse (a refusal can be
  relaxed later, an admission can't be withdrawn).
- The Rust rule and the TS twin disagree → a property test over generated
  strings, as ADR 0727 does for `IsoDate`.
- A schedule that compiled before now fails. For an invalid one (`61 * * * *`)
  this costs nothing, since it already failed at deploy. **A valid one with
  irregular whitespace also fails (3C), and that one works today** → the error
  carries a fix-it note with the single-spaced form, and `upgrading.md` and the
  changelog list it as a breaking change. The same applies to anything else
  the old field count admitted that Cloudflare also accepts; spacing is the
  confirmed case, and the edge-case pinning above should look for others.

### Docs delta

- Reference `refined-types.md` (the predicate), `cron.md` (the dialect in
  full: fields, characters, Sunday = 1, UTC, `*/45`), `types.md` or the prelude
  list (`CronSchedule`); troubleshooting `cron-errors.md` (the sharper
  `invalid_schedule`, the two warnings); spec static semantics and runtime
  library. Changelog and version history; `upgrading.md` (the whitespace
  break, 3C).

### Tooling delta

- **Hover:** on `Cron` and `CronSchedule`, the rule; on a `schedule("…")`
  literal, the expression's fields named. (A plain-English rendering and next
  fire times are deferred; see the last section.)
- **Completion:** offers `Cron` after `where`, and `CronSchedule` as a type.
- **Semantic tokens:** unchanged, because `Cron` is a predicate name like
  `IsoDate`, already tokenised.
- **Signature help:** unchanged, because no callable changes shape.
- The **Try values…** panel works for `Cron` with no extra work, because it
  drives the predicate.

### Done when

- Every example on Cloudflare's page is admitted; `61 * * * *`, `* * * * 0`,
  `5-1 * * * *`, `*/0 * * * *`, `? * * * *`, `@daily`, a six-field string and a
  double space are refused, each with its field named.
- A schedule with irregular whitespace (`"0  2 * * *"`, a tab, a trailing
  space), which compiles today, is refused with the single-spaced form in
  the fix-it note; a fixture pins it.
- `.of`, `Json.decode`, an HTTP boundary and `is` agree with the compile-time
  rule (property test).
- Warnings fire on `* * * * 5` and `*/45 * * * *`, and not on `* * * * FRI`.
- Explain entry updated for `invalid_schedule`; fixtures at the next free
  indices; docs current; version bump; an ADR records 3A–3F.

---

## Increment 4 — The handler can take its schedule

### Summary

- **Scope:** checker (`validate_cron_handler`, the test address), emitter
  (pass `event.cron`), book.
- **Addresses:** a handler can't see which schedule fired it, so a run key
  for `Idempotency.claim` must repeat the schedule literal by hand.
- **Realises:** a handler may declare `schedule: CronSchedule` alongside or
  instead of `at: Instant`, and `"\(schedule)@\(at.toEpochMillis())"` is a
  ready-made run key. (`Instant` is not interpolable, `checker.rs:4968`, so
  the `toEpochMillis()` is required.)

### The surface

```bynk,ignore
service sweeper from cron {
  on schedule("0 2 * * *") (schedule: CronSchedule, at: Instant)
      -> Effect[Result[(), SweepError]] given Idempotency {
    let claim <- Idempotency.claim("sweeper:\(schedule)@\(at.toEpochMillis())", 1.hours)
    match claim {
      Claimed => sweep(at),
      AlreadyClaimed => Ok(()),
    }
  }
}
```

### Decisions

**[DECISION 4A] Up to two parameters, told apart by type, in either order
(Recommended).** One may be `Instant`, one may be `CronSchedule`; a second of
the same type or any other type is `bynk.cron.bad_params`. Rejected: a single
`tick: Tick { at, schedule }` record. Cloudflare's event carries nothing else
for it to grow into, and it makes the common one-parameter case wordier.
Rejected: fixed positional order, which adds a rule to remember for no gain.

**[DECISION 4B] The parameter's type is exactly `CronSchedule`
(Recommended),** not `String` or another `Cron` refinement. One spelling;
a `CronSchedule` widens to `String` wherever it is read.

**[DECISION 4C] The value is `event.cron` (Recommended).** By 3C it equals
the expression as written. At `unit` and `integration` the test address
passes its own first argument; at `system` the driver already sends it as
`cron`.

### Framing note

While a handler has exactly one literal schedule, this parameter always holds
the same value. It is still worth having: it removes a repeated literal from
run keys, and it becomes the only way to tell runs apart once a handler can
carry several schedules or a schedule comes from a constant (increment 5).

### The deltas

- **Grammar / AST.** None.
- **Checker.** 4A and 4B in `validate_cron_handler`; the address is
  unchanged in shape.
- **Emitter.** The handler call passes `event.cron` and/or
  `event.scheduledTime` in the declared order, in single-context and group
  entries and in `compose`.
- **Runtime.** None.

### Risks & mitigations

- Authors read `schedule` as "the next run" → the reference says plainly it is
  the expression that fired this run.
- In a worker group, two members may share an expression and both run
  (`group_triggers.rs:15`). They share one Worker and so one idempotency
  store (the `<worker>-idempotency` database), and both see the same
  `schedule` and `at`, so the recipe's run key collides and one member skips
  its run → the recipe puts the service name in the key, and the guide says
  why.

### Docs delta

- Reference `cron.md` (parameters), guide `entry-points/cron.md` (a "Run once
  per scheduled time" recipe using `Idempotency.claim`), troubleshooting
  (`bad_params` wording). Changelog and version history.

### Tooling delta

- **Hover:** the parameter shows `CronSchedule`; unchanged mechanism.
- **Completion:** unchanged, because parameter names are the author's.
- **Semantic tokens:** unchanged.
- **Signature help:** unchanged, because the address's shape is unchanged.

### Done when

- Each of `()`, `(at: Instant)`, `(schedule: CronSchedule)`, and both orders
  of the pair compiles and receives the right values at `unit` and `system`
  (behavioural fixture); a duplicate or wrong type is `bad_params`.
- The idempotency recipe runs twice at the same `at` and sweeps once.
- Docs current; version bump; an ADR records 4A–4C.

---

## Increment 5 — A constant names a schedule

### Summary

- **Scope:** parser (`schedule(` accepts a name), checker (resolution and the
  type rule), IDE (the reference is a use), book.
- **Addresses:** a schedule shared by two places (a handler and a test, or
  two contexts through a commons) must be written twice.
- **Realises:** `const Nightly: CronSchedule = "0 2 * * *"` and
  `on schedule(Nightly)`, resolved to its literal at compile time so the
  trigger table and the dispatch key are still literal text.

### What exists today

A constant **can already be refined**: `const DefaultPort: Port = 8080` is
checked by literal admission at its declaration (`constants.md`). Every
initialiser is in the static set (ADR 0678 A), so any constant resolves to
literal source text through `consts::chain_literal`
(`bynk-check/src/consts.rs:560`). But ADR 0826 A lets a constant stand for a
literal in an annotation argument only when it is declared **exactly the base
type**; a refined or opaque constant does not count. Reused as it stands, that
rule would refuse `const Nightly: CronSchedule`, the one we most want.

The exclusion is a policy choice, not a soundness need: a refined constant
resolves to its literal like any other, and that literal can be rechecked.

### The surface

```bynk,ignore
commons ops.schedules

const Nightly: CronSchedule = "0 2 * * *"
```

```bynk,ignore
context ops.reaper
uses ops.schedules

service sweeper from cron {
  on schedule(Nightly) (at: Instant) -> Effect[Result[(), E]] { … }
}
```

```bynk,ignore
let r <- sweeper.schedule(Nightly, Instant.fromEpochMillis(0))
```

### Decisions

**[DECISION 5A] What `schedule(…)` accepts (Recommended): a string literal, a
`String` constant, or a constant declared exactly `CronSchedule`.** In every
case the chain resolves to a literal, which is then checked against `Cron`.
Refused: a constant of another refined type (`String where MaxLength(20)`, or
a user's own `type Nightly = String where Cron`), keeping named types distinct
inside a context (ADR 0714 B). This extends ADR 0826 A from "exactly the base
type" to "exactly the type the position expects, which may be refined", for
this position only.

**[DECISION 5B] Don't generalise ADR 0826 in this increment (Recommended).**
No annotation argument currently expects a refined type, so there is nothing
to apply the wider rule to. The ADR for this increment records the principle
so the next refined-typed position adopts it rather than re-deciding.

**[DECISION 5C] The test address accepts the same forms (Recommended),**
matched to a handler by the resolved literal. A test that names the constant
follows a schedule change instead of breaking on it.

**[DECISION 5D] A refused constant is `bynk.cron.invalid_schedule` with a note
naming the constant and its declared type (Recommended),** following ADR 0826
D (no new diagnostic; the position's existing code, with a note). No new
explain entry is needed.

### The deltas

- **Grammar / AST.** `schedule(` takes a string literal or an identifier; the
  AST holds either, keeping the reference's span. The handler kind's `expr` is
  filled once resolved.
- **Checker.** Resolution runs before `validate_cron_handler`, the duplicate
  check, `cron_and_queue_triggers` (`symbols.rs:2339`) and group trigger
  collection, reusing `consts::chain_literal` (ADR 0826 C). 5A's type rule.
- **Emitter.** None: it sees the resolved literal.
- **Runtime.** None.

### Risks & mitigations

- A reader that takes the schedule before resolution sees a name, not an
  expression → resolve once, upstream, as ADR 0826 B does for annotations;
  every downstream reader (wrangler, dispatch, architecture facts, hover)
  reads the resolved form.
- Expectation that a constant varies by environment → the reference says
  constants are fixed at compile time; per-environment schedules are a
  separate question (below).

### Docs delta

- Reference `cron.md` and `constants.md` (the new position, and the refined
  rule beside ADR 0826's list); guide `entry-points/cron.md` (share a schedule
  through a commons); spec syntactic grammar and static semantics. Changelog
  and version history.

### Tooling delta

- **Hover:** on the name in `schedule(Nightly)`, the constant's type and
  resolved expression.
- **Completion:** inside `schedule(`, offers in-scope `String` and
  `CronSchedule` constants.
- **Semantic tokens:** the name is tokenised as a constant reference.
- **Signature help:** unchanged.
- Go to definition, find references and rename treat it as a use (ADR 0826 E).

### Done when

- Literal, `String` constant, `CronSchedule` constant (same file, sibling
  file, used commons) all compile and emit identical `wrangler.toml` and
  dispatch; another refined type is refused with the note; a private constant
  from another unit is `bynk.visibility.private_item`.
- Duplicate detection sees through constants (`schedule(Nightly)` and
  `schedule("0 2 * * *")` in one context are `duplicate_schedule`).
- Docs current; version bump; an ADR records 5A–5D.

---

## Considered and deferred

- **Several schedules on one handler** (`on schedule("0 9 * * 1-5", "0 12 * * 1,7")`):
  natural once increment 4 lets the body tell them apart. Not yet asked for.
- **Per-environment schedules** (a different expression for staging): needs
  `bynk.toml` and `[env.<name>.triggers]`; a deploy-configuration question,
  not a cron one.
- **A run-once switch** that wraps the handler in `Idempotency.claim` on
  `(schedule, at)`: increment 4 makes the hand-written version one `match`;
  revisit if the recipe proves common.
- **Overlap protection** (skip a run while the previous one is still going):
  needs a lock (a Durable Object or a claim with a lease, ADR 0829); worth its
  own proposal.
- **Hover in plain English with the next fire times (UTC), and a test lint for
  an `at` the schedule can never produce, `Schedule.next(expr, after)`:**
  tooling and kernel additions that build on increment 3's parser.
- **Named handlers** (`on schedule("…") as hourly`): would let tests skip the
  expression, but a test breaking when the schedule changes is arguably
  useful, and increment 5 already lets both name a constant.
- **Rejected: a typed builder** (`every 15.minutes`). Cron syntax is the
  standard, and many durations have no cron expression.
