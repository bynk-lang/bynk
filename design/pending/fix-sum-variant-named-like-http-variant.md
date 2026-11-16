---
level: patch
changelog: A user sum variant spelled like an `HttpResult` variant (`Found`, `Gone`, ...) is its own constructor inside an `HttpResult` handler, so it no longer type-checks as the HTTP response or appears in the wire contract and OpenAPI document (#3631)
---
