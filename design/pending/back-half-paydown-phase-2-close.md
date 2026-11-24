---
level: patch
changelog: Internal: the module-size cap now binds every back-half file, with no exemptions, now that Phase 2 of the back-half paydown has taken every one under 2,000 lines
---
chore: close Phase 2 of the back-half paydown

Part of #3676.

Phase 2's splits (1B.4b, 2.0–2.9) have all merged, and no back-half file is
over 2,000 production lines; the largest is `bynk-emit/src/emitter/wrangler.rs`
at 1,990. This closes the phase:

- **The cap's exemption is retired.** `GRANDFATHERED` in
  `xtask/src/module_lines.rs` listed the files already over the 3,000-line exit
  cap when the ratchet landed. 2.5b took the last of them under and left the
  list empty, keeping its rule; the maintainer approved retiring it, as its own
  doc prescribed. The list, the exemption in `violations`, `stale_grandfathered`
  and the three tests of the exemption go; the cap's own test keeps its
  assertions. `module-line-ceilings.txt` holds no entries, so
  `cargo xtask module-lines --apply` is a no-op.
- **Doc comments the splits carried** (pure moves kept them as they were) are
  put on the items they describe, or brought up to date.
- **The track doc** records each Phase 2 slice as done, with its measured sizes,
  and the §8 "back-half files ≥ 3,000 lines" row as met.

No gated probe moves, so no `probes:` line. Output is unchanged.
