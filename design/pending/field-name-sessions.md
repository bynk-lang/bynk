---
level: patch
changelog: `examples/sessions` reads its `__Host-sid` cookie through a typed `@name("__Host-sid")` record, and the HTTP reference shows both ways to read a prefixed cookie (#3343, closing slice)
---
