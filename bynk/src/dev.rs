//! `bynk dev` — build a project and serve it locally in one step.
//!
//! Collapses the manual recipe (compile → `cd` into the generated worker dir →
//! `wrangler dev`) into a single command (proposal v0.57). The orchestration is
//! **pre-flight → compile → select → serve**, and almost every piece is reused:
//! the in-process [`workers::compile_once`](crate::workers), the doctor `Deploy`
//! capability for the Node + `wrangler` gate, and [`probe`](crate::probe) for locating
//! `wrangler` with the same provenance ordering doctor reports.
//!
//! The serve step runs `wrangler dev` in **local mode** (Miniflare), which
//! simulates KV / Durable Objects / queues keyed by *binding name* — so no
//! namespace provisioning is needed and the generated `wrangler.toml` is served
//! untouched (proposal §1, D4). Everything `wrangler`-specific is encapsulated
//! here so the serve step can later be swapped for a first-party `workerd`
//! server without touching the rest (proposal §4).
//!
//! Since #552 the step is **one `wrangler dev` per context**, not one per
//! project: the processes discover each other through wrangler's dev registry
//! and wire the emitted `[[services]]` bindings between themselves, so a
//! cross-context call resolves locally. That makes the driver a supervisor of N
//! children rather than a hand-off to one — hence the port allocation
//! ([`allocate`]), the joint teardown (`terminate`, private), and the plural
//! selection rule ([`select_contexts`]) that replaced ADR 0096 D3's ambiguity
//! error.
//!
//! The file is the facade; the children hold the parts of one session:
//! `run` (the serve loop, its options, the selection and the pre-flight),
//! `events` (what `bynk dev` tells the user, as text or JSON), `stand_ins`
//! (stand-in Workers for foreign services), `bindings` (each binding wired
//! locally or proxied, and the dev configs), `serving` (ports and the
//! arguments each `wrangler dev` gets), `supervise` (running, reading and
//! stopping the children) and `watch` (what a rebuild is triggered by and
//! what it migrates). Containers, Hyperdrive, Vectorize and the inspector live
//! in the `dev_*` files beside this one.

mod bindings;
mod events;
mod run;
mod serving;
mod stand_ins;
mod supervise;
#[cfg(test)]
mod test_support;
mod watch;

// The build/select/`wrangler` plumbing `dev` shares with `deploy` now lives in
// [`crate::workers`] (it is neither command's to own). Re-exported rather than
// merely imported, so `bynk::dev::compile_once` and `bynk::dev::select_contexts`
// — the paths this crate's integration tests already use — keep resolving.
pub use crate::workers::{
    SelectError, compile_once, discover_workers, prepare_build_dir, select_contexts,
};

pub use bindings::DEV_CONFIG;
pub use events::{DevEvent, DevFormat, ServedContext, event_line, serving_event, serving_report};
pub(crate) use run::is_remote;
pub use run::{
    DevOptions, needs_wrangler_warmup, preflight_failure_message, run, wrangler_age_notice,
};
pub use serving::{Serving, allocate, assign_free_inspector};
pub use stand_ins::{
    StandIn, foreign_services, stand_in_files, stand_in_name, stand_in_report, stand_ins,
};
pub(crate) use supervise::forward_lines;
