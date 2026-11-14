---
level: minor
changelog: Naming a standard-library type (or a bare variant of one of its sums) that a braced `consumes` does not list is the warning `bynk.consumes.type_not_selected`, and `bynk check --fix` adds the name to the braces; an error follows in a later release (#3536)
probes: emit_diagnostics, diagnostic_coverage, explained_codes
---

## ADR: braced-consumes-lists-what-is-in-scope
title: A braced `consumes` of a first-party unit brings into scope only the names it lists
summary: Unlisted standard-library types are not clash or ambiguity candidates; naming one is a deprecation warning with a fix, then an error

**Context.** `consumes bynk { Logger }` brought every type `bynk` exports into scope, so adding a standard-library export (`Claim`, `ContainerError`) broke correct programs that never asked for it: a suite `type Claim`, or a bare `Unavailable(…)` of the program's own sum, clashed or became ambiguous (#3536).

**Decision.** A braced `consumes` of a first-party unit (`bynk`, `bynk.*`) brings into scope only what it lists, plus whatever the listed capabilities' signatures need to flow as values; a bare `consumes bynk` still brings in the whole module. Naming a type, or a bare variant of a sum, that the braces leave out requires listing it. Stage 1 (this increment): the unlisted types are not candidates for suite name clashes, bare-variant resolution, const/variant clashes or `bynk.consumes.name_conflict`, and a declaration of the unit's (or a suite's) own wins; a use of an unlisted name still resolves, as the warning `bynk.consumes.type_not_selected` with a machine-applicable fix that adds the name to the braces. Stage 2 (a following minor) makes it an error. User-authored contexts keep the whole-export scope, and so do suites naming an unlisted type for now.

**Consequences.** A standard-library release can add an export without breaking a program that did not list it. Existing programs that name unlisted types gain a warning and `bynk check --fix=bynk.consumes.type_not_selected` migrates them. The repository's fixtures, examples and Book pages are migrated.
