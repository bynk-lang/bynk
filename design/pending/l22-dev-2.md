---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: A queue name that starts with `bynk-ev-` or `bynk-dl-` is refused in a `Queue @name`, a `from queue` consumer and a `deadLetter` (`bynk.queue.reserved_name`), reserving both prefixes for the compiler's event queues (durable event delivery track, slice 2)
breaking: A `Queue @name("…")`, `from queue("…")` or `deadLetter: "…"` whose name starts with `bynk-ev-` or `bynk-dl-` now fails with `bynk.queue.reserved_name`; rename the queue (any other name, including other `bynk-` names, is unaffected)
---
