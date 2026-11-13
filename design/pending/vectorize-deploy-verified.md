---
level: patch
changelog: `Vectorize` `@dimensions` below 32 is refused at compile time (Cloudflare's minimum), and a deploy that creates a metadata index waits until it is listed before pushing the Worker
breaking: A `Vectorize` instance declaring `@dimensions` below 32 is now `bynk.capability.invalid_option` at compile time; Cloudflare refuses to create such an index, so a deploy could never have succeeded. Declare at least 32 dimensions.
---
