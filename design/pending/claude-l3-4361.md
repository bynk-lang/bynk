---
level: patch
changelog: The `.gitignore` `bynk new` writes now also ignores `/out` (the TypeScript `bynk check`, `bynk test` and `bynk build` write), `.wrangler/` and `node_modules/`, so a first `git add -A` no longer stages generated output
---

## ADR: new-scaffold-ignores-generated-output
title: The scaffolded .gitignore covers the output the tools write
summary: Which paths the template ignores, and why each

**Context.** #4361: `bynk new` wrote a `.gitignore` that ignored only `/.bynk`, but `bynk check`, `bynk test` and `bynk build` write the compiled TypeScript to `out/` in the project root. In a fresh project, `git add -A` after a first `bynk test` staged the generated output; the first project to hit it (bynk-lang/turn-up#1) worked around it with its own root `.gitignore`.

**Decision.** The template ignores four paths. `/out` is the build output the issue reports, anchored to the project root so a source directory named `out` deeper in the tree is not hidden. `/.bynk` stays: it is `bynk dev`'s build dir (and `bynk` also writes its own `.bynk/.gitignore`). `.wrangler/` is the local state a `wrangler dev` or a deploy leaves, in the project root or in any worker dir under it, so it is unanchored. `node_modules/` is what an `npm install` for the Cloudflare tooling (or a project's own TypeScript, as the showcase has) leaves, and it is likewise unanchored. The showcase's `out-js/` is left out: no `bynk` command writes it by default, so a scaffolded project has no use for it. The embedded template is the only source (`bynk/src/templates/gitignore`); the golden `bynk/tests/golden/new-scaffold.txt` follows it, and the two docs that show the scaffold's tree list the same paths. An existing `.gitignore` is still left untouched (#737), so a project scaffolded before this change keeps what it has and adds the lines by hand.

**Consequences.** A fresh `bynk new` project can `git add -A` straight after a first `bynk test` without staging generated output. No language or compiler change.
