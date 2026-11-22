//! Every call bynk makes to a Cloudflare account, behind one module (back-half paydown 1D.3,
//! #4043).
//!
//! - [`api`]: [`CloudflareApi`], one method per account operation, and [`Op`], which names each
//!   call and renders its wrangler arguments.
//! - [`process`]: [`Wrangler`], the adapter that runs each operation as a wrangler process.
//! - [`error`]: [`WranglerError`], whose text is what bynk printed before the move, byte for
//!   byte, and which is the one place wrangler's prose is read ("already exists", "already
//!   gone").
//! - `fake` (tests only): [`fake::Recording`], which logs each call as its wrangler arguments and
//!   answers from a test's own replies.
//!
//! - [`rest`]: Cloudflare's REST API, one client for the run ([`rest::for_purpose`]), with typed
//!   errors ([`RestError`]); [`retry`]: its retry policy, at one attempt during the freeze.
//!
//! 1D.3a moved the resource kinds' operations here (queues, KV, D1, R2 and Vectorize), 1D.3b the
//! rest of wrangler (the push, the secrets, `rollback`, `dev` and `tail`), and 1D.3c the REST
//! client.
//! 1D.4 then moves each resource operation to REST behind these same methods, so a kind does not
//! change when its transport does.

pub(crate) mod api;
pub(crate) mod error;
#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod paging;
pub(crate) mod process;
pub(crate) mod rest;
pub(crate) mod retry;

#[cfg(test)]
pub(crate) use api::Op;
pub(crate) use api::{CloudflareApi, PushCall, VectorizeOp};
#[cfg(test)]
pub(crate) use error::WranglerError;
pub(crate) use error::{CloudflareError, Exit, Failure, push_lost_its_upload};
pub(crate) use process::{Wrangler, spawn_command};
pub(crate) use rest::Rest;
#[cfg(test)]
pub(crate) use rest::RestError;
pub(crate) use retry::Idempotent;
