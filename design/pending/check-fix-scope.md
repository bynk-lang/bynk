---
level: patch
changelog: `bynk check --fix` applies only the fixes of diagnostics `check` shows, names the code each file's edits fixed, and no longer binds a bare statement with `let _ =`
---

## ADR: fix-applies-only-shown-diagnostics
title: Batch fixes (`check --fix`, fix-all) take only the fixes of diagnostics `check` shows
summary: Hints are skipped by the shared selection rule; the bare-statement `let _ =` fix is a quick fix only

**Context.** ADR 0717 said `bynk check --fix` still applies a hint's fix, so `--fix` silently flattened nested `match`es that `check` never reported (#3537). The bare-statement fix (#3161) also guessed `let _ =` for any expression not syntactically an effect, which on an `Effect` silently stops it running.

**Decision.** `bynk_syntax::fix::select`, the rule `check --fix` and `source.fixAll.bynk` share, skips Hint-severity diagnostics; a hint's fix stays an individual editor quick fix. `bynk.parse.bare_expression_statement` offers `let _ =` as `MaybeIncorrect`, and `let _ <-` stays `MachineApplicable`. `--fix` reports `fixed: <file>: <code> ×n`, n counting diagnostics fixed. This supersedes the `--fix` sentence of ADR 0717.

**Consequences.** `--fix` is an upgrade tool for what `check` shows. A new surface such as `--fix=<code>` is a separate proposal (#3537).
