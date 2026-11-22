//! What an application composes, rendered per target (back-half paydown 1B).
//!
//! The two composition roots live side by side here: the Bundle target's
//! single `compose.ts` ([`bundle`]) and each Cloudflare Worker's own
//! `compose.ts` ([`workers`]). Both read what is composed from one
//! [`ComposePlan`] ([`plan`]): the units, their dependency edges and keys, and
//! the facts that decide which of them need `__exec` or the caller. The plan
//! says *what*; each renderer owns its own *spelling and order* (Decision C of
//! #3928), so the seven target divergences stay renderer choices, each named
//! where it is made.

use crate::emit_model::ident;
use bynk_ts::TsExpr;

pub(crate) mod bundle;
pub(crate) mod plan;
pub(crate) mod workers;

pub use plan::{ComposeFacts, ComposePlan, ComposeUnit, ConsumedSurface, ExecNeeds};

/// The `deps` object of a unit that has no entries. Both renderers spell it
/// `{  }` (a double space, not the tight `{}` an empty `TsExpr::Object`
/// prints): the pre-conversion `format!("{{ {} }}", entries.join(", "))`
/// template always put a space on each side of its slot, and the goldens carry
/// the result. One helper, so the two copies cannot drift.
pub(crate) fn empty_deps() -> TsExpr {
    ident("{  }")
}
