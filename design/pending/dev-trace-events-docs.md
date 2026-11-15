---
level: patch
changelog: `bynk dev --trace-events` is documented, and a workerd fixture proves recording changes no outcome
---

The `bynk dev` reference documents the `request` event, the guide gains a "see
what a request did" section, and a fixture serves one project untraced and traced
under real `wrangler dev`, requiring identical answers and, from the traced run,
one spanned trace line per request.
