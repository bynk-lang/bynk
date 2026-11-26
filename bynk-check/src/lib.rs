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
//! Extracted from the old `bynkc` crate as slice 3 of the crate-decomposition
//! track. Behaviour is unchanged; the emitter and project layers above depend on
//! this crate directly.
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

// The README's `rust` examples are compiled (and, unless marked `no_run`, run)
// as doctests, so they cannot rot (#1645). `cfg(doctest)` keeps this item out of
// every build and out of the docs.rs front page, which the `//!` docs above own.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;

pub mod actors;
pub mod agent_busy;
pub mod agent_jurisdiction;
pub mod agent_state;
pub mod ai_schema;
pub mod analysis;
pub mod analytics_lock;
pub mod annotation_consts;
pub mod api_key;
pub(crate) mod app_boundary;
pub mod architecture_facts;
pub mod assets;
pub mod ast_text;
pub mod builtin_names;
#[cfg(test)]
mod callee_tests;
pub mod caller_admission;
pub mod capability_paths;
pub mod check_pipeline;
pub mod checker;
pub mod consts;
pub mod containers;
pub mod context_checks;
pub mod contract;
pub mod contract_compat;
pub mod cors_origin;
pub mod d1_migrations;
pub mod d1_sql;
pub mod email;
pub mod email_fallback;
pub mod email_route;
pub mod embeds;
pub mod emitted_names;
pub mod entailment;
pub mod env_consts;
pub mod exec_plan;
pub mod exports;
pub mod expr_types;
pub mod field_renames;
pub mod firstparty;
pub mod fixtures;
pub mod group_triggers;
pub mod hints;
pub mod http_rate_limit;
#[cfg(test)]
mod http_result_extensible_tests;
pub mod icu;
mod implicit_uses;
pub use implicit_uses::inline_prelude_units;
pub mod index;
pub mod kernel_methods;
pub mod kernel_op;
pub mod locals;
pub mod memo;
pub mod mysql_sql;
pub mod narrowing;
pub mod oracle;
pub mod ord_order;
pub mod packages;
pub mod pg_sql;
pub mod phase_timing;
pub(crate) mod prelude_record;
pub mod producer_admission;
pub mod project_model;
pub mod queue_consumers;
pub mod queue_delivery;
pub mod queue_producers;
pub mod recovery;
pub mod refinement_consts;
pub mod regex_groups;
pub mod regex_walk;
mod rename_plan;
pub mod requirements;
pub mod resolver;
pub mod route_order;
pub mod schema_registry;
#[cfg(test)]
mod scope_tests;
pub mod secrets;
pub mod shadow_uses;
pub mod stability;
pub mod store_ops;
pub mod store_write;
pub mod stub_agent;
pub mod stub_outcome;
pub mod symbols;
pub mod test_env;
pub mod test_suites;
#[cfg(test)]
pub(crate) mod testkit;
pub mod ty_ref;
pub mod unit_signature;
pub mod uses_view;
mod violates;
pub mod websocket;
pub mod wire;
pub mod wire_default;
pub mod wire_format;
pub mod wire_names;
pub mod worker_bindings;
pub mod worker_groups;

pub use firstparty::Platform;
