---
level: minor
changelog: A `Duration` literal uses the singular unit with `1` and the plural otherwise (`1.hour`, `5.hours`), and an unknown unit is `bynk.duration.unknown_unit`
breaking: `1.hours` (and every `1.<plural unit>`) no longer parses as a `Duration` literal: it is `bynk.duration.unit_agreement`, with a machine-applicable fix-it (`bynk check --fix`) writing `1.hour`. `5.hour` is the same error, and a word after an integer literal that names no unit (`5.hr`) is `bynk.duration.unknown_unit`, where it used to be a field access on an `Int`.
probes: diagnostic_coverage, explained_codes
---

## ADR: duration-literal-unit-agreement
title: Duration literals use the unit that agrees with the number, and an unknown unit is its own error
summary: Singular exactly with 1, plural with every other number; an integer literal followed by a word is always a Duration attempt

**Context.** `DurationUnit::from_name` recognised only plural units, so `1.hour` fell through to a field access on an `Int` (an error that said nothing about durations) while `1.hours` was accepted and read wrongly.

**Decision.** Strict agreement: the singular unit exactly with `1`, the plural with every other number, `0` included. An integer literal followed by `.<identifier>` (that is not a call) is always a `Duration` attempt, since an integer literal has no fields: a wrong form is `bynk.duration.unit_agreement` and a word that names no unit is `bynk.duration.unknown_unit`, both with a fix-it over the unit word (the nearest unit for the latter). Both parser sites, an expression and a scheme config literal (`ttl = 12.hours`), share one check. A negative duration is the negation of a literal, so the rule reads the literal's own number. The formatter writes the form that agrees with the number. No `weeks` unit in this increment.

**Consequences.** Existing `1.<plural>` sources stop parsing until rewritten; the repository was migrated in one pass and the fix-it does the same for users. Completion after `1.` offers the singular units and after any other integer the plural ones. `Duration.toString()` renders plain milliseconds, so it needed no change.
