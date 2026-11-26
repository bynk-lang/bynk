---
level: patch
changelog: `bynk.email.bad_tag` now also refuses a bad literal tag written in a helper function, whether the function builds the `EmailActions` or only the tag map a `tag:` calls
breaking: a literal `Route` tag that breaks the header rules (not `X-`, characters other than letters, digits and `-`, a line break in a value) written in a helper function is now the error `bynk.email.bad_tag`, as it already was in a handler body; before, the runtime dropped the entry. Rename the tag or fix the value.
---

## ADR: bad-tag-follows-helper-functions
title: A literal Route tag is checked where it is written, in a helper function as in a handler body
summary: What counts as a literal the check reaches, and what is left to the runtime

**Context.** #4486, from #3887: `bynk.email.bad_tag` refused a literal tag map only inside a `from email` handler body. A literal built in a helper `fn` and returned to the handler was not diagnosed; at run time its bad entries are dropped and logged by name, so nothing leaked, but the author got no compile-time error. The explain text and the reference said so plainly.

**Decision.** (A) The check runs over every helper function of a context as well as every handler of an email service, from the same place and into one list, so a literal is checked where it is written and a helper reached from two literals is reported once (by span). (B) Two shapes are covered, because the issue names both: a helper that builds the whole `EmailActions { tag: … }` literal and is returned to the handler, and a helper that builds only the map, which a literal's `tag:` calls by bare name (`tag: tags()`). For the second the callee's tail expression is read as a literal `Map.empty().insert("name", "value")` chain and checked at its own spans. (C) It is syntactic, like the handler-body check it extends: no call graph, no value tracking. A tag map that is not a literal chain (a name taken from a parameter, a map built in a loop or read from storage, a call to a function the literal cannot resolve by bare name) is built at run time, where the runtime still drops a bad entry and logs its name. That is the remaining limit, and the explain text and the reference say it now instead of "handler body only". (D) No new code: the diagnostic, its explain entry and its bad/good pair are unchanged; the blurb and the reference lose the handler-body-only limit. Two negative fixtures cover the two shapes, and unit tests cover the clean cases and the once-only report.

**Consequences.** A project with a bad literal tag in a helper function stops compiling until the tag is fixed, which is the point: it was silently dropped before, so the project's forwards never carried that header. Declared as `breaking:`. A user type named `EmailActions` with a `tag:` field built by `insert` would be read the same way a handler-body literal already is.
