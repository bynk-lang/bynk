---
level: patch
changelog: A `system` suite reads a `Raw` response back as `Raw(body, contentType)`, as the unit tier does (#3050); a `system` suite asserting `expect r is Ok(_)` on a raw or streamed route now sees `Raw`, so change it to `Raw(_, _)`
---
