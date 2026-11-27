---
level: patch
probes: diagnostic_coverage, explained_codes
changelog: `bynk deploy` resumes an agent move under way: with a move recorded in `bynk.deploy.lock`, a plain run plans that move from the step it reached, without `--move-agent`, and the plan says which move it resumes (the JSON plan's `moves` entry carries `resumes`). A manifest that no longer puts a moving context on its receiver, or a new move sharing a script with the move under way, is refused (`bynk.deploy.move_open`). The ledger's move record keeps each moved class's namespace id. A move is still applied only in a later release, so outside `--dry-run` it stays refused (`apply_not_yet`) (#3565)
---
