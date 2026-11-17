---
level: patch
changelog: The `bynk` tests write their stand-in `wrangler`, `npm` and git-hook scripts from a child `sh`, so parallel test runs no longer fail intermittently with ETXTBSY (Text file busy)
---
