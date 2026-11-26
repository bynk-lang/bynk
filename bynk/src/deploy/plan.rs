//! `bynk deploy`'s plan/apply flow, as six concerns:
//!
//! - `run.rs` — the run: the options, `run` and `deploy`, and the helpers that select and record.
//! - `model.rs` — the plan: its types and derivation, its report, and the confirmations.
//! - `keys.rs` — the managed and edge keys a run decides.
//! - `skew.rs` — the contract-skew policy and its refusals.
//! - `messages.rs` — the messages a run prints as it goes.
//! - `report.rs` — the `refusals` and `warnings` entries of the JSON plan (#3797).
//!
//! This parent only declares them and re-exports what the rest of `deploy` names.

// The moved bodies name this module as `super::on_account`, as they did before the split.
use super::on_account;

mod keys;
mod messages;
mod model;
mod report;
mod run;
mod shared;
mod skew;
#[cfg(test)]
pub(crate) mod tests;

pub(crate) use keys::{
    ManagedKeys, end_presence, foreign_worker_warnings, interrupted_rotation_hint,
};
pub use messages::preflight_failure_message;
pub(crate) use messages::{
    absent_dependencies_refusal, deploying_notice, foreign_binding_maybe_set_notice,
    preflight_refusal, stopped_report, unrecorded_deploy_failure,
};
pub(super) use model::print_plan;
pub(crate) use model::{
    ContextPlan, GroupNames, Plan, requires_interactive_confirmation, unrecordable_in_ci,
};
pub(crate) use report::{Entry, Stop, code, refused_report};
// What `deploy`'s own tests and the sibling test modules reach through `use plan::*`.
#[cfg(test)]
pub(crate) use keys::unknown_edge_key_notice;
#[cfg(test)]
pub(crate) use messages::foreign_binding_refusal;
#[cfg(test)]
pub(crate) use model::{derive_plan, plan_report};
pub use run::{DeployFormat, DeployOptions, run};
pub(crate) use run::{
    conflicting_env_passthrough, rebuild, record_pushed_contracts, record_pushed_tombstones,
};
pub(crate) use skew::{
    accepted_skew_notice, contract_skew_refusal, partition_accepted, record_accepted,
    report_callee_skews, shapes_to_accept, unrecorded_caller_warning, unused_accepted,
    unused_accepted_notice,
};
#[cfg(test)]
pub(crate) use skew::{callee_contract_skew_refusal, orphaned_caller_skew_refusal};
