---
title: "Appendix B — Version history"
---
This specification defines the language at its **current version, v0.441**
([§1.1](/book/spec/scope/)). Bynk is pre-1.0 and developed in small, spec-first
increments; while it is pre-1.0, an increment may change behaviour. (v0.59 is
tooling — `bynkc test --format json` — with **no language-surface change**; the
language is unchanged from v0.55, the last increment that touched it.)

This specification is the **single source of truth** for the shipped language.
The standalone development instalments that preceded it — the
`bynk-mvp-grammar-v0.X.md` files and the v0.17 adapters specification — have
been retired and removed: they were working snapshots that drifted as
ambiguities were resolved during implementation, and a maintained projection of
the language is exactly what this document replaces. Their history is preserved
in version control; the **design decisions** they recorded live on as the
decision records in `design/decisions/`.

The per-increment history — the notable change in each version from v0.5
onwards — is the
[version compatibility & changelog](/book/reference/changelog/). It is reused
here rather than duplicated; this appendix adds only the framing that the
specification is authoritative.

> [!NOTE]
> The maintenance discipline: a language increment updates **this
> specification** (and the generated grammar, diagnostics, and changelog it
> draws on) and records its language-defining calls as decision records — it
> does not add a standalone instalment document. This note is informative.
