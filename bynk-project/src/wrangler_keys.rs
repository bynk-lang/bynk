//! The keys of a generated `wrangler.toml`, and what a named environment does with each
//! (back-half paydown 1C.2, Decisions B and D of #4039).
//!
//! Every top-level table the emitter writes is a [`WranglerKey`], and
//! [`WranglerKey::environment_rule`] is one exhaustive `match`: a table the emitter writes cannot
//! be left out of `[env.<name>]` by omission, which is the #3577 and #3586 bug class (`ai` and
//! `ai_search` were written at the top level and never restated for a named environment). The
//! emitter's `TomlBlock` headers take a key, the patchers and readers spell the keys through
//! [`WranglerKey::top_key`], and `xtask`'s `wrangler_key_literals` probe fails a key spelled as a
//! string literal anywhere else.
//!
//! The one-per-Worker tables of `bynk.cloudflare` (`version_metadata`, `ai`, `images` and
//! `analytics`) are named by `bynk-std`'s registry, which is language surface, so they are a
//! [`WranglerKey::Singleton`] holding the registry's own spelling. This crate sits below the
//! registry and cannot enumerate them.

/// A key of a generated `wrangler.toml`: a root entry, or a table or array of tables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WranglerKey {
    // Root entries.
    Name,
    Main,
    CompatibilityDate,
    CompatibilityFlags,
    WorkersDev,
    Addresses,
    // Tables, one per binding kind.
    Services,
    KvNamespaces,
    R2Buckets,
    RateLimits,
    QueueProducers,
    QueueConsumers,
    AnalyticsEngineDatasets,
    D1Databases,
    SecretsStoreSecrets,
    Hyperdrive,
    SendEmail,
    Vectorize,
    AiSearch,
    MtlsCertificates,
    DurableObjectBindings,
    Exports,
    Containers,
    ContainerImages,
    // Worker-wide settings.
    Vars,
    Triggers,
    Assets,
    Cache,
    Rules,
    /// #4127: Workers Logs (`[observability]`).
    Observability,
    /// #4128: the CPU limit (`[limits]`).
    Limits,
    /// #4128: Smart Placement (`[placement]`).
    Placement,
    /// A one-per-Worker table named by `bynk-std`'s registry: `version_metadata`, `ai`, `images`
    /// or `analytics`, held as the registry spells it.
    Singleton(&'static str),
}

