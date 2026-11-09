---
level: patch
changelog: "The test D1's parameter scanner treats non-ASCII characters as part of a named parameter, as SQLite does, so `:café` and `:cafè` count as two parameters (#3157)"
---
