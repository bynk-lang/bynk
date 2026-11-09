---
level: patch
probes: ts_writes
changelog: Workers WebSocket `on open`/`on message`/`on close` bodies that call a consumed context's service or construct an agent now get the Durable Object's own `env`, so the call reaches the real Service Binding or DO namespace instead of an in-memory fallback
---
