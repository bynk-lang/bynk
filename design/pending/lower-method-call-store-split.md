---
level: patch
changelog: Internal refactor of the emitter, with no change to emitted TypeScript. The persisted-store receivers of a method call (held Map, Map, Set, Cache, Log, Cell) now lower in one function per kind instead of inline in `lower_method_call` (#3167).
---
