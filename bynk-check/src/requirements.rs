//! v0.99 (ADR — agent capability provenance): the capability-requirement ledger.
//!
//! A requirement is *where a capability is needed and why*. The checker already
//! decides, at every capability-consuming site, whether the enclosing handler's
//! `given` covers it (and errors when it does not). This sink records **every**
//! such decision — covered or not — so the editor surfaces can answer two
//! questions the bare diagnostic cannot:
//!
//! - *"What `given` does this handler still need?"* — the uncovered requirements
//!   drive the materializable ghost `given` inlay hint (DECISION E).
//! - *"Why does this handler declare `given Clock`?"* — the covered requirements
//!   explain, on hover, what a declared capability is *for*.
//!
//! The sink mirrors [`HintSink`](crate::hints::HintSink): a `&mut` parameter
//! threaded through the checker entry points, NOT part of the `Ok(TypedCommons)`
//! payload — so requirements persist through a transient type error at every
//! site the checker still reaches. Spans are bare byte offsets into the file the
//! sink was attributed to via [`RequirementSink::enter_file`].
//!
//! **Decisive property (DECISION C):** the human *reason* is a total function of
//! the [`RequirementSource`] — a small closed enum — with **no per-capability
//! text**. Adding a new capability needs zero new reason text; a fragment is
//! authored only when a new capability-*consuming feature* is added (a store
//! kind, a builtin) — a closed, compiler-internal set.

use crate::capability_paths::{CapabilityGraph, FileCapabilityPaths};
use bynk_syntax::span::Span;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Why a capability requirement arises. The reason renders from this alone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequirementSource {
    /// The body calls `Cap.op(...)` directly — the call site *is* the
    /// explanation. Correct for **any** capability, including user-defined ones
    /// (`Payments.authorise` → *"calls `Payments.authorise`"*) with no bespoke
    /// text.
    DirectCall { op: String },
    /// A storage op consumes a capability. `(kind, op)` keys a reason fragment
    /// owned by the storage feature — the only code that knows *why* a store
    /// needs a capability (e.g. `Cache` eviction reads the clock).
    StoreOp { kind: StoreKind, op: String },
    /// A language builtin draws on a capability (e.g. `Uuid` → `Random`). The
    /// fragment is owned by the builtin's surface.
    Builtin { feature: String },
}

/// The storage kinds that consume a capability. A closed set: a new kind is a
/// deliberate language addition, and adding one is the only time a new store
/// reason fragment is authored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StoreKind {
    Cache,
    Log,
}

impl StoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            StoreKind::Cache => "Cache",
            StoreKind::Log => "Log",
        }
    }
}

impl RequirementSource {
    /// The human reason for a requirement, derived purely from the source — no
    /// per-capability text. `capability` names the required capability so a
    /// `DirectCall` can render `calls \`Cap.op\``.
    pub fn reason(&self, capability: &str) -> String {
        match self {
            RequirementSource::DirectCall { op } => format!("calls `{capability}.{op}`"),
            RequirementSource::StoreOp { kind, op } => store_reason(*kind, op).to_string(),
            RequirementSource::Builtin { feature } => {
                format!("the `{feature}` builtin draws on `{capability}`")
            }
        }
    }
}

/// The storage feature's `(kind, op) -> reason` table — the one place that knows
/// *why* a store consumes a capability. Total over the clock-consuming store ops
/// (`requirements::tests::store_reason_table_is_total` pins this); a `(kind, op)`
/// outside the table falls back to a generic phrasing rather than panicking, so
/// a new store op can never crash a render before its fragment is authored.
pub fn store_reason(kind: StoreKind, op: &str) -> &'static str {
    match (kind, op) {
        // Every `Cache` op but `remove` applies TTL expiry, which reads the clock.
        (StoreKind::Cache, "put" | "get" | "update" | "upsert" | "contains" | "size") => {
            "a `Cache` operation applies TTL expiry, which reads the clock"
        }
        // `Log.append` stamps the current time.
        (StoreKind::Log, "append") => "`Log.append` stamps the current time, which reads the clock",
        _ => "a storage operation consumes this capability",
    }
}

