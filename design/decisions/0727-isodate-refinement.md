# 0727 — IsoDate, a calendar-date refinement predicate for String

- **Status:** Accepted (v0.503)

**Context.** A date-only input (a path parameter, a body field) could not be
fully validated at the boundary. `Matches` can bound months and days but cannot
reject `2026-02-30` or `2026-02-29`, so an impossible date passed and every
handler re-checked the calendar by hand. The kernel already knows the rule:
`Instant.fromIso` requires the date to exist.

**Decision.** `IsoDate` is a new predicate in the closed `String` set (#2867),
taking every recommended option from the proposal:

- *A: a refinement, not a base type.* A `Day` stays a `String` on the wire, in
  SQL and in interpolation, and composes with the existing machinery: a literal
  is admitted or refused at compile time (`"2026-11-03"` admitted,
  `"2026-02-30"` refused with `bynk.refine.literal_violates`).
- *B: exactly `YYYY-MM-DD`, years 0000-9999,* the date half of RFC 3339 and the
  date rule `Instant.fromIso` applies. No week, ordinal or basic forms.
  Leap years follow the Gregorian rule (`2028-02-29` and `2000-02-29` exist;
  `2026-02-29` and `1900-02-29` do not).
- *C: no `Instant` helper.* The guide documents
  `Instant.fromIso("\(day)T00:00:00Z")`, always `Some` for a `Day`.

One rule per side. `bynk_syntax::ast::is_iso_date` decides compile-time literal
admission; the runtime's `__bynkIsIsoDate` (`time.ts`) is the emitted check, used
by `.of`, the `Json.decode` and boundary codecs, inline consumer-side
revalidation and `is` tests, and it shares `isRealDate` with `__bynkInstantFromIso`
so the two cannot disagree. A property test pins `IsoDate(d)` against
`Instant.fromIso(d + "T00:00:00Z")` over generated strings.

The predicate canonicalises to `IsoDate` in the contract hash and entails
`Length(10)` (so also `MinLength(1)` through `MinLength(10)` and `MaxLength(10)`
and above); only `IsoDate` entails `IsoDate`. Combined with a conflicting length
it is an `empty_refinement`. The OpenAPI export maps it to `format: date` with
`minLength`/`maxLength` of 10. Hover shows the rule and completion offers the
name after `where`.

**Consequences.** `type Day = String where IsoDate` makes `/digest/2026-02-30`
a `400 RefinementViolation` before the handler runs. A full `Date` kernel type
(calendar fields, `plusDays`) can follow and is not blocked. Adding a predicate
to the closed vocabulary changes the grammar's predicate-name set (tree-sitter,
TextMate, the grammar appendix) and the wire-format reference's predicate list;
no existing contract hash changes.
