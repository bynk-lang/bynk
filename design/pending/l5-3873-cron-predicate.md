---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: A `Cron` refinement predicate and the `CronSchedule` type admit exactly Cloudflare's cron syntax; a schedule's fields are now checked, with `bynk.cron.numeric_weekday` and `bynk.cron.uneven_step` warnings
breaking: A cron schedule with irregular whitespace (a double space, a tab, a leading or trailing space) now fails as `bynk.cron.invalid_schedule`, as does any schedule Cloudflare would reject (`61 * * * *`); write the fields separated by single spaces (the note gives the corrected form).
---

## ADR: cron-refinement-predicate
title: A `Cron` refinement predicate and the `CronSchedule` type
summary: `Cron` admits exactly Cloudflare's cron syntax, single-spaced; `CronSchedule` names it

**Context.** The cron schedule was only checked for having five fields, so
`"61 * * * *"` compiled and failed at deploy, and nothing could name "a valid
schedule" as a type. A `Matches` regex cannot express the rule (a range runs low
to high, a step is at least one, `L`/`W`/`#` appear only in certain fields) and
would trip `bynk.types.catastrophic_regex`. `IsoDate` (ADR 0727) is the
precedent for a domain predicate the regex family can't express.

**Decision.** A. The predicate is `Cron` and the prelude type is
`CronSchedule = String where Cron`, exported transparently from the `bynk`
adapter beside `Uuid`. B. The syntax is exactly Cloudflare's: day of week `1-7`
with `1` = Sunday, names in any case, no `?`, seconds, year or macros. Where
Cloudflare's documentation does not settle a form (a bare `L` in the day of
week, a step on a single value, `#` or `W` combined with ranges) it is refused:
a refusal can be relaxed later, an admission can't be withdrawn. C. Fields are
separated by exactly one space, with no leading or trailing space, so the string
is exactly what Cloudflare sends back in `event.cron` and the exact-string
duplicate check means what it says; this is a breaking change, accepted. D. In
`schedule(…)` a violation keeps the code `bynk.cron.invalid_schedule`, now
naming the failing field and why; elsewhere it is `bynk.refine.literal_violates`.
E. Two warnings ship with the predicate: `bynk.cron.numeric_weekday` and
`bynk.cron.uneven_step`; both forms are valid, so they only warn, and a name or
an explicit list silences each. F. OpenAPI maps `Cron` to `type: string` with
`x-bynk-format: cron`. The rule lives once in `bynk_syntax::cron`, with its
TypeScript twin `__bynkIsCron`; a generated corpus (`bynk-syntax/tests/cron_corpus.tsv`)
keeps the two in agreement. `Cron` entails `MinLength(1)` to `MinLength(9)`
(`* * * * *` is the shortest schedule) and only `Cron` entails `Cron`.

**Consequences.** Schedules that Cloudflare rejects now fail at compile time. A
valid schedule with irregular whitespace also fails, and is listed as breaking
with the corrected form in the fix-it note. The old fixture that pushed a quote
into a schedule to test TOML escaping is now a negative fixture, because the
checker refuses such a schedule first; the emitter's escaping stays as defence in
depth.
