---
level: patch
changelog: Internal restructuring: the Worker entry generator's inline blocks (route collection, WebSocket upgrade, events delivery, CORS preflight and the method fall-through) become named functions; no output changes
---
refactor: extract the Worker entry's inline blocks into functions (2.4c)

Closes #4089. Part of #3676.
