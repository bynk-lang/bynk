---
level: patch
changelog: A negated negation such as `- -x` now emits `-(-x)`, not `--x`, which TypeScript read as a pre-decrement (and `- -5` as a syntax error)
---
