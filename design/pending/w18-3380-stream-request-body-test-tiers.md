---
level: patch
changelog: A test case passes a `Stream[Bytes]` to a streamed `@raw` route at every tier, and a `system` case sends it as a chunked body, so the router's counted overrun answers `Rejected(PayloadTooLarge(_))`; a workerd smoke checks the platform behaviour (#3380, #3180 slice 3)
---
