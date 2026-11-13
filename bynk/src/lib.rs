//! `bynk` — the Bynk driver.
//!
//! The compiler, linked in, and an orchestrator over the Node toolchain. The
//! compiler stays pure (compile / check / fmt / test); environment orchestration
//! — "is `wrangler` installed", "is your machine ready" — lives here (ADR:
//! introduce the `bynk` driver).
//!
//! v0.46 ships the first command, [`doctor`], an upfront environment check. The
//! crate is deliberately split into single-concern modules (per ADR 0060):
//!
//! - [`probe`] — the portable detection primitive (presence + version +
//!   provenance), backed by the `which` crate so it is not Unix-only.
//! - [`doctor`] — the capability model, the checks, and the exit-code contract.
//! - [`report`] — render a [`doctor::Report`] as a human table, `--format
//!   short`, or `--format json`.
//! - [`new`] — scaffold a new project (offline file-writing; no toolchain).
//! - [`dev`] — build a project and serve it locally with `wrangler dev`.
//! - [`tail`] — stream a deployed Worker's logs with `.bynk` frames (#2546).
//! - [`check`] / [`fmt`] / [`compile`] / [`test`](mod@test) — type-check, format,
//!   compile and test in-process (v0.138, #487; #2998).
//! - [`diagnostics`] — shared flatten-then-delegate rendering for in-process
//!   compiles; [`shell`] — child-process exit statuses.
//! - [`migrate`] — `bynk migrate status` and `sweep` (state-migrations S4).
//! - [`sweep`] — stopping what `bynk dev`'s wranglers leave running (#1742).
//! - `job` — on Windows, the job object that stops a wrangler's whole process
//!   tree (#1762).

pub mod approve;
pub mod book;
pub mod check;
pub mod cli;
pub mod compile;
pub mod deploy;
pub mod dev;
mod dev_hyperdrive;
mod dev_vectorize;
pub mod diagnostics;
pub mod doctor;
pub mod explain;
pub mod fetch;
pub mod fmt;
#[cfg(windows)]
pub mod job;
pub mod logs;
pub mod mcp;
pub mod migrate;
pub mod new;
pub mod openapi;
pub mod ready;
pub mod token;

// The shared detection probe (Wave 5 §5.4, findings #40/#72) moved down into
// `bynk-driver` (it replaced the old `tool_exists`
// PATH-only check). Re-exported
// here so every `crate::probe`/`bynk::probe` path in this crate resolves
// unchanged.
pub use bynk_driver::probe;
pub mod registry;
pub mod report;
pub mod shell;
pub mod stop;
pub mod sweep;
pub mod tail;
pub mod test;
pub mod workers;

/// The driver's own version, from Cargo.
pub const DRIVER_VERSION: &str = env!("CARGO_PKG_VERSION");
