//! First-party standard adapters embedded in the toolchain (v0.17 §4.2).
//!
//! The sources, bindings and metadata live in the data-only `bynk-std` crate
//! (standard-library track, #1940). This module re-exports them unchanged, so
//! `bynk_check::firstparty::*` keeps resolving for every consumer. Parsing and
//! injecting the sources is checker logic, and stays here in `project_model`.

pub use bynk_std::*;

/// Whether a parsed unit reaches `reach`: it is a context that declares a
/// service of the protocol `reach` names, or an instance of the capability it
/// names. Decided on the parsed declarations, so a comment or a string literal
/// that spells the words reaches nothing.
pub fn unit_reaches(unit: &bynk_syntax::ast::SourceUnit, reach: &Reach) -> bool {
    use bynk_syntax::ast::{CommonsItem, ServiceProtocol, SourceUnit};
    let SourceUnit::Context(ctx) = unit else {
        return false;
    };
    ctx.items.iter().any(|item| match (item, reach) {
        (CommonsItem::Service(s), Reach::ServiceProtocol(wanted)) => matches!(
            (&s.protocol, *wanted),
            (ServiceProtocol::Email { .. }, "email")
        ),
        (CommonsItem::Capability(d), Reach::CapabilityInstance(base)) => d
            .derived
            .as_ref()
            .is_some_and(|derived| derived.base.name == *base),
        _ => false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(src: &str) -> bynk_syntax::ast::SourceUnit {
        let tokens = bynk_syntax::lexer::tokenize(src).expect("lexes");
        bynk_syntax::parser::parse_unit(&tokens, src).expect("parses")
    }

    /// Hyperdrive H1 (#3318): a `Postgres` instance reaches `postgres`, decided
    /// on the declaration; a comment or a string that spells it, another kind's
    /// instance and a bare `given` reach nothing.
    #[test]
    fn a_postgres_instance_reaches_the_driver_and_nothing_else_does() {
        let reach = Reach::CapabilityInstance("Postgres");
        let declared = parse(
            "context shop.orders\nconsumes bynk.cloudflare { Postgres }\ncapability Main = Postgres\n",
        );
        assert!(unit_reaches(&declared, &reach));
        let mentioned = parse(
            "context shop.orders\n// capability Main = Postgres\nconsumes bynk.cloudflare { Kv }\ncapability Cache = Kv\nfn label() -> String { \"capability Main = Postgres\" }\n",
        );
        assert!(!unit_reaches(&mentioned, &reach));
        assert!(!unit_reaches(&declared, &Reach::ServiceProtocol("email")));
        let pg = FIRST_PARTY_NPM
            .iter()
            .find(|d| d.package == "postgres")
            .expect("postgres is a first-party dependency");
        assert_eq!(pg.runtime_module, "postgres");
        assert!(is_exact_version(pg.version));
    }
}
