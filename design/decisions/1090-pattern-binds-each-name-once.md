# 1090 — A pattern binds each name once

- **Status:** Accepted (v0.814)

**Context.** A pattern could bind one name twice, `Both(a, a)` or, since `as` (#3971), `Both(a, a as a)`, and nothing said so: which binding the body saw was whatever the lowering did (#4550). Rust (E0416) and the ML family refuse it. A survey with the rule on found no such pattern in the test corpus, the site's code blocks, `bynk-std` or the examples and their suites, so the change breaks nothing in the repository.

**Decision A — one rule over every binding form.** A bare name, a record field's `f` shorthand or sub-pattern, a list element and `..rest`, and an `as` name all count, at any depth. The second binding is reported at its own span with a label on the first. In `a as a` the inner pattern's name comes first in the source, so the `as` name is the repeat.

**Decision B — or-pattern alternatives are counted separately.** An or-pattern's alternatives must bind the same names (#474, `bynk.types.or_pattern_binding_mismatch`), so a name repeated across them is required, not a duplicate, and that rule is untouched. Each alternative is walked on its own against the names bound outside the or-pattern, so `Dot(x) | Line(x, x)` and an outer name repeated inside an alternative are caught, and the or-pattern as a whole binds what its first alternative binds. `Pattern::bound_names` already reports only the first alternative's names, which is why the rule has its own walk.

**Decision C — a field listed twice is the existing rule's alone.** `Pt { x, x, .. }` is `bynk.types.duplicate_pattern_field`; its second listing is not walked, so one slip gets one diagnostic.

**Decision D — in the type checker, once per whole pattern.** `check_pattern` (every `match` arm, and so `let … else`, a destructuring `let` and a lambda's pattern parameter, which all desugar to a `match`) and `check_is` report the duplicates, and the recursive walk they used to be is now `check_sub_pattern`, so a nested pattern is not reported twice. The checker, unlike the resolver's reference pass, also checks suite case bodies, where a duplicate is as much a mistake. Rejected: the resolver pass (it does not reach suite bodies) and a quick-fix (which binding to rename is the author's choice).

**Consequences.** One new code, `bynk.types.duplicate_pattern_binding`, with an explain entry. Programs that bound a name twice and relied on whichever binding won stop compiling; none were found in the repository.
