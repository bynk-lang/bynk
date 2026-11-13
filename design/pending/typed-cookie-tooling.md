---
level: minor
changelog: Editor support for typed cookie writes (completion of a cookie record's fields and of `cookie`/`expire`, signature help), a guide section, and `examples/sessions` written from its cookie record
---

Tooling and documentation for the typed cookie write (the two previous increments; no new decision): after `Record.` on a cookie record the editor offers its fields, after `Record.field.` it offers `cookie(value: T) -> Cookie` and `expire() -> Cookie`, and signature help shows the same signatures inside the call. The record and field segments are recorded references, so rename and go-to-definition already work. `examples/sessions` writes and clears its session cookie from `SessionCookies`, and the HTTP guide gains a section on it.
