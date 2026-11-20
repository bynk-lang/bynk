---
level: patch
changelog: Provider classes and service objects are built as TypeScript declaration nodes rather than text, so a generated provider class prints a blank line between its operations and its constructor over three lines (a ruled layout change in 35 generated files, token-identical); source maps and what the emitted code does are otherwise unchanged; a consumed multi-file context's bundle namespace import now targets its smallest file path deterministically instead of the first in hash order (#3807)
probes: ts_writes, ts_raw_sites_emit
---
refactor(emit): provider and service carriers lower to declaration nodes (1A.32a)

Part of #3733. Part of #3676. #3733 closes with 1A.32c.

**What changed.** `emit_provider` returns a doc comment, an exported `Class` that implements the capability over typed fields, constructor and ops, and the `__XProvider` registration. `emit_service` returns a doc comment and an exported `const` over a multiline object of `Method` entries. The per-carrier `SourceMapBuilder`, the `print_class_method_and_merge` and `print_object_entry_and_merge` calls and both `nested_map` assignments are gone from these two functions. `bynk_ts` gains `TsDecl::Class.implements` and `end_span`, `span` on `TsClassMethod` and `TsObjectEntry::Method`, and `end_span` on a multiline object; a top-level `const` now keeps the spans of its method entries' bodies.

**Source maps are unchanged.** A new snapshot (`carrier_source_map.rs`) pins the whole decoded map of a documented provider, service, agent and history driver, and of a websocket agent. It skips blank lines: the layout below adds lines, so raw generated-line positions shift, and skipping them makes the snapshot compare line content rather than position.

**Goldens, as ruled: 35 files, layout only, token-identical.** A blank line between provider ops and before the first op after the constructor, and the provider constructor on three lines instead of one, as the printer's class policy lays it out. Service objects are byte-identical.

**Deterministic import target.** On the bundle target, a consumed context whose declarations span several files was imported through whichever of its files a `HashMap` yielded first, so repeated builds could differ. It now picks the smallest path. Any single file is still wrong for such a context: #3807 tracks importing the file that declares each name. The `2704_recursive_cycle_across_files` case is masked by the file now chosen, not fixed.
