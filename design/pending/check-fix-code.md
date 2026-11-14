---
level: minor
changelog: `bynk check --fix=<code>[,<code>…]` applies only the fixes of the named diagnostic codes, hints included, and refuses an unknown code with the nearest real one
---

## ADR: check-fix-by-code
title: `check --fix=<code>` narrows a fix run to the named diagnostic codes
summary: A repeatable, comma-separated filter on `--fix`; naming a code asks for its fixes, a hint's included

**Context.** `--fix` applies every fix of every diagnostic `check` shows (#3537), which is more than an upgrade note wants: a release that deprecates one construct should be able to say "run `bynk check --fix=<code>`" and change nothing else.

**Decision.** `--fix` takes an optional value: `--fix=<code>[,<code>…]`, repeatable. Bare `--fix` is unchanged. A named run takes only the lone machine-applicable fixes of those codes, by the shared `bynk_syntax::fix` rule; naming a code is asking for it, so a hint's fix is taken when its code is named, and an editor-only quick fix (`MaybeIncorrect`) never is. The fixpoint and the gate are unchanged and still see every shown diagnostic, so a narrowed run is refused on the same grounds as a full one. Each code must be in the diagnostic registry: an unknown one fails the run before any work, naming the nearest real code within two edits. A valid code with nothing to fix writes nothing. `--fix-style` is not added.

**Consequences.** Upgrade notes can name the one code to run. The CLI gains a value on an existing flag, not a new flag; `bynk-driver`'s `plan_file_in` and `plan_project_in` take a `Scope`, and `run_check` takes `Option<&[String]>` in place of `bool`.
