---
level: patch
changelog: A record literal that leaves a defaulted field out is built as a TypeScript node rather than text, so two generated files print an enclosing closure on several lines instead of one (a ruled, whitespace-only change); no change to what the emitted code does
---
refactor(emit): record field defaults lower to nodes (1A.31)

Closes #3732. Part of #3676.

**What changed.** `bynk-check` returns a plain-data `DefaultValue` tree (`field_default_value`) instead of text, and `bynk-emit` builds the node (`default_value_node`, in `emitter/default_value.rs`), so a record literal that omits a defaulted field stays an `Object` node. `lower_field_default_value` is deleted; the wire twin is untouched. `bynk_ts` gains `TsExpr::AsConst`, and the two existing `As { ty: const }` sites move to it.

**Goldens, as ruled: 2 files, group (a), whitespace only.** An enclosing statement-bodied closure prints multi-line instead of as one text line. Token-identical once whitespace is stripped; no string or bracket changed.

**Decimal.** A `Decimal` default is emitted as its canonical string where an in-memory `{ coefficient, scale }` is expected. This is pre-existing (filed as #3801) and kept as it is here.
