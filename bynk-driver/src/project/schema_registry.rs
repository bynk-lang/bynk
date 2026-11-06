//! Events track, slice 3c (#980): the cross-build schema registry.
//!
//! Slice 3b (#978) added `@schema(N)` as an unconditionally-trusted
//! assertion — nothing verified it, because nothing existed to verify it
//! against. A committed `bynk.schema.lock`, keyed by qualified event name,
//! stores each event's field shape (name, type, whether the field has a
//! default) and its current schema version. Every build reconciles the live
//! shape against the stored one and either leaves the version alone, bumps
//! it (a purely additive change), or fails the build (a change that isn't
//! safely additive) — [`bynk_check::schema_registry::reconcile`] is that
//! verification, plus the auto-bump path for events that don't annotate at
//! all.
//!
//! Read/write is opt-in (`CompileOptions::schema_registry`) — see
//! `project.rs`'s `compile_project`/`run_checks` for why: fixtures and
//! in-memory builds must never touch a project tree as a side effect of
//! compiling.
//!
//! P4.0 (#1113, [DECISION A]): the document shape itself — `SchemaRegistry`,
//! `EventEntry`, `FieldShape`, and their `parse`/`serialize` — moved to
//! `bynk_project::schema_registry`. `SchemaRegistry`/`serialize` are
//! re-exported below so every existing `schema_registry::X` call site in
//! `project.rs` keeps working unchanged; `EventEntry`/`FieldShape` had no
//! remaining call site here once `reconcile` moved (see next) and are not.
//!
//! P5.3 (`design/tracks/semantics-in-the-checker.md` §6): `reconcile` and its
//! helpers (`snapshot`, `canon_type`, `Reconciled`, …) — the
//! `bynk_check`-coupled half (`UnitTable`) that stayed here pending P4.1's
//! `bynk-check` entry point — moved to `bynk_check::schema_registry`,
//! category 1 of `bynk-check/src/analysis.rs`'s own residual-gap accounting.
//! This module is now just the re-export below; `project.rs`'s `run_checks`
//! calls `bynk_check::schema_registry::reconcile` directly.
//!
//! P5.5 (§6, §3.2's "eighth site"): `project.rs`'s own
//! `bynk.project.schema_registry_corrupt` construction moved to
//! `bynk_check::schema_registry::parse_or_diagnose`, which calls
//! `bynk_project::schema_registry::parse` itself now — `parse` had no
//! remaining call site in this crate once that moved, and is not re-exported
//! below either, the same way `EventEntry`/`FieldShape` dropped out at P5.3.
//!
//! #1078: this module touches no disk. `serialize` is pure —
//! `bynk.schema.lock`'s content comes in through
//! `CompileOptions::schema_registry`'s `SchemaLock::On { existing }` and goes
//! out through `ProjectOutput::schema_lock`; `bynk-driver`'s `schema_lock`
//! module owns the actual read/atomic-write, ported verbatim (including the
//! unchanged-content no-op and the directory fsync) from what used to live
//! here.

pub(crate) use bynk_project::schema_registry::{SchemaRegistry, serialize};
