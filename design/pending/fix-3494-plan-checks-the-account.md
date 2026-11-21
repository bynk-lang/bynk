---
level: patch
changelog: With `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` set, the `bynk deploy` plan (`--dry-run` included) checks what it would create against the account, so an R2 bucket, D1 database, Vectorize index or queue already there is planned as `reuse` instead of `create`, and a KV namespace whose title is already taken is a warning; without them the plan stays offline and says so
---
