---
level: patch
changelog: A Worker now reports each fault it catches to Cloudflare as well as logging it, so with `--upload-source-maps` the exception `wrangler tail` shows is mapped to `.bynk` lines; the `500` and a queue message's per-message `retry()` are unchanged, and `fetch` and `queue` take an optional `ctx` (#2192)
---
