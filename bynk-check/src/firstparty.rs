//! First-party standard adapters embedded in the toolchain (v0.17 §4.2).
//!
//! The sources, bindings and metadata live in the data-only `bynk-std` crate
//! (standard-library track, #1940). This module re-exports them unchanged, so
//! `bynk_check::firstparty::*` keeps resolving for every consumer. Parsing and
//! injecting the sources is checker logic, and stays here in `project_model`.

pub use bynk_std::*;

/// Whether a unit is first-party: the `bynk` root namespace is reserved for the
/// toolchain (`bynk.namespace.reserved`), so a unit under it is the standard
/// library's own (#3458).
pub fn is_first_party_unit(name: &bynk_syntax::ast::QualifiedName) -> bool {
    name.parts.first().is_some_and(|p| p.name == "bynk")
}

/// A capability that is bynk's own: one the toolchain's `bynk` unit declares and a
/// consumer flattens in with `consumes bynk { Cap }` (1A.33, Decision A).
///
/// The question "is this capability bynk's own `Idempotency` (or `Events`, `Clock`,
/// `Locale`)?" has one definition, [`resolve`]. The checker records its answer on
/// each capability call ([`crate::checker::Callee::Capability::first_party`]); the
/// emitter asks `resolve` where it has a unit but no call (the unit-level
/// decisions). A same-named capability from any other adapter or context is not
/// first-party and resolves to `None` (#934).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FirstPartyCap {
    /// `Idempotency`: `claim`, `dedup` and `remember`, whose keys the emitter scopes.
    Idempotency,
    /// `Events`: `emit`, buffered and flushed to the fan-out.
    Events,
    /// `Clock`: the harness clock substitutes for it in a `system` test build.
    Clock,
    /// `Locale`, provided by `LocaleProvider`, which receives the negotiation arguments.
    Locale,
}

impl FirstPartyCap {
    /// The capability's declared name in the `bynk` unit.
    pub const fn name(self) -> &'static str {
        match self {
            FirstPartyCap::Idempotency => "Idempotency",
            FirstPartyCap::Events => "Events",
            FirstPartyCap::Clock => "Clock",
            FirstPartyCap::Locale => "Locale",
        }
    }

    /// The first-party capability named `name`, ignoring where it comes from.
    fn from_name(name: &str) -> Option<FirstPartyCap> {
        [
            FirstPartyCap::Idempotency,
            FirstPartyCap::Events,
            FirstPartyCap::Clock,
            FirstPartyCap::Locale,
        ]
        .into_iter()
        .find(|c| c.name() == name)
    }
}

/// Whether the capability `name`, as seen from the unit `declaring_unit` (the unit
/// whose source the name appears in, or the provider's own unit), is bynk's own, and
/// which one. `flattened_caps` is that unit's flattened-consume map (a bare
/// `consumes bynk { Cap }` makes `Cap` a local name for the `bynk` unit's).
///
/// First-party when the name is one of [`FirstPartyCap`]'s and either this unit *is*
/// `bynk` (the capability is declared right here) or `consumes bynk { name }`
/// flattened it in. This is the single copy of the rule (#934): a same-named
/// capability from another unit (the #926/#931 generic-operation fixtures' own
/// illustrative `capability Idempotency`) must not be scoped.
pub fn resolve(
    name: &str,
    declaring_unit: &str,
    flattened_caps: &std::collections::HashMap<String, String>,
) -> Option<FirstPartyCap> {
    let cap = FirstPartyCap::from_name(name)?;
    (declaring_unit == BYNK_UNIT || flattened_caps.get(name).map(String::as_str) == Some(BYNK_UNIT))
        .then_some(cap)
}

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

    fn flattened(entries: &[(&str, &str)]) -> std::collections::HashMap<String, String> {
        entries
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// #934 / 1A.33 Decision A: a capability is first-party when this unit is `bynk`
    /// or flattened it in from `bynk`, and a same-named one from anywhere else is not.
    #[test]
    fn resolve_is_first_party_only_for_bynks_own_capability() {
        let none = flattened(&[]);
        // Declared in the `bynk` unit itself: each first-party capability resolves.
        assert_eq!(
            resolve("Idempotency", "bynk", &none),
            Some(FirstPartyCap::Idempotency)
        );
        assert_eq!(
            resolve("Events", "bynk", &none),
            Some(FirstPartyCap::Events)
        );
        assert_eq!(resolve("Clock", "bynk", &none), Some(FirstPartyCap::Clock));
        assert_eq!(
            resolve("Locale", "bynk", &none),
            Some(FirstPartyCap::Locale)
        );
        // Flattened in with `consumes bynk { Idempotency }`.
        let from_bynk = flattened(&[("Idempotency", "bynk"), ("Events", "bynk")]);
        assert_eq!(
            resolve("Idempotency", "shop.orders", &from_bynk),
            Some(FirstPartyCap::Idempotency)
        );
        assert_eq!(
            resolve("Events", "shop.orders", &from_bynk),
            Some(FirstPartyCap::Events)
        );
        // Not flattened: unscoped.
        assert_eq!(resolve("Idempotency", "shop.orders", &none), None);
        // A same-named capability flattened from another unit: unscoped.
        let other = flattened(&[("Idempotency", "vendor.keys"), ("Events", "vendor.bus")]);
        assert_eq!(resolve("Idempotency", "shop.orders", &other), None);
        assert_eq!(resolve("Events", "shop.orders", &other), None);
        // Flattened from `bynk` under a different name: nothing to resolve.
        assert_eq!(resolve("Cache", "bynk", &none), None);
        assert_eq!(
            resolve("Cache", "shop.orders", &flattened(&[("Cache", "bynk")])),
            None
        );
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

    /// Hyperdrive H5 (#3322): a `MySql` instance reaches `mysql2`, decided on
    /// the declaration, and `Postgres` does not.
    #[test]
    fn a_mysql_instance_reaches_mysql2_and_a_postgres_one_does_not() {
        let reach = Reach::CapabilityInstance("MySql");
        let declared = parse(
            "context shop.orders\nconsumes bynk.cloudflare { MySql }\ncapability Legacy = MySql @tls(\"required\")\n",
        );
        assert!(unit_reaches(&declared, &reach));
        let other = parse(
            "context shop.orders\nconsumes bynk.cloudflare { Postgres }\ncapability Main = Postgres\n",
        );
        assert!(!unit_reaches(&other, &reach));
        let my = FIRST_PARTY_NPM
            .iter()
            .find(|d| d.package == "mysql2")
            .expect("mysql2 is a first-party dependency");
        assert_eq!(my.runtime_module, "mysql");
        assert!(is_exact_version(my.version));
        // Cloudflare's mysql2 guide needs 3.13.0 or later.
        let parts: Vec<u32> = my.version.split('.').map(|p| p.parse().unwrap()).collect();
        assert!(parts >= vec![3, 13, 0], "{}", my.version);
    }
}
