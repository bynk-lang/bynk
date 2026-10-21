---
level: patch
changelog: A cross-context call whose callee faulted is reported as `CalleeFaulted`, naming the callee's context and service, instead of `Transport`; the callee's `500` carries `X-Bynk-Fault`, and the caller logs `CalleeFaulted <caller> -> <callee> call <service>` before it faults in turn (#2193)
---
