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

/// Durable event delivery D2 (#3898): the prefix of every subscribing service's event queue.
/// The compiler owns every queue under it; user code may not name one
/// (`bynk.queue.reserved_name`).
pub const EVENT_QUEUE_PREFIX: &str = "bynk-ev-";

/// Durable event delivery D2 (#3898): the prefix of every subscribing service's event
/// dead-letter queue. It has [`EVENT_QUEUE_PREFIX`]'s length, so one budget covers both names.
pub const EVENT_DEAD_LETTER_PREFIX: &str = "bynk-dl-";

/// The queue-name prefixes the compiler reserves for event delivery. Only these two are
/// reserved, not `bynk-` as a whole.
pub const RESERVED_QUEUE_PREFIXES: [&str; 2] = [EVENT_QUEUE_PREFIX, EVENT_DEAD_LETTER_PREFIX];

/// The reserved prefix `name` starts with, if any (`bynk.queue.reserved_name`).
pub fn reserved_queue_prefix(name: &str) -> Option<&'static str> {
    RESERVED_QUEUE_PREFIXES
        .into_iter()
        .find(|prefix| name.starts_with(prefix))
}

/// Cloudflare's longest queue name.
const QUEUE_NAME_MAX: usize = 63;

/// The hex digits of SHA-256 an event queue's name keeps.
const EVENT_QUEUE_HASH_DIGITS: usize = 8;

/// The fewest characters of slug an event queue's name keeps, however long the environment.
const EVENT_QUEUE_MIN_SLUG: usize = 1;

/// The longest environment name an event queue's name fits under: `<prefix><slug>-<hash>`
/// with the shortest slug, plus `-<environment>`, is Cloudflare's 63 characters.
pub const EVENT_QUEUE_ENVIRONMENT_MAX: usize = QUEUE_NAME_MAX
    - EVENT_QUEUE_PREFIX.len()
    - EVENT_QUEUE_MIN_SLUG
    - 1
    - EVENT_QUEUE_HASH_DIGITS
    - 1;

/// D2: the event queue of subscribing service `service` of the context qualified as `context`
/// (`shop.billing`), as deployed in `environment`: `bynk-ev-<slug>-<hash>`, passed through
/// [`env_qualify`]. `None` when `environment` is longer than [`EVENT_QUEUE_ENVIRONMENT_MAX`].
pub fn event_queue_name(environment: &str, context: &str, service: &str) -> Option<String> {
    event_queue_name_with(EVENT_QUEUE_PREFIX, environment, context, service)
}

/// D2: the event dead-letter queue of the same service: `bynk-dl-<slug>-<hash>`, qualified as
/// [`event_queue_name`] is.
pub fn event_dead_letter_queue_name(
    environment: &str,
    context: &str,
    service: &str,
) -> Option<String> {
    event_queue_name_with(EVENT_DEAD_LETTER_PREFIX, environment, context, service)
}

fn event_queue_name_with(
    prefix: &str,
    environment: &str,
    context: &str,
    service: &str,
) -> Option<String> {
    let suffix = if environment == "default" {
        0
    } else {
        environment.len() + 1
    };
    let budget =
        (QUEUE_NAME_MAX - prefix.len() - 1 - EVENT_QUEUE_HASH_DIGITS).checked_sub(suffix)?;
    if budget < EVENT_QUEUE_MIN_SLUG {
        return None;
    }
    let pair = event_queue_pair(context, service);
    let full = event_queue_slug(&pair);
    let slug = truncated_slug(&full, budget);
    let hash = event_queue_hash(&pair);
    Some(env_qualify(environment, &format!("{prefix}{slug}-{hash}")))
}

/// The text both the slug and the hash are taken from: `<qualified context>/<service>`.
fn event_queue_pair(context: &str, service: &str) -> String {
    format!("{context}/{service}")
}

/// The pair, lower-cased, with every character outside `[a-z0-9]` written `-`.
fn event_queue_slug(pair: &str) -> String {
    pair.chars()
        .map(|c| {
            let c = c.to_ascii_lowercase();
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else {
                '-'
            }
        })
        .collect()
}

/// The slug cut to at most `budget` characters, without a trailing `-`, so the separator
/// before the hash is a single `-`. A run of `-` inside the slug (from `__`, or `_` before a
/// `.`) is kept as D2 writes it: Cloudflare accepts it, and the hash keeps names apart.
fn truncated_slug(slug: &str, budget: usize) -> &str {
    // The slug is ASCII, so any byte index is a character boundary.
    slug[..slug.len().min(budget)].trim_end_matches('-')
}

