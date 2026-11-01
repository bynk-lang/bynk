---
title: "Apply a breaking-change migration without an editor"
---
**Goal:** bring a project up to a release whose breaking change the compiler can
fix for you, from the command line, with no editor.

When a release makes something a hard error, its diagnostic often carries a fix
the compiler knows is exactly right (a *machine-applicable* suggestion). The
editor applies these through **fix all**. `bynk check --fix` applies the same
ones from a terminal or CI.

## Steps

1. **Start from a clean working tree**, so the migration is one reviewable diff:

   ```sh
   git status --short   # nothing listed
   ```

2. **Run the fixes** from the project root (or a workspace root, which fixes every
   member, or none):

   ```sh
   bynk check --fix
   ```

   Each file written is listed, with how many edits it took:

   ```text
   fixed: src/shop.bynk (3 edits)
   ```

   The diagnostics that remain are reported as `check` reports them, and the
   exit code is non-zero if any error is left.

3. **Review and test** what changed:

   ```sh
   git diff
   bynk test
   ```

4. **Format, if you want to.** `--fix` writes the compiler's text as it is and
   does not run the formatter, so a migration commit holds only the migration.
   Run `bynk fmt .` as a separate step or commit.

5. **Undo, if you need to**, with `git checkout -- .`. There is no dry-run flag:
   the diff is the preview.

## When it refuses

`--fix` writes nothing, and exits non-zero, in two cases:

- A fix would leave a problem that needs a human: a diagnostic with no single
  machine-applicable fix, or a choice between candidates. The message names it.
  Fix that one by hand, then run `--fix` again.
- The fixes keep being applied for ten rounds, which means two of them are undoing
  each other. The message lists the codes involved; that is a compiler bug worth
  reporting.

> **Understand — the same fixes as the editor.** `--fix` uses the rule behind
> the editor's fix all, so it never takes a fix a person would have to choose
> between. It differs in repeating the fixes until none remain, because a fix can
> expose another (adding a capability to an agent handler's `given` makes the
> provider that calls it short of the same capability), and in refusing the whole
> run rather than leaving a half-migrated tree.

**See also:** [`bynk check`](/docs/bynk-cli/#applying-fixes),
[Editor support](/docs/editor-and-tooling/editor-support/).
