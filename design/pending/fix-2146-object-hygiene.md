---
level: patch
changelog: '`bynkc test` compiles the emitted TypeScript to ES modules, so a declaration named `Object` or `require` no longer fails `tsc` with TS2441 ("Compiler reserves name … in top level scope of a module"); `tsc` had been down-levelling the output to CommonJS, whose transform reserves those names (#2146)'
---
