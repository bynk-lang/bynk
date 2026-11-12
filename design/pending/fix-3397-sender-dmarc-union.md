---
level: patch
probes: ts_writes
changelog: A `Sender` identity's `dmarc` is typed as the `DmarcVerdict` union in generated TypeScript, so `s.identity.dmarc` passes to a `DmarcVerdict` parameter
---
