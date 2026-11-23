//! The names a deploy gives a project's resources in a named environment (back-half paydown
//! 1C.2, Decision E of #4039).
//!
//! Pure functions of `(environment, name)`. The plan prints the names, the push writes them into
//! the `[env.<name>]` block `bynk-emit` renders, and the two must agree by construction: they
//! are the one set of functions, in one crate below both (#2028).

/// `<name>` in the `default` environment, `<name>-<environment>` in any other. The suffix shape
/// Cloudflare gives a Worker's *own* deployed name under `--env`, so a Service Binding to it
/// must match exactly to resolve; queues, buckets, indexes and databases follow the same shape,
/// which keeps the resource kinds, and the mental model, consistent.
pub fn env_qualify(environment: &str, name: &str) -> String {
    if environment == "default" {
        name.to_string()
    } else {
        format!("{name}-{environment}")
    }
}

/// The name project Worker `worker` is deployed under in `environment`, and so the name every
/// Service Binding to it must carry (#2028): [`env_qualify`]. Under `--env`, Wrangler names an
/// environment that sets no `name` `<top-level name>-<env>` and uses an environment's own `name`
/// as given, so stating it changes nothing Wrangler would have done on its own; it makes Bynk,
/// not a Wrangler default, the owner of the name the plan promises.
pub fn deployed_worker_name(environment: &str, worker: &str) -> String {
    env_qualify(environment, worker)
}

/// P5 (#2323): the name an `R2` instance's bucket has on the account in `environment`. Bucket
/// names are scoped to the account, so (like a Worker's) it is a function of the project's names
/// alone.
pub fn r2_bucket_name(environment: &str, name: &str) -> String {
    env_qualify(environment, name)
}

/// #3479: the name a `Vectorize` instance's index has in `environment`.
pub fn vectorize_index_name(environment: &str, name: &str) -> String {
    env_qualify(environment, name)
}

/// P3b (#2582): the name an `Analytics` instance's dataset has in `environment`: qualified as
/// [`env_qualify`] does but with `_` (`shop_cart_signups_staging`), since the dataset is a SQL
/// table name.
pub fn analytics_dataset_name(environment: &str, name: &str) -> String {
    if environment == "default" {
        name.to_string()
    } else {
        format!(
            "{name}_{}",
            environment.to_ascii_lowercase().replace('-', "_")
        )
    }
}

/// P4a (#2602): the name a `D1` instance's database has on the account in `environment`.
pub fn d1_database_name(environment: &str, name: &str) -> String {
    env_qualify(environment, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_environment_is_the_bare_name_and_any_other_is_suffixed() {
        assert_eq!(env_qualify("default", "orders"), "orders");
        assert_eq!(env_qualify("staging", "orders"), "orders-staging");
        assert_eq!(
            deployed_worker_name("staging", "shop-orders"),
            "shop-orders-staging"
        );
        assert_eq!(r2_bucket_name("eu-staging", "media"), "media-eu-staging");
        assert_eq!(vectorize_index_name("default", "docs"), "docs");
        assert_eq!(d1_database_name("staging", "db"), "db-staging");
    }

    #[test]
    fn a_dataset_is_qualified_with_an_underscore_in_lower_case() {
        assert_eq!(analytics_dataset_name("default", "signups"), "signups");
        assert_eq!(
            analytics_dataset_name("EU-Staging", "signups"),
            "signups_eu_staging"
        );
    }
}
