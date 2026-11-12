---
level: patch
changelog: `bynk deploy` warns when an inbound email `Forward` destination is not a verified address
---

`bynk deploy` resolves each literal `Forward(to: "…")` of a `from email` handler against the account's Email Routing destination addresses and warns, naming the address and the handler, when one is missing or not yet verified. A destination computed at run time gets one note per handler. The check is a warning, never a failure.
