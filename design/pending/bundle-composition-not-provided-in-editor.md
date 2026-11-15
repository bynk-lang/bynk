---
level: patch
probes: ts_writes
changelog: The editor now shows `bynk.capability.not_provided` in a bundle with a composition root, at the same `given` and with the same message as `bynk check`. `bynk check` also raises it when the only thing that brings the root in is a context that emits events (`Events.emit`); before, only `bynk test` and `bynk build` failed, with `tsc` errors in `compose.ts`
---
