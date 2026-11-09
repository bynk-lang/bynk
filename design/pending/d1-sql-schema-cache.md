---
level: patch
changelog: "The D1 SQL schema check reuses a database's built schema until its migrations change, so the language server doesn't re-run every migration on each edit; test-body statements also point inside the literal"
---
