---
level: patch
changelog: Every emitted Worker now targets compatibility date `2026-08-04`, with Node.js compatibility on by default, and the minimum `wrangler` is 4.120.1 (#3317)
breaking: Generated Workers pin `compatibility_date = "2026-08-04"`, so `bynk dev` needs wrangler 4.120.1 or newer; `bynk doctor` and `bynk dev` warn about an older one. Update with `npm install -g wrangler@4`.
---
