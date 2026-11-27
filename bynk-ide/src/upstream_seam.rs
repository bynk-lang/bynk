//! #3752 (pass-through HTTP, D20): `bynk.Upstream`'s `exchange` is the network step
//! beneath `forward`'s policy, declared so a suite can stub and observe it. The
//! checker refuses it outside a suite (`bynk.test.upstream_exchange_outside_suite`),
//! so completion and signature help offer it only in a suite file, and its hover
//! says so everywhere.

use bynk_syntax::ast::CapabilityDecl;

/// What a hover adds to `exchange`.
pub(crate) const SUITE_ONLY: &str = "\n**Suite-only.** A program forwards with `forward`, whose path and origin checks run before anything is sent; calling `exchange` outside a suite is `bynk.test.upstream_exchange_outside_suite`.\n";

/// Whether `op` of `c` is `bynk.Upstream`'s `exchange`, or a derived instance's
/// copy of it (completion follows a derived capability to its base).
pub(crate) fn is_seam(c: &CapabilityDecl, op: &str) -> bool {
    op == "exchange" && c.name.name == "Upstream" && c.ops.iter().any(|o| o.name.name == "forward")
}

/// Whether completion and signature help leave `op` of `c` out in `doc_text`.
pub(crate) fn hidden(c: &CapabilityDecl, op: &str, doc_text: &str) -> bool {
    is_seam(c, op) && !crate::locals_nav::is_suite_file(doc_text)
}
