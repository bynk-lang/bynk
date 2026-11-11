//! First-party standard adapters embedded in the toolchain (v0.17 §4.2).
//!
//! The sources, bindings and metadata live in the data-only `bynk-std` crate
//! (standard-library track, #1940). This module re-exports them unchanged, so
//! `bynk_check::firstparty::*` keeps resolving for every consumer. Parsing and
//! injecting the sources is checker logic, and stays here in `project_model`.

pub use bynk_std::*;

/// Whether a parsed unit reaches `reach`: it is a context that declares a
/// service of the protocol `reach` names. Decided on the parsed declarations, so
/// a comment or a string literal that spells the words reaches nothing.
pub fn unit_reaches(unit: &bynk_syntax::ast::SourceUnit, reach: &Reach) -> bool {
    use bynk_syntax::ast::{CommonsItem, ServiceProtocol, SourceUnit};
    let Reach::ServiceProtocol(wanted) = reach;
    let SourceUnit::Context(ctx) = unit else {
        return false;
    };
    ctx.items.iter().any(|item| {
        let CommonsItem::Service(s) = item else {
            return false;
        };
        matches!(
            (&s.protocol, *wanted),
            (ServiceProtocol::Email { .. }, "email")
        )
    })
}
