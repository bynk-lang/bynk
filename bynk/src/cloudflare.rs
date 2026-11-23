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
//! 1D.3a moves the resource kinds' operations here: queues, KV, D1, R2 and Vectorize. The push,
//! the secrets, `rollback`, `dev` and `tail` follow in 1D.3b, and the REST client in 1D.3c.
//! 1D.4 then moves each resource operation to REST behind these same methods, so a kind does not
//! change when its transport does.

pub(crate) mod api;
pub(crate) mod error;
#[cfg(test)]
pub(crate) mod fake;
pub(crate) mod process;

#[cfg(test)]
pub(crate) use api::Op;
pub(crate) use api::{CloudflareApi, VectorizeOp};
#[cfg(test)]
pub(crate) use error::WranglerError;
pub(crate) use process::Wrangler;
