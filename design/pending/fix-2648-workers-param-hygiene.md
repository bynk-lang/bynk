---
level: patch
changelog: A parameter named `body`, `result`, `request`, `surface` or the like no longer collides with the Workers wrapper's own locals
---

A service parameter whose name matched a local the Workers target generates
(`body`, `result`, `args`, `argsObj`, `request`, `surface`, `handlers`, ...)
passed `bynkc check` and then failed `tsc` on the emitted `index.ts` (#2648): a
`call` handler's `body` parameter was redeclared by the encoded result, and an
`http` path parameter named `request` or `surface` shadowed the wrapper's own.

The workers emitter now binds a user parameter through `wire_ident`, which sends
any such name into the generated-name namespace (`__id_<name>`) that the parser
keeps free of user identifiers. A `call` handler's `body` is renamed the same
way; on an `http` route `body` is the request body and keeps its name. Programs
whose parameters do not collide emit byte-identical TypeScript.
