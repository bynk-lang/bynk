//! The language server's reusable analysis state (#3243, #1523 S2).
//!
//! [`analyse_project_memoised`] is the entry point the language server calls
//! once per round, passing the same [`AnalysisMemo`] each time. The memo keeps
//! each file's checked output (a `FileRecord`) keyed by a digest of everything
//! the check read ([`crate::check_pipeline::file_check_digest`]); a file whose
//! digest is unchanged is replayed instead of re-checked. The answer must be
//! exactly `analyse_project`'s; the differential oracle
//! (`xtask/tests/analysis_oracle.rs`) holds it to that.
//!
//! `bynk check` and `bynk build` never call this: they stay on the plain,
//! table-free path (`analyse_project` / the driver's own checks), which is the
//! cold-compile cost R3.15 asks us not to raise. A test pins it.
//!
//! # The type table's growth policy
//!
//! A cached record holds `TyId`s, so every round of one memo shares **one**
//! append-only [`Types`] table (a `TyId` stays valid as long as its table does).
//! Interning deduplicates, so an unchanged project adds nothing; but an edit
//! that mints new types leaves the old ones behind. The policy
//! (`AnalysisMemo::begin_round`): after a round the table's size is the
//! baseline; once it has grown past [`GROWTH_FACTOR`] times that baseline (and
//! past [`MIN_RESET_LEN`], so a tiny project never churns), the memo is dropped
//! and the next round starts cold with a fresh table. That bounds the table to a
//! constant multiple of one round's, by construction; the oracle's long edit
//! sequence tests it.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use crate::analysis::{ProjectAnalysis, analyse_project_cached};
use crate::check_pipeline::DeclDigests;
use crate::checker::{TypedCommons, Types};
use crate::expr_types::ExprTypeSink;
use crate::hints::HintSink;
use crate::index::RefSink;
use crate::locals::LocalsSink;
use crate::project_model::ErrorSink;
use crate::requirements::RequirementSink;
use bynk_project::{Roots, SchemaLock};

/// The type table may grow to this many times its size after the last cold round
/// before the memo starts over.
pub const GROWTH_FACTOR: usize = 4;
/// ...but never resets while smaller than this many entries.
pub const MIN_RESET_LEN: usize = 50_000;

/// Everything one file's check produced, enough to replay it into the project's
/// sinks without checking the file again.
#[derive(Clone)]
pub(crate) struct FileRecord {
    pub errors: ErrorSink,
    pub refs: RefSink,
    pub hints: HintSink,
    pub locals: LocalsSink,
    pub exprs: ExprTypeSink,
    pub requirements: RequirementSink,
    /// The typed unit, `None` when the file did not check clean. Needed by
    /// `ExecPlan::extend`, which runs every round over every file.
    pub typed: Option<Arc<TypedCommons>>,
}

struct Entry {
    digest: u64,
    record: FileRecord,
}

/// How a round used the memo, for tests and for the bench's reporting.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct MemoStats {
    /// Files replayed from the memo this round.
    pub hits: usize,
    /// Files checked this round.
    pub misses: usize,
    /// Entries held after the round.
    pub entries: usize,
    /// Size of the shared type table after the round.
    pub type_table_len: usize,
    /// Rounds in which the growth policy dropped the memo.
    pub resets: usize,
    /// Distinct shared declarations hashed in the round (their digests are
    /// dropped when it ends).
    pub decls_hashed: usize,
}

/// State reused between rounds. One per project, owned by the language server.
pub struct AnalysisMemo {
    types: Arc<Types>,
    entries: HashMap<PathBuf, Entry>,
    /// Paths looked up this round; the rest are dropped when it ends.
    seen: HashSet<PathBuf>,
    baseline: usize,
    /// Each shared declaration's digest, for the round in progress only (see
    /// [`crate::check_pipeline::DeclDigests`]); emptied when a round begins.
    decls: DeclDigests,
    growth_factor: usize,
    min_reset_len: usize,
    stats: MemoStats,
}

impl std::fmt::Debug for AnalysisMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AnalysisMemo")
            .field("entries", &self.entries.len())
            .field("baseline", &self.baseline)
            .field("stats", &self.stats)
            .finish_non_exhaustive()
    }
}

impl Default for AnalysisMemo {
    fn default() -> Self {
        Self {
            types: Arc::new(Types::new()),
            entries: HashMap::new(),
            seen: HashSet::new(),
            baseline: 0,
            decls: DeclDigests::default(),
            growth_factor: GROWTH_FACTOR,
            min_reset_len: MIN_RESET_LEN,
            stats: MemoStats::default(),
        }
    }
}

impl AnalysisMemo {
    /// A memo that has seen no round.
    pub fn new() -> Self {
        Self::default()
    }

