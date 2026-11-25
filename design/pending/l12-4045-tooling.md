---
level: minor
changelog: A name a braced `uses` list selects outranks the same name from a whole `uses`, so `uses lib.y { Status }` settles a `bynk.uses.ambiguous_name`. Completion inside `uses lib.y { }` offers the commons' public names, and the diagnostic offers an alias (with one that avoids names already taken) or a selection as fixes
---

## ADR: uses-selection-outranks-a-whole-import
title: A name a braced `uses` list selects outranks the same name from a whole `uses`
summary: How `bynk.uses.ambiguous_name` is settled by selecting, and the fixes the tooling offers for it

**Context.** #4045 Decision B says a clash is reported where the name is written, and suggests "an alias or braced list" to settle it. As first built, a selected name entered the same provider set as a whole import, so `uses lib.x` beside `uses lib.y { Status }` was still ambiguous: the advice in the diagnostic, its explanation and the code action did not work.

**Decision.** A name that a braced list selects (under its own name or a rename) competes only with other selections of that name. A whole `uses` that also declares it loses, so `Status` is the selected commons' while the whole commons' own declarations still read their own `Status`. Two selections of one name are still `bynk.uses.ambiguous_name`. The code action on that error offers an alias for each commons, only where exactly two declare the name (aliasing one of three leaves two), with an alias that avoids an alias the unit already writes, a name it declares or brings in, or the alias suggested for another provider, and a selection for each, which settles it for any number. A provider whose clause is already a braced list has no fix. Completion inside `uses U { … }` offers every file's public names of a commons split over several, and not a name already listed on the line.

**Consequences.** A program that was ambiguous only because one of two clauses named the name in a braced list now compiles; no program that compiled changes. A selection narrows its clause to the names listed, so the fix's title says so.
