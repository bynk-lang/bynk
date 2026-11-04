---
level: patch
changelog: A typed `query` field of a refined type or enum declared in a `uses` commons now reads as its real wire kind on Workers (so `?n=2` is no longer a 400), and a context whose only capability use is a `fn … given Fetch` now imports the `bynk` namespace its `deps` name
---
