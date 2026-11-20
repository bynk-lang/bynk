---
level: patch
changelog: The generated code now brackets the operand of `Duration.millis`, `toMillis`, `Instant.fromEpochMillis`, `toEpochMillis` and `Int.toFloat`, so `Duration.millis(s.length() + 1).toMillis() * 2`, unary minus and subtraction over them compute the right value instead of binding the operand's `+` or `-` to the surrounding operator
---
fix(emit): identity conversions bracket their operand (#3770)

Closes #3770. Part of #3676.
