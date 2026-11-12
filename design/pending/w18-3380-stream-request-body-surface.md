---
level: minor
changelog: A `@raw` handler's `body` may be a `Stream[Bytes]`, the request body as it arrives, counted against the cap as it is read; a body that crosses the cap answers the route's `413`, and a test case passes it a `Stream[Bytes]` at every tier (#3380, #3180 slice 3)
probes: emit_diagnostics, diagnostic_coverage, explained_codes
---
