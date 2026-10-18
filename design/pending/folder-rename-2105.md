---
level: patch
changelog: Renaming or moving a folder of `.bynk` files in the editor now updates the units it holds and every `uses`/`consumes` reference to them, as a single-file rename already did; a move into another project is left alone
---

## ADR: lsp-folder-rename
title: `willRenameFiles` covers folders by expanding them into per-file renames on the server
summary: A second `**` folder filter; the server expands folders from its snapshots, deduplicates, and skips moves across project roots

**Context.** #302 registered `workspace/willRenameFiles` for `**/*.bynk` files
only, so renaming or moving a single file kept `uses`/`consumes` references in
sync but a folder move, the usual refactor when reorganising contexts, left
every reference into it broken (#2105). Unit names are derived from paths, so a
folder move is the same operation as one file move per `.bynk` file under it.

This is a `patch` increment: it adds no request, setting or author-facing
surface, only a second filter on an existing registration, and it makes an
existing editor action cover a case it skipped.

**Decision.** Registration (DECISION A): a second filter, `matches: Folder`
with the glob `**`, next to the existing file filter. A folder's name can't say
whether it holds `.bynk` files, so the server decides: a `FileRename` whose old
path has no snapshot in the analysis round is a folder, expanded to every
snapshot under it by component-wise prefix and mapped to the same relative path
under the new folder. Each file then goes through the single-file computation.
A folder with no project file produces no edit. Within one request each file is
handled once, so a file named alongside its enclosing folder is not edited
twice, and each renamed unit's references are rewritten once, since every file
of a multi-file unit maps to the same new name. The result is one merged
`WorkspaceEdit`, which the client previews.

Moves across project roots (DECISION B): when the destination resolves to a
different project root than the source (its canonicalised parent walked up to
the nearest `bynk.toml`, nested projects included), the server returns no edit
for it and logs the skip. Moving units into another project changes their
identity beyond a rename. The same check now applies to single-file moves,
which a nested `bynk.toml` inside the source project used to slip past.

**Consequences.** A folder rename or move updates every unit under it and every
reference to those units, with no change in the extension. The client sends a
`willRenameFiles` request for every folder rename in the workspace; one with no
Bynk files costs a lookup over the round's snapshots and returns nothing.
References into a folder moved to another project stay for the user to fix.
