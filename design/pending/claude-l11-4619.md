---
level: patch
changelog: A `system` case that builds or reads a `bynk` prelude type its target consumes (`consumes bynk { CloseInfo }`) no longer fails with an internal `missing_checker_fact` on a method such as `.any` over a list of them, when no service signature names that type
---

## ADR: system-view-holds-the-first-party-types-a-participant-consumes
title: A system harness's view holds the first-party types its participants consume by name
summary: `reach_first_party_types` seeds from each participant's consumed first-party types, not only from the names its declarations and handler signatures reach

**Context.** #4619: with `consumes bynk { CloseInfo }` and no service signature naming `CloseInfo`, a `system` case `let cs = [CloseInfo { … }]` followed by `cs.any((c) => c.code == 1006)` failed with `bynk.internal.missing_checker_fact` ("no receiver namespace for an instance call"). The harness view held a first-party type only if a view declaration or a participant's handler signature reached it (#3400), so the case's record was left unchecked (`check_record_construction` skips a type the view lacks), the list's element type was unknown, and the method call recorded no callee.

**Decision.** `SuiteView::reach_first_party_types` also seeds its walk with each participant's in-scope consumed first-party types: those of the units the participant's table records as consumed, less the ones a braced `consumes` leaves out (`unselected_consumed_types`). The units it reads from are those same units besides `bynk` and `bynk.cloudflare`.

**Consequences.** A case may name and build the prelude types its target consumes, and they are checked. A participant that consumes a first-party unit whole (`consumes bynk`) brings all that unit's types into the view, where it brought only the reached ones before; a type the view already binds is unchanged.
