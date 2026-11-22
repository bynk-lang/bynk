//! What an application composes, rendered per target (back-half paydown 1B.1a).
//!
//! The two composition roots live side by side here: the Bundle target's
//! single `compose.ts` ([`bundle`]) and each Cloudflare Worker's own
//! `compose.ts` ([`workers`]). This slice only moved them out of
//! `bynk-driver` and `workers.rs`; the plan both will read comes later.

pub(crate) mod bundle;
pub(crate) mod workers;
