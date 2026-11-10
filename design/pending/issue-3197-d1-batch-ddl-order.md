---
level: patch
changelog: "`bynk check` prepares a literal D1 `batch`'s statements in order, each against the schema the earlier statements leave, so a batch that drops then recreates a table, or creates a table and then uses it, is no longer refused (#3197)"
---
