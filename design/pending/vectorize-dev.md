---
level: minor
changelog: Under `bynk dev` a `Vectorize` instance is bound, remotely, to the index `[dev.vectorize-index] <Capability> = "<index-name>"` names in `bynk.toml` and never to the deploy's, an instance with no entry is unbound and a call to it fails saying how to bind it, the first call checks the index against the declaration, and the first call prints a one-time warning that links to Cloudflare's pricing page unless `[dev] vectorize = "allow"` acknowledges it
---
