//! The language server's reusable analysis state (#3243, #1523 S2).
//!
//! [`analyse_project_memoised`] is the entry point the language server will call
//! once per round, passing the same [`AnalysisMemo`] each time. **This slice
//! only introduces the seam**: the memo is empty and the function delegates to
//! [`analyse_project`], so its answer is by construction a fresh round's. It
//! exists so the differential oracle (`xtask/tests/analysis_oracle.rs`) already
//! drives the entry point the real memoisation will sit behind: each later slice
//! adds reuse here and must keep the oracle green.
//!
//! `bynk check` and `bynk build` never call this: they stay on the plain,
//! table-free path ([`analyse_project`] / the driver's own checks), which is the
//! cold-compile cost R3.15 asks us not to raise.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::analysis::{ProjectAnalysis, analyse_project};
use bynk_project::{Roots, SchemaLock};

/// State reused between rounds. Empty until a later slice gives it content; one
/// per project, owned by the language server.
#[derive(Debug, Default)]
pub struct AnalysisMemo {
    _reserved: (),
}

impl AnalysisMemo {
    /// A memo that has seen no round.
    pub fn new() -> Self {
        Self::default()
    }
}

/// One analysis round over `roots`, free to reuse what `memo` learned from the
/// previous round. Must answer exactly what [`analyse_project`] answers; the
/// oracle holds it to that.
pub fn analyse_project_memoised(
    roots: &Roots,
    overlay: &HashMap<PathBuf, String>,
    schema_lock: &SchemaLock,
    memo: &mut AnalysisMemo,
) -> ProjectAnalysis {
    let _ = memo;
    analyse_project(roots, overlay, schema_lock)
}
