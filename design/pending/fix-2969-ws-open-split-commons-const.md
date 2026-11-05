---
level: patch
changelog: "A Workers WebSocket `on open` keyed by a `const` of a used commons split across files now builds: `compose.ts` imports it from the file that declares it, where it imported from a commons module such a commons does not emit (#2969)"
---
