//! The Bynk registry's admission rules, I/O-free (first-party registry track
//! §3.4, decision E of #2321): unpack, pack and admit an archive in memory.
//!
//! No `std::fs`, no `ring`, no network. The native registry (`bynk registry
//! add` / `serve`) links this as a library; the same source builds to a
//! standalone wasm32 module with a C ABI ([`abi`]) that the hosted registry's
//! `admission` adapter loads, so the client, the reference server and the
//! service apply one implementation.

pub mod abi;
mod admit;
mod archive;

pub use admit::{Admitted, Refusal, Requirement, admit};
pub use archive::{ArchiveError, BAD_ARCHIVE, MAX_UNPACKED, Tree, is_excluded, pack, unpack};
