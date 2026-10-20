//! Bynk's semantic-analysis layer — the crate between `bynk-syntax` and the
//! emitter.
//!
//! Holds name resolution (`resolver`), type checking (`checker`), the registries
//! the checker dispatches and the LSP reads (`kernel_methods`, `builtin_names`),
//! the first-party embedded sources (`firstparty`), actor analysis (`actors`),
//! and the **captured analysis tables** written during resolution/checking:
//! the binding `index`, inlay `hints`, `expr_types`, and `locals`. These tables
//! are produced here and *queried* by the IDE layer (`bynk-ide`, a later slice):
//! captured tables live in `bynk-check`, queries live above it (ADR 0102, and
//! the crate-decomposition track's check↔IDE seam).
//!
//! Extracted from `bynkc` as slice 3 of the crate-decomposition track. Behaviour
//! is unchanged; `bynkc` depends on this crate and re-exports its modules so its
//! public API and the emitter/project layers above are untouched.
//!
//! P4.1 (#1115) added this crate's first project-level orchestration:
//! `project_model` (discovery→parse→group→resolve, shared with `bynk-emit`'s
//! `run_checks`), `check_pipeline` (the per-file resolve+check core, also
//! shared), and `analysis` (`ProjectAnalysis` and the new
//! `analyse_project` entry point `bynk-ide` will call once P4.2 repoints it).
//! `context_checks` — the per-file context/capability/provider/service/agent
//! validation, previously `bynk-emit`'s `validate.rs` — moved here in the
//! same slice, for the same reason: `project_model`'s and `check_pipeline`'s
//! shared pipeline needs it, and duplicating it into a second copy is exactly
//! what this crate's extraction discipline exists to avoid.

pub mod actors;
pub mod analysis;
pub mod assets;
pub mod builtin_names;
pub mod check_pipeline;
pub mod checker;
pub mod context_checks;
pub mod contract;
pub mod exports;
pub mod expr_types;
pub mod firstparty;
pub mod hints;
pub mod icu;
pub mod index;
pub mod kernel_methods;
pub mod locals;
pub mod narrowing;
pub mod packages;
pub mod project_model;
pub mod recovery;
pub mod requirements;
pub mod resolver;
pub mod schema_registry;
#[cfg(test)]
mod scope_tests;
pub mod secrets;
pub mod shadow_uses;
pub mod stability;
pub mod store_ops;
pub mod symbols;
pub mod test_env;
pub mod test_suites;
#[cfg(test)]
pub(crate) mod testkit;
pub mod unit_signature;
pub mod websocket;
pub mod wire;
pub mod wire_default;
pub mod wire_format;
pub mod worker_bindings;

pub use firstparty::Platform;
