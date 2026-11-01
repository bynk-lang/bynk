---
level: patch
changelog: "`bynk.par.question_in_join` is now permanent rather than a placeholder: `?` takes a `Result`, not an effect, so a `Result` bound in a `par` block is unwrapped with `?` after the block, where the first `Err` in source order propagates. The diagnostic's notes show that pattern and how to compute a `?` on an argument before the block. Concurrent-effects slice 1 is dropped ([#2615](https://github.com/bynk-lang/bynk/issues/2615))"
---
