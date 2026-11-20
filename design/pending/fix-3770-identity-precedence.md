---
level: patch
changelog: The generated code now brackets the operand of `Duration.millis`, `toMillis`, `Instant.fromEpochMillis`, `toEpochMillis` and `Int.toFloat`, so `Duration.millis(s.length() + 1).toMillis() * 2`, unary minus and subtraction over them compute the right value instead of binding the operand's `+` or `-` to the surrounding operator
---
fix(emit): identity conversions bracket their operand (#3770)

Closes #3770. Part of #3676.

The five identity ops return their operand's node, so the printer of the enclosing node brackets it by precedence; a still-text operand (a family not yet converted to nodes) is bracketed by the helper unless it is a plain name or member path. New fixture `3770_identity_precedence` (7 cases) fails on `main` (`passed=1 failed=5`, the two other cases happened to pass by coincidence of values).

**Golden ruling.** Goldens `3042_ws_open_transfer_given` and `3045_ws_lifecycle_given` (`room/handlers.ts`) change: layout-only reprints. The enclosing IIFE now prints over several lines because `now.toEpochMillis()` is no longer a text exit. Blessed on the coordinator's ruling.

No probe moves.
