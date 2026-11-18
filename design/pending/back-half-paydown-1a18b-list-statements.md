---
level: patch
probes: ts_any
raises: ts_any — the list kernel's two `any` fallbacks (a checked `traverseTry` shape and a `sequenceAll`/`sequenceTry` shape that are absent) were the text `any`, which the probe does not see; they are now one `TsType::named("any")` in `lower_list.rs` (`any_ty`), kept at the same points as #3719 Decision D requires, and fall when the `1A.fallback` slice removes them
changelog: The `List` kernel's statement-bodied operations are built as TypeScript nodes rather than text, so each prints as a multi-line arrow function; no change to what the emitted code does
---
refactor(emit): the list kernel's statement-bodied arms lower to nodes (1A.18b)

Closes #3719. Part of #3676.
