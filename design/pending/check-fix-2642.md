---
level: minor
changelog: "`bynk check --fix` and `bynkc check --fix` apply the compiler's machine-applicable fix suggestions from the command line, repeated to a fixpoint and written only when the whole run is sound; the selection rule moves into `bynk-syntax` and is shared with the editor's fix all"
---

## ADR: cli-check-fix
title: `check --fix` applies machine-applicable suggestions from the CLI
summary: One shared selection rule, a fixpoint with a soundness gate, and an all-or-nothing write

**Context.** ADR 0054 put structured fix suggestions on `CompileError` so that
"the same suggestions back a future CLI `--fix` with no LSP involvement". Only
the editor could apply them, through a quick fix or `source.fixAll.bynk`. A
breaking increment that ships a machine-applicable migration (constrained
generics slice 0, #2639) needs a path for users with no editor. This increment
is that CLI (#2642). No grammar, checker, emitter or runtime change.

**Decision.**

- **Where (A).** `--fix` is a flag on `check` in both `bynk` and `bynkc`,
  implemented once in `bynk_driver::run_check`. The `BYNK_BYNKC` delegation
  forwards it; a pinned `bynkc` too old to know it fails with clap's
  unknown-argument error, and the skew gate already names the gap.
- **One rule (B).** The pure half of `bynk-lsp/src/fix_all.rs` (`edits`,
  `overlaps`, `valid`, `apply`) moves to `bynk_syntax::fix`, beside `Suggestion`,
  and takes `&CompileError`s rather than `bynk_ide::Diagnostic`s, with its unit
  tests. `bynk-lsp` keeps `kind()` and `action()` and calls the moved code. A
  test asserts the CLI's first pass takes exactly the editor's edits.
- **Fixpoint (C).** Re-check and re-apply until a pass takes no edits, at most
  ten passes. If the tenth still takes edits two fixes are undoing each other:
  nothing is written and the codes involved are reported.
- **Gate (D).** A pass is accepted only if every diagnostic it introduces
  (matched by file, category and message, since spans move) carries a single
  machine-applicable fix. Otherwise the run writes nothing. The editor keeps its
  stricter one-shot gate (no new diagnostic), because a fixpoint exists to follow
  a cascade, and a cascade is a new, fixable diagnostic. All passes run in
  memory over the source texts; files are written at the end.
- **Writes (E).** Only files whose text changed, in place (via the same atomic
  rename `fmt` uses), with no formatter pass: suggestions are already formatted
  text and `fmt` would add unrelated diffs to a migration commit. A read-only
  file fails the run before the first write. A write error fails the run with
  the path, and files already written stay written, as for `fmt`.
- **No dry run (F).** `git diff` is the review and `git checkout` the undo; a
  `--fix --diff` can follow if asked for.
- **Input (G).** Whatever `check` accepts: a file, a project, or a workspace
  root (#2207). At a workspace root every member is planned before any is
  written, so a member that is refused leaves the whole workspace untouched.
  Members are planned independently, against their on-disk text, so a fix in
  one member that changes something another depends on is not seen by the gate;
  `check` reports the result and `git checkout` undoes it. The final state of
  each member is also held against its input: it may hold no diagnostic the
  input did not.

Output is one `fixed: <path> (<n> edits)` line per file written, then the
ordinary report for what remains. The exit code is that of the check that
follows, and non-zero for a refused or failed fix.

**Consequences.** This fulfils the CLI ADR 0054 anticipated. Every later
breaking increment that ships a machine-applicable fix has its editor-less
migration path with no extra work. A wrong machine-applicable suggestion is now
applied to disk unreviewed, as it already was by one-click fix-all; ADR 0054's
test strategy (every seed re-diagnoses clean) and the gate bound the risk. The
gate is conservative: a fix that changes the message of a pre-existing
fixless diagnostic counts as introducing one and refuses the run. The cascade
test uses a real chain: adding a capability to an agent handler's `given`
(`bynk.given.undeclared_capability`) makes the provider that calls it short of
the same capability (`bynk.provider.agent_call_undeclared_capability`), a second
fixable diagnostic found only on the second pass.