/// What a named environment's `[env.<name>]` block does with a key, from the generated file's
/// top level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnvironmentRule {
    /// Not restated: Wrangler inherits it (#1890, #3328).
    Inherited,
    /// Copied as the top level has it, which the deploy has already made this environment's.
    Copied,
    /// The Worker's deployed name in the environment (#2028).
    DeployedName,
    /// Each entry's `field` is the resolved id of its `binding`. The table is left out when no
    /// id is resolved.
    ResolvedIds { field: &'static str },
    /// Each entry's `field` is qualified for the environment.
    Qualified { field: &'static str, form: Qualify },
    /// The values of the variables whose names start with `prefix` are dataset names, qualified
    /// for the environment (#3513 B); the others are copied.
    QualifiedVars { prefix: &'static str },
    /// Each queue, consumer and producer alike, is qualified for the environment.
    QualifiedQueues,
    /// Each Service Binding to a project Worker names its deployed name in the environment;
    /// one to a foreign Worker is copied as declared (interop I5, #1972).
    QualifiedServices,
}

/// How a name is qualified for an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qualify {
    /// `<name>-<environment>`, as Cloudflare names a Worker's own environment.
    Suffix,
    /// `<name>_<environment>`, a dataset being a SQL table name.
    Dataset,
}

impl WranglerKey {
    /// Every key but the registry's [`Self::Singleton`]s, for the tests and the probe.
    pub const ALL: &'static [WranglerKey] = &[
        Self::Name,
        Self::Main,
        Self::CompatibilityDate,
        Self::CompatibilityFlags,
        Self::WorkersDev,
        Self::Addresses,
        Self::Services,
        Self::KvNamespaces,
        Self::R2Buckets,
        Self::RateLimits,
        Self::QueueProducers,
        Self::QueueConsumers,
        Self::AnalyticsEngineDatasets,
        Self::D1Databases,
        Self::SecretsStoreSecrets,
        Self::Hyperdrive,
        Self::SendEmail,
        Self::Vectorize,
        Self::AiSearch,
        Self::MtlsCertificates,
        Self::DurableObjectBindings,
        Self::Exports,
        Self::Containers,
        Self::ContainerImages,
        Self::Vars,
        Self::Triggers,
        Self::Assets,
        Self::Cache,
        Self::Rules,
        Self::Observability,
        Self::Limits,
        Self::Placement,
    ];

    /// The key as a header or root entry spells it, dotted for a sub-table
    /// (`queues.producers`).
    pub fn header(self) -> &'static str {
        match self {
            Self::Name => "name",
            Self::Main => "main",
            Self::CompatibilityDate => "compatibility_date",
            Self::CompatibilityFlags => "compatibility_flags",
            Self::WorkersDev => "workers_dev",
            Self::Addresses => "addresses",
            Self::Services => "services",
            Self::KvNamespaces => "kv_namespaces",
            Self::R2Buckets => "r2_buckets",
            Self::RateLimits => "ratelimits",
            Self::QueueProducers => "queues.producers",
            Self::QueueConsumers => "queues.consumers",
            Self::AnalyticsEngineDatasets => "analytics_engine_datasets",
            Self::D1Databases => "d1_databases",
            Self::SecretsStoreSecrets => "secrets_store_secrets",
            Self::Hyperdrive => "hyperdrive",
            Self::SendEmail => "send_email",
            Self::Vectorize => "vectorize",
            Self::AiSearch => "ai_search",
            Self::MtlsCertificates => "mtls_certificates",
            Self::DurableObjectBindings => "durable_objects.bindings",
            Self::Exports => "exports",
            Self::Containers => "containers",
            Self::ContainerImages => "containers.images",
            Self::Vars => "vars",
            Self::Triggers => "triggers",
            Self::Assets => "assets",
            Self::Cache => "cache",
            Self::Rules => "rules",
            Self::Observability => "observability",
            Self::Limits => "limits",
            Self::Placement => "placement",
            Self::Singleton(table) => table,
        }
    }

    /// The top-level key it lives under: [`Self::header`] up to its first dot.
    pub fn top_key(self) -> &'static str {
        let header = self.header();
        header.split_once('.').map_or(header, |(top, _)| top)
    }

    /// The key whose [`Self::top_key`] is `key`, the first of them in [`Self::ALL`] when two
    /// share one (`queues`). `None` for a key the emitter does not write, and for a singleton
    /// table, which only the registry names.
    pub fn from_top_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|k| k.top_key() == key)
    }

    /// What a named environment does with this key. One exhaustive `match`, so a key added
    /// without a rule does not compile.
    pub fn environment_rule(self) -> EnvironmentRule {
        use EnvironmentRule::{
            Copied, DeployedName, Inherited, Qualified, QualifiedQueues, QualifiedServices,
            QualifiedVars, ResolvedIds,
        };
        match self {
            Self::Name => DeployedName,
            // #1890, #3328: inheritable, and nothing environment-specific about them.
            Self::Main
            | Self::CompatibilityDate
            | Self::CompatibilityFlags
            | Self::WorkersDev
            | Self::Addresses
            | Self::Assets
            | Self::Cache
            | Self::Rules
            | Self::Observability
            | Self::Limits
            | Self::Placement => Inherited,
            Self::KvNamespaces => ResolvedIds { field: "id" },
            Self::R2Buckets => Qualified {
                field: "bucket_name",
                form: Qualify::Suffix,
            },
            Self::Vectorize => Qualified {
                field: "index_name",
                form: Qualify::Suffix,
            },
            Self::AnalyticsEngineDatasets => Qualified {
                field: "dataset",
                form: Qualify::Dataset,
            },
            Self::Vars => QualifiedVars {
                prefix: crate::env_protocol::DATASET_PREFIX,
            },
            Self::QueueProducers | Self::QueueConsumers => QualifiedQueues,
            Self::Services => QualifiedServices,
            Self::Containers
            | Self::ContainerImages
            | Self::D1Databases
            | Self::Hyperdrive
            | Self::SecretsStoreSecrets
            | Self::MtlsCertificates
            | Self::SendEmail
            | Self::AiSearch
            | Self::RateLimits
            | Self::DurableObjectBindings
            | Self::Exports
            | Self::Triggers
            | Self::Singleton(_) => Copied,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};

    #[test]
    fn headers_are_unique() {
        let headers: BTreeSet<_> = WranglerKey::ALL.iter().map(|k| k.header()).collect();
        assert_eq!(headers.len(), WranglerKey::ALL.len());
    }

    /// Two keys under one top-level key (`queues.producers`, `queues.consumers`) must agree on
    /// what an environment does with it, since the block is made per top-level key.
    #[test]
    fn every_top_key_has_one_rule() {
        let mut rules: BTreeMap<&str, EnvironmentRule> = BTreeMap::new();
        for key in WranglerKey::ALL {
            let rule = key.environment_rule();
            assert_eq!(
                *rules.entry(key.top_key()).or_insert(rule),
                rule,
                "`{}` has two rules",
                key.top_key()
            );
        }
    }

    #[test]
    fn a_dotted_header_has_its_top_key() {
        assert_eq!(WranglerKey::QueueProducers.top_key(), "queues");
        assert_eq!(
            WranglerKey::DurableObjectBindings.top_key(),
            "durable_objects"
        );
        assert_eq!(WranglerKey::ContainerImages.top_key(), "containers");
        assert_eq!(WranglerKey::KvNamespaces.top_key(), "kv_namespaces");
        assert_eq!(
            WranglerKey::from_top_key("queues"),
            Some(WranglerKey::QueueProducers)
        );
        assert_eq!(
            WranglerKey::from_top_key("images"),
            None,
            "a singleton is the registry's"
        );
        assert_eq!(WranglerKey::from_top_key("nonsense"), None);
    }

    /// #1890 and #3328: the keys a named environment inherits.
    #[test]
    fn the_inherited_keys_are_the_ones_wrangler_inherits() {
        let inherited: BTreeSet<_> = WranglerKey::ALL
            .iter()
            .filter(|k| k.environment_rule() == EnvironmentRule::Inherited)
            .map(|k| k.header())
            .collect();
        assert_eq!(
            inherited,
            [
                "main",
                "compatibility_date",
                "compatibility_flags",
                "workers_dev",
                "addresses",
                "assets",
                "cache",
                "rules",
                "observability",
                "limits",
                "placement"
            ]
            .into_iter()
            .collect()
        );
    }

    #[test]
    fn a_singleton_is_copied_under_the_registrys_spelling() {
        let key = WranglerKey::Singleton("version_metadata");
        assert_eq!(key.header(), "version_metadata");
        assert_eq!(key.top_key(), "version_metadata");
        assert_eq!(key.environment_rule(), EnvironmentRule::Copied);
    }
}
