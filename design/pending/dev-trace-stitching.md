---
level: patch
changelog: A `bynk dev --trace-events` request names the request that called it, so a client can join a call across Workers
---

A cross-context call's Service Binding carries the calling request's id and the
id of the call step to the callee, so the callee's `request` event has a
`parent` and the caller's `call` step has a `request`. The headers ride the
internal request beside the signed ones; the signature does not cover them.