/// One capability requirement: which capability, at what site, why, and whether
/// the enclosing handler already covers it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    /// The required capability's simple name (the deps key) — e.g. `Clock`.
    pub capability: String,
    /// The consuming site (the op or direct-call span).
    pub site: Span,
    pub source: RequirementSource,
    /// `true` when the enclosing handler's `given` already lists the capability.
    /// An uncovered requirement drives the ghost `given` inlay hint; a covered
    /// one explains, on hover, what a declared capability is for.
    pub covered: bool,
    /// For an **uncovered** requirement, where the ghost `given` would render
    /// (the handler's return-type span) — and the edit that materializes the
    /// clause. `None` for covered requirements and where no anchor applies
    /// (e.g. a provider body, which has no return-type-anchored `given`).
    pub materialize: Option<Materialize>,
}

/// The data the ghost `given` inlay hint needs to render and one-click apply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Materialize {
    /// Where the ghost clause renders — the end of the handler's return type.
    pub anchor: Span,
    /// The text edit that writes the real `given Cap` (or `, Cap`) clause.
    pub edit_span: Span,
    pub edit_text: String,
}

/// Project-relative source path → that file's requirements, span-ordered.
pub type FileRequirements = HashMap<PathBuf, Vec<Requirement>>;

/// Records capability requirements per file. A fresh sink records nothing until
/// [`enter_file`](Self::enter_file) attributes it.
#[derive(Debug, Default)]
pub struct RequirementSink {
    files: FileRequirements,
    /// #2469: per-file capability paths, built after a unit's bodies are checked.
    paths: FileCapabilityPaths,
    file: Option<PathBuf>,
    /// Set for synthetic (toolchain-injected) and test/integration files:
    /// requirements are discarded — they never surface in an editor.
    muted: bool,
}

impl RequirementSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// Enter a per-file recording context.
    pub fn enter_file(&mut self, file: &Path, muted: bool) {
        self.file = Some(file.to_path_buf());
        self.muted = muted;
    }

    /// Record a requirement. Dropped when muted or before any `enter_file`.
    pub fn record(&mut self, req: Requirement) {
        if self.muted {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        self.files.entry(file.clone()).or_default().push(req);
    }

    /// The requirements recorded so far for the file being checked.
    pub fn current(&self) -> &[Requirement] {
        self.file
            .as_ref()
            .and_then(|f| self.files.get(f))
            .map_or(&[], Vec::as_slice)
    }

    /// #2469: attach the capability graph built over the file being checked.
    /// Dropped when muted or before any `enter_file`, like [`record`](Self::record).
    pub fn record_paths(&mut self, graph: CapabilityGraph) {
        if self.muted || graph.is_empty() {
            return;
        }
        let Some(file) = &self.file else {
            return;
        };
        self.paths.entry(file.clone()).or_default().merge(graph);
    }

    /// Drain the capability paths recorded per file.
    pub fn take_paths(&mut self) -> FileCapabilityPaths {
        std::mem::take(&mut self.paths)
    }

    /// Drain the recorded requirements, each file's entries ordered by span.
    pub fn take_files(&mut self) -> FileRequirements {
        let mut files = std::mem::take(&mut self.files);
        for reqs in files.values_mut() {
            reqs.sort_by_key(|r| (r.site.start, r.site.end));
        }
        files
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// DECISION C: the store-reason table is total over the clock-consuming
    /// store ops — every op that records a `StoreOp` requirement has a concrete
    /// (non-fallback) fragment, so the reason is never the generic placeholder
    /// for a real requirement.
    #[test]
    fn store_reason_table_is_total() {
        let cache_ops = ["put", "get", "update", "upsert", "contains", "size"];
        for op in cache_ops {
            let r = store_reason(StoreKind::Cache, op);
            assert_ne!(
                r, "a storage operation consumes this capability",
                "Cache.{op} has no concrete reason fragment"
            );
        }
        assert_ne!(
            store_reason(StoreKind::Log, "append"),
            "a storage operation consumes this capability",
            "Log.append has no concrete reason fragment"
        );
    }

    /// DECISION C: a user-defined capability's `DirectCall` reason renders with
    /// no bespoke entry — the call site is the whole explanation.
    #[test]
    fn direct_call_reason_needs_no_bespoke_entry() {
        let src = RequirementSource::DirectCall {
            op: "authorise".to_string(),
        };
        assert_eq!(src.reason("Payments"), "calls `Payments.authorise`");
    }
}
