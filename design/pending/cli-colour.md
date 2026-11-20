---
level: patch
changelog: A global `--color auto|always|never` (alias `--colour`) says when `bynk` colours its output. `bynk test` shows a passing case's ✓ green, a failing case red and the tally in colour; the `--coverage` table is red, amber or green by percentage; `bynk doctor`'s marks, the build progress lines and `--help` are coloured too. `auto`, the default, keeps the existing `NO_COLOR`/`FORCE_COLOR`/`CLICOLOR_FORCE` rules, and machine-readable formats are never coloured
---
