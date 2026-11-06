//! `bynk-model`: the Bynk **system model**. It is the resolved description of
//! what a Bynk program *is* at the level of declarations: service protocols,
//! handler kinds and routes, type shapes, store fields, capability and
//! attached-fn signatures, `given` references, actor seams and event
//! subscriptions. Every tool that needs those facts reads them here, not from
//! the AST.
//!
//! This is distinct from `bynk_check::project_model`, which models units and
//! manifests, not declarations.
//!
//! The model sits above `bynk-check` (crate-hygiene B4, #3012). Its boundary
//! half, `bynk_check::wire::WireModel`, stays in `bynk-check` because the
//! checker's own boundary checks read it.
//!
//! Three modules, all re-exported at the root:
//! - [`vocabulary`]: the model's items (until crate-hygiene B1, #2939, the
//!   `bynk-ir` crate).
//! - [`lower`]: the builder from a checked program to those items (until B1,
//!   the `bynk-lower` crate).
//! - [`entry`]: the entry handlers' signatures, described structurally for a
//!   host such as the playground (until crate-hygiene B3, #2999,
//!   `bynk_check::entry_signature`).
//!
//! The system-model ADR (crate-hygiene track §6, `design/tracks/crate-hygiene.md`)
//! records the scope, the requirements every item is held to (M-1 one source
//! per fact, M-2 total over certified programs, M-3 a reader outside the model,
//! M-4 no new AST embedding) and what the model is not: not an expression IR,
//! not the emit ABI, and not a second checker.

pub mod entry;
pub mod lower;
pub mod vocabulary;

pub use entry::*;
pub use lower::*;
pub use vocabulary::*;
