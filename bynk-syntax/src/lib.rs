//! Bynk's syntax foundation — the lowest leaf of the compiler crate set.
//!
//! This crate holds the modules every other layer depends *on* and none depend
//! *up* from: the lexer, the parser and its AST, source [`span`]s, the
//! [`keywords`] table, the structured [`CompileError`]
//! type, and the [`diagnostics`] code registry (the single source of truth for
//! `bynk.*` codes). Diagnostics, positions, and codes therefore cross every
//! crate without an upward edge.
//!
//! Extracted from `bynkc` as slice 1 of the crate-decomposition track (ADRs
//! 0099 layering, 0102 foundation boundary). Behaviour is unchanged from when
//! these modules lived in `bynkc`; `bynkc` now re-exports them so its public
//! API is preserved.

pub mod ast;
pub mod diagnostics;
pub mod error;
pub mod keywords;
pub mod lexer;
pub mod parser;
pub mod semantic_tokens;
pub mod span;

pub use error::{CompileError, Severity, partition_by_severity};

/// Maximum nesting depth the recursive-descent parser and the interpolation
/// lexer accept before reporting a bounded-depth diagnostic instead of
/// recursing another frame (#713). A compiler/LSP must never abort on
/// malformed source, but every nesting level costs a stack frame, so an
/// unbounded parser overflows and the process aborts (`SIGABRT`) on
/// pathologically nested input — reachable on the 8 MB main thread (~880
/// parenthesised levels) and, at ~8× smaller frames-to-stack ratio, in the
/// low hundreds on the ~1 MB stacks the LSP and the in-browser playground
/// run with.
///
/// The value sits well below the ~110 levels a 1 MB stack holds, leaving
/// comfortable headroom, and far above any realistic hand-written or
/// generated source (expression, type, and interpolation nesting past a
/// handful is already exceptional). Source that exceeds it is rejected with
/// a diagnostic rather than crashing the process.
pub(crate) const MAX_NESTING_DEPTH: usize = 64;
