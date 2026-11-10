# Design archive

Superseded and shelved design documents, kept for the historical record. Nothing
here is current; each file notes what replaced it. Live design docs are one level
up in [`design/`](../README.md).

| Archived file | Why | Superseded by |
|---|---|---|
| `bynk-cicd-roadmap.md` | Merged into a single engineering roadmap | [`../bynk-engineering-roadmap.md`](../bynk-engineering-roadmap.md) Part A |
| `bynk-refactor-proposal-queue.md` | Merged into the engineering roadmap (statuses refreshed; several items had landed) | [`../bynk-engineering-roadmap.md`](../bynk-engineering-roadmap.md) Part B |
| `bynk-tooling-proposal-queue.md` | Folded into the tooling roadmap as its remaining-backlog section | [`../bynk-tooling-roadmap.md`](../bynk-tooling-roadmap.md) §7 |
| `v0.29.6-refactor-codewriter.md` | Shelved — its central premise was contradicted on inspection (indentation is not centrally threaded) | n/a (not implemented) |
| `bynk-phd-exploratory-memo.md` | Dropped — the research-instrument identity was retired when Bynk committed to a single production-language identity (#540 §7(1)) | [`../bynk-positioning.md`](../bynk-positioning.md) |
| `feature-implementation-checklist.md` | A dated (~v0.107.0) audit of the design notes against the compiler; many ❌ rows have since shipped | [`../bynk-status-and-roadmap.md`](../bynk-status-and-roadmap.md) for present state; [`../proposals/README.md`](../proposals/README.md) and the PR template for the per-increment doc delta |

The first four rows were archived in the 18 June 2026 design-docs consolidation,
alongside the refresh of `bynk-status-and-roadmap.md` to v0.54; the PhD memo was
archived with the 2026-07 positioning decision.

One file here is a **rolling record** rather than a superseded doc:
[`retired-tracks.md`](retired-tracks.md) holds the closing summary of every
retired [feature track](../tracks/README.md) — appended to by each track's
retirement PR (ADR 0167), after the track doc itself is removed.
