---
level: patch
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: Two contexts that consume the same queue are now refused, grouped or not (`bynk.queue.duplicate_consumer`), and `bynk.deploy.queue_consumer_conflict` is removed in its favour. A project this newly refuses built cleanly before but could never deploy, since Cloudflare allows one consumer Worker per queue (#3644)
---
