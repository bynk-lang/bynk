---
level: minor
probes: diagnostic_coverage
changelog: A record field renamed with `replaces` inside a value a `store` field holds (a `Cell`, or a `Map` value) needs no `migrate from` step, and a value stored before the rename reads back under the new name
---

## ADR: nested-field-rename
title: The lock reads a `replaces` rename inside a stored record as a rename, not a removal plus an addition
summary: A record field's `replaces` is carried on its path in the lock's per-path shapes, and the path comparison applies it to the recorded paths before comparing
**Context.** #4483, from #3793. A `replaces` on a record field was honoured on the wire (decoders accept the old key), but the lock's per-path shapes of a `store` field still saw the old path removed and the new one added without a default, so the build demanded `migrate from` for a rename the stored value would decode.

**Decision.** `PathShape` carries `renamed_from` (the old name, on the field's own path), which the path walk (`record_paths`) records from the field's `replaces`. `compare_paths` first applies each live path's rename to the recorded paths: when the lock records the old position and not the new one, and the live shape no longer has the old one, the old path and everything beneath it take the new path, shallower renames first so a field renamed inside a renamed record is found. The renames made are reported, and a rename is an additive change that bumps the class's version, as a top-level one does; a rename that also retypes, a field with no clause, and an old field kept beside its replacement are the non-additive changes they were. The lock records `renamed_from` per path (absent when none), so an unchanged build stays unchanged. A generic record instantiation's fields carry no clause. The clause is guarded as a top-level one is: the lock keeps `renamed_from` on the renamed path, and `nested_renames` judges each clause against the class's retained shapes, so deleting it while a retained version keeps the old path is `bynk.field.replaces_needed` and a clause no retained version needs is `bynk.field.replaces_settled`.

**Consequences.** A rename inside a stored record in a `Cell` or a `Map` value builds without a migration, and a value written before it reads back on workerd. The lock gains an optional `renamed_from` on a path.
