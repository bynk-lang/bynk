---
level: patch
changelog: "A `system` suite call with a `Wire(…)` body to a route whose handler declares `headers: RequestHeaders` (a `@raw` route too) passes the headers to the request, as a typed call does; it converted them to a string, and the generated test failed to type-check"
---
