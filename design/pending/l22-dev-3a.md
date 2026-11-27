---
level: minor
probes: emit_diagnostics, diagnostic_coverage, explained_codes
changelog: Ahead of durable event delivery, `Events.emit` measures each emission's future queue message and logs `EventNearLimit` from 96,000 bytes of the 127,000-byte budget, an `event` with an unbounded `Bytes` or collection payload warns (`bynk.event.unbounded_payload`), and two subscribers whose event queue names would coincide are refused (`bynk.event.queue_name_collision`) (durable event delivery track, slice 3a)
breaking: Two `from Events` services whose compiler-owned event queue names coincide (an 8-digit hash collision, vanishingly rare) now fail with `bynk.event.queue_name_collision`; rename one of the two services
---