    /// A memo with its own growth limits, so a test can exercise the reset.
    pub fn with_limits(growth_factor: usize, min_reset_len: usize) -> Self {
        Self {
            growth_factor,
            min_reset_len,
            ..Self::default()
        }
    }

    /// What the last round did.
    pub fn stats(&self) -> MemoStats {
        self.stats
    }

    /// The table every round of this memo interns into.
    pub(crate) fn types(&self) -> &Arc<Types> {
        &self.types
    }

    /// Start a round: apply the growth policy, and reset the per-round counters.
    pub(crate) fn begin_round(&mut self) {
        let resets = self.stats.resets;
        self.stats = MemoStats {
            resets,
            ..MemoStats::default()
        };
        self.seen.clear();
        // `end_round` already emptied it; a round that did not end would leave
        // addresses that a new allocation could reuse.
        debug_assert_eq!(
            self.decls.len(),
            0,
            "the previous round's digests were kept"
        );
        self.decls = DeclDigests::default();
        if self.types.len() > self.min_reset_len
            && self.types.len() > self.growth_factor * self.baseline
        {
            self.types = Arc::new(Types::new());
            self.entries.clear();
            self.baseline = 0;
            self.stats.resets += 1;
        }
    }

    /// The round's declaration digests.
    pub(crate) fn decls_mut(&mut self) -> &mut DeclDigests {
        &mut self.decls
    }

    /// The record for `path` if its digest is unchanged.
    pub(crate) fn lookup(&mut self, path: &PathBuf, digest: u64) -> Option<FileRecord> {
        self.seen.insert(path.clone());
        let hit = self
            .entries
            .get(path)
            .filter(|e| e.digest == digest)
            .map(|e| e.record.clone());
        if hit.is_some() {
            self.stats.hits += 1;
        } else {
            self.stats.misses += 1;
        }
        hit
    }

    pub(crate) fn store(&mut self, path: PathBuf, digest: u64, record: FileRecord) {
        self.entries.insert(path, Entry { digest, record });
    }

    /// End a round: forget files that are gone, and record the table's size as
    /// the baseline if this round started cold.
    pub(crate) fn end_round(&mut self) {
        let seen = &self.seen;
        self.entries.retain(|p, _| seen.contains(p));
        if self.baseline == 0 {
            self.baseline = self.types.len();
        }
        self.stats.entries = self.entries.len();
        self.stats.type_table_len = self.types.len();
        // The declaration digests are keyed by address and valid only while the
        // round's tables pin the `Arc`s: drop them with the round, so no later
        // allocation at a reused address can be handed a stale hash.
        self.stats.decls_hashed = self.decls.len();
        self.decls = DeclDigests::default();
    }
}

/// One analysis round over `roots`, free to reuse what `memo` learned from the
/// previous round. Must answer exactly what `analyse_project` answers; the
/// oracle holds it to that.
pub fn analyse_project_memoised(
    roots: &Roots,
    overlay: &HashMap<PathBuf, String>,
    schema_lock: &SchemaLock,
    memo: &mut AnalysisMemo,
) -> ProjectAnalysis {
    analyse_project_cached(roots, overlay, schema_lock, memo)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::analyse_project;

    /// #3243 S2.4a: the per-round declaration digests are keyed by address, so they
    /// must not outlive the round that pins the `Arc`s: a later allocation at a
    /// reused address would be handed a stale hash.
    #[test]
    fn a_round_leaves_no_declaration_digests_behind() {
        let root = std::env::temp_dir().join(format!("bynk-memo-decls-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let src = |name: &str| {
            format!(
                "context {name}\n\ntype Row = {{\n  id: String,\n  qty: Int,\n}}\n\nservice probe {{\n  on call(n: Int) -> Effect[Int] {{\n    Effect.pure(n)\n  }}\n}}\n"
            )
        };
        let mut overlay = HashMap::new();
        for name in ["alpha", "beta"] {
            let path = root.join(format!("{name}.bynk"));
            std::fs::write(&path, src(name)).unwrap();
            overlay.insert(path, src(name));
        }
        let roots = Roots::Single(root.clone());
        let mut memo = AnalysisMemo::new();
        for round in 0..3 {
            analyse_project_memoised(&roots, &overlay, &SchemaLock::Off, &mut memo);
            assert!(
                memo.stats().decls_hashed > 0,
                "round {round} hashed the declarations it shares"
            );
            assert_eq!(memo.decls.len(), 0, "round {round} ended with digests held");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plain_analysis_does_not_touch_a_memo() {
        // `analyse_project` has no memo parameter at all: the cold path stays
        // table-free by its signature. This pins that it is still the function
        // the memoised one is compared to.
        let f: fn(&Roots, &HashMap<PathBuf, String>, &SchemaLock) -> ProjectAnalysis =
            analyse_project;
        let _ = f;
    }
}