/// The first eight hex digits of SHA-256 over the pair.
fn event_queue_hash(pair: &str) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(pair.as_bytes())
        .iter()
        .take(EVENT_QUEUE_HASH_DIGITS / 2)
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// D2 (`bynk.event.queue_name_collision`): every pair of subscribing services, given as
/// `(qualified context, service)`, whose event queue names coincide in some environment
/// whose name fits [`EVENT_QUEUE_ENVIRONMENT_MAX`], by index, each pair once with the lower
/// index first. Names are collision-checked rather than injective: the hash is short and the
/// slug is truncated, so two services can share a name, and so a queue, which allows only one
/// consumer.
///
/// Two names coincide in an environment exactly when the hashes agree and the slugs agree once
/// truncated to that environment's budget. Every budget from the shortest slug to the whole
/// slug is tried, so a pair is reported if any environment would merge its queues.
pub fn event_queue_collisions(subscribers: &[(&str, &str)]) -> Vec<(usize, usize)> {
    let keyed: Vec<(String, String)> = subscribers
        .iter()
        .map(|(context, service)| {
            let pair = event_queue_pair(context, service);
            (event_queue_hash(&pair), event_queue_slug(&pair))
        })
        .collect();
    let widest = QUEUE_NAME_MAX - EVENT_QUEUE_PREFIX.len() - 1 - EVENT_QUEUE_HASH_DIGITS;
    let mut out = Vec::new();
    for (i, (hash_a, slug_a)) in keyed.iter().enumerate() {
        for (j, (hash_b, slug_b)) in keyed.iter().enumerate().skip(i + 1) {
            let coincide = hash_a == hash_b
                && (EVENT_QUEUE_MIN_SLUG..=widest)
                    .any(|budget| truncated_slug(slug_a, budget) == truncated_slug(slug_b, budget));
            if coincide {
                out.push((i, j));
            }
        }
    }
    out
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

    #[test]
    fn an_event_queue_is_named_by_its_slug_and_hash_and_qualified_like_any_queue() {
        let hash = event_queue_hash("shop.billing/notify");
        assert_eq!(hash.len(), 8);
        assert!(
            hash.bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_eq!(
            event_queue_name("default", "shop.billing", "notify").as_deref(),
            Some(format!("bynk-ev-shop-billing-notify-{hash}").as_str())
        );
        assert_eq!(
            event_dead_letter_queue_name("staging", "shop.billing", "notify").as_deref(),
            Some(format!("bynk-dl-shop-billing-notify-{hash}-staging").as_str())
        );
    }

    #[test]
    fn an_event_queue_name_is_deterministic_and_lower_case() {
        let a = event_queue_name("eu-staging", "Shop.Billing", "Notify_Owner");
        assert_eq!(
            a,
            event_queue_name("eu-staging", "Shop.Billing", "Notify_Owner")
        );
        let a = a.unwrap();
        assert!(a.starts_with("bynk-ev-shop-billing-notify-owner-"), "{a}");
        // The hash is over the pair as written, so case is not folded out of it.
        assert_ne!(
            Some(a),
            event_queue_name("eu-staging", "shop.billing", "notify_owner")
        );
    }

    #[test]
    fn both_names_fit_63_characters_under_the_longest_environment_admitted() {
        let context = format!("{}.{}", "a".repeat(40), "b".repeat(40));
        let service = "c".repeat(40);
        let longest = "e".repeat(EVENT_QUEUE_ENVIRONMENT_MAX);
        for environment in ["default", "staging", longest.as_str()] {
            for name in [
                event_queue_name(environment, &context, &service).unwrap(),
                event_dead_letter_queue_name(environment, &context, &service).unwrap(),
            ] {
                assert!(name.len() <= 63, "{name} is {} characters", name.len());
                let ok = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
                assert!(name.bytes().all(|b| ok(b) || b == b'-'), "{name}");
                let hash_at =
                    name.len() - environment_suffix(environment) - EVENT_QUEUE_HASH_DIGITS;
                assert!(!name[..hash_at].ends_with("--"), "{name}");
                assert!(name.bytes().next().is_some_and(ok) && name.bytes().last().is_some_and(ok));
            }
        }
        // The longest environment leaves exactly the shortest slug.
        let name = event_queue_name(&longest, &context, &service).unwrap();
        assert_eq!(name.len(), 63);
        assert!(name.starts_with("bynk-ev-a-"), "{name}");
        // One character more and no name fits.
        let over = "e".repeat(EVENT_QUEUE_ENVIRONMENT_MAX + 1);
        assert_eq!(event_queue_name(&over, &context, &service), None);
        assert_eq!(
            event_dead_letter_queue_name(&over, &context, &service),
            None
        );
    }

    #[test]
    fn a_very_long_service_under_the_longest_environment_still_fits() {
        // An identifier has no length limit, so the slug's truncation is what keeps the name
        // within Cloudflare's 63 characters.
        let longest = "e".repeat(EVENT_QUEUE_ENVIRONMENT_MAX);
        let context = format!("{}.{}", "a".repeat(120), "b".repeat(120));
        let service = "s".repeat(255);
        for name in [
            event_queue_name(&longest, &context, &service).unwrap(),
            event_dead_letter_queue_name(&longest, &context, &service).unwrap(),
            event_queue_name("default", &context, &service).unwrap(),
        ] {
            assert!(name.len() <= 63, "{name} is {} characters", name.len());
        }
    }

    #[test]
    fn services_whose_slugs_truncate_alike_get_distinct_names_through_the_hash() {
        // Both slugs agree on their first 60 characters, past any environment's budget, so
        // every truncation agrees; only the hash tells the two queues apart.
        let context = format!("shop.{}", "x".repeat(60));
        for environment in [
            "default",
            "staging",
            &"e".repeat(EVENT_QUEUE_ENVIRONMENT_MAX),
        ] {
            let a = event_queue_name(environment, &context, "notify").unwrap();
            let b = event_queue_name(environment, &context, "refund").unwrap();
            assert_ne!(a, b, "in {environment}");
            let cut = |n: &str| n.len() - EVENT_QUEUE_HASH_DIGITS - environment_suffix(environment);
            assert_eq!(
                a[..cut(&a)],
                b[..cut(&b)],
                "the slugs truncate alike in {environment}"
            );
        }
        assert_eq!(
            event_queue_collisions(&[(&context, "notify"), (&context, "refund")]),
            Vec::new()
        );
    }

    fn environment_suffix(environment: &str) -> usize {
        if environment == "default" {
            0
        } else {
            environment.len() + 1
        }
    }

    #[test]
    fn only_the_two_event_prefixes_are_reserved() {
        assert_eq!(reserved_queue_prefix("bynk-ev-orders"), Some("bynk-ev-"));
        assert_eq!(reserved_queue_prefix("bynk-dl-orders"), Some("bynk-dl-"));
        assert_eq!(reserved_queue_prefix("bynk-orders"), None);
        assert_eq!(reserved_queue_prefix("orders-bynk-ev-"), None);
        assert_eq!(reserved_queue_prefix("bynk-ev"), None);
        let name = event_queue_name("default", "shop", "notify").unwrap();
        assert_eq!(reserved_queue_prefix(&name), Some(EVENT_QUEUE_PREFIX));
    }

    #[test]
    fn distinct_subscribers_do_not_collide_and_one_listed_twice_does() {
        let subscribers = [
            ("shop.billing", "notify"),
            // The same slug as the first (`_` and `.` both become `-`), another hash.
            ("shop_billing", "notify"),
            ("shop.fulfil", "notify"),
            ("shop.billing", "notify"),
        ];
        assert_eq!(event_queue_collisions(&subscribers), vec![(0, 3)]);
        assert_eq!(event_queue_collisions(&subscribers[..3]), Vec::new());
    }

    #[test]
    fn a_collision_is_judged_by_the_hash_and_the_truncated_slug() {
        // Agreeing on the first slug character is enough once the hashes agree, since the
        // longest environment keeps only that character.
        let slug = event_queue_slug("a.b/c");
        assert_eq!(slug, "a-b-c");
        assert_eq!(truncated_slug(&slug, 1), "a");
        assert_eq!(truncated_slug(&slug, 2), "a");
        assert_eq!(truncated_slug(&slug, 3), "a-b");
        assert_eq!(truncated_slug(&slug, 63), "a-b-c");
    }
}
