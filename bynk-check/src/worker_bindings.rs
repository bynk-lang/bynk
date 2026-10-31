//! A Worker's binding names must be unique (`bynk.context.duplicate_binding`).
//!
//! A Worker's bindings come from derivations that know nothing of each other:
//! the KV namespace's fixed [`crate::firstparty::KV_BINDING_NAME`], a consumed
//! context's name (`a.b` → `A_B`), an agent's class name (`OrderEntity` →
//! `ORDER_ENTITY`) and a derived capability's (`Billing` →
//! `SERVICE_BILLING`). Two of them can meet: an agent `Kv` in a context that
//! consumes `bynk.cloudflare { Kv }` is `KV` twice, and an agent
//! `ServiceBilling` and a `capability Billing = Service` are both
//! `SERVICE_BILLING`. Wrangler takes such a
//! config without complaint and one binding hides the other, so the clash is
//! a compile error. The names themselves are never adjusted to avoid it,
//! because deployed Workers depend on them.
//!
//! The names are the emitter's (`bynk_emit::emitter::wrangler::
//! worker_bindings`, the list `wrangler.toml` is written from); this module
//! owns the check over them and the diagnostic.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;

use bynk_project::ParsedFile;
use bynk_syntax::ast::{CommonsItem, SourceUnit};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::firstparty;
use crate::project_model::ErrorSink;

/// A declaration's name in screaming snake case (`OrderEntity` →
/// `ORDER_ENTITY`): the casing of an agent's binding and of a derived
/// capability's. One function, so the two kinds of name the duplicate check
/// compares cannot drift apart (review of #1968).
pub fn screaming_snake(name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in name.chars().enumerate() {
        if i > 0 && ch.is_uppercase() {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
    }
    out
}

/// S4a (#1966): the Worker binding name of a derived capability, the kind's
/// prefix then the capability name in [`screaming_snake`] case (`Sessions` →
/// `KV_SESSIONS`, `PriceCache` → `KV_PRICE_CACHE`).
pub fn instance_binding_name(prefix: &str, capability: &str) -> String {
    format!("{prefix}_{}", screaming_snake(capability))
}

/// What put a binding into a Worker's `wrangler.toml`: the source a
/// `bynk.context.duplicate_binding` diagnostic names.
///
/// Each variant's doc also says whether a handler's `deps.env` type exposes
/// it on Workers (#1950, `bynk_emit::emitter::wrangler::handler_bindings`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BindingSource {
    /// A `[[services]]` Service Binding to another context's Worker: a
    /// consumed context whose services the context calls (#1961 — consuming
    /// one for its types or events alone binds nothing), or one subscribing to
    /// an event it publishes. Handlers see a called context's binding; never a
    /// subscriber's, which only the events fan-out calls.
    Service { context: String },
    /// The `[[kv_namespaces]]` binding the cloudflare adapter's `Kv` reads.
    /// Never seen by handlers: they reach KV through the `WorkersKv` provider,
    /// which takes the Worker's `env` at compose time.
    Kv,
    /// S4a (#1966): the `[[kv_namespaces]]` binding of a derived capability
    /// (`capability Sessions = Kv`), one per instance. Never seen by handlers:
    /// they reach it through the instance's `WorkersKv`.
    KvInstance { capability: String },
    /// Interop I5 (#1972): the `[[services]]` binding of a `Service`
    /// instance (`capability Billing = Service @worker("legacy-billing")`), a
    /// Service Binding to a Worker outside the project. Never seen by
    /// handlers: they reach it through the instance's `WorkersService`.
    ForeignService { capability: String, worker: String },
    /// P5 (#2323): the `[[r2_buckets]]` binding of an `R2` instance
    /// (`capability Archives = R2`), one bucket per instance, pinned to
    /// `jurisdiction` when it names one. Never seen by handlers: they reach
    /// it through the instance's `WorkersR2`.
    R2Instance {
        capability: String,
        jurisdiction: Option<String>,
    },
    /// Standard-library P3 (#2342): the `[[ratelimits]]` binding of a
    /// `RateLimit` instance (`capability LoginLimit = RateLimit @limit(100)
    /// @period(60.seconds)`), allowing `limit` calls per key every `period`
    /// seconds. Never seen by handlers: they reach it through the instance's
    /// `WorkersRateLimit`.
    RateLimitInstance {
        capability: String,
        limit: i64,
        period: i64,
    },
    /// Standard-library P2 (#2578): the `[[queues.producers]]` binding of a
    /// `Queue` instance (`capability OrderEvents = Queue @name("order-events")`),
    /// sending to `queue` as written (`bynk deploy` environment-qualifies it).
    /// Never seen by handlers: they reach it through the instance's
    /// `WorkersQueue`.
    QueueProducer { capability: String, queue: String },
    /// Standard-library P3b (#2582): the `[[analytics_engine_datasets]]`
    /// binding of an `Analytics` instance (`capability Signups = Analytics`),
    /// one dataset per instance under a derived name. Never seen by handlers:
    /// they reach it through the instance's `WorkersAnalytics`.
    AnalyticsDataset { capability: String },
    /// Standard-library P4a (#2602): the `[[d1_databases]]` binding of a `D1`
    /// instance (`capability Orders = D1`), one database per instance under a
    /// derived name, its id minted by `bynk deploy`. Never seen by handlers:
    /// they reach it through the instance's `WorkersD1`.
    D1Database { capability: String },
    /// A `[[durable_objects.bindings]]` entry for one of the context's agents.
    /// A handler that constructs an agent sees every local agent's binding.
    Agent { class_name: String },
    /// The `[[durable_objects.bindings]]` entry for the events fan-out class
    /// of a context whose handlers emit. Never seen by handlers: they reach it
    /// through `deps.__eventsDispatch`.
    EventsFanout { class_name: String },
}

impl BindingSource {
    /// The source in a diagnostic's words.
    pub fn describe(&self) -> String {
        match self {
            Self::Service { context } => format!("the Service Binding to context `{context}`"),
            Self::Kv => format!(
                "the KV namespace binding of `{}`'s `Kv`",
                firstparty::CLOUDFLARE_UNIT
            ),
            Self::KvInstance { capability } => {
                format!("the KV namespace binding of derived capability `{capability}`")
            }
            Self::ForeignService { capability, worker } => format!(
                "the Service Binding of derived capability `{capability}` (to Worker `{worker}`)"
            ),
            Self::R2Instance { capability, .. } => {
                format!("the R2 bucket binding of derived capability `{capability}`")
            }
            Self::RateLimitInstance { capability, .. } => {
                format!("the rate limiting binding of derived capability `{capability}`")
            }
            Self::QueueProducer { capability, queue } => format!(
                "the queue producer binding of derived capability `{capability}` (to queue `{queue}`)"
            ),
            Self::AnalyticsDataset { capability } => {
                format!("the Analytics Engine binding of derived capability `{capability}`")
            }
            Self::D1Database { capability } => {
                format!("the D1 database binding of derived capability `{capability}`")
            }
            Self::Agent { class_name } => {
                format!("the Durable Object binding of agent `{class_name}`")
            }
            Self::EventsFanout { .. } => {
                "the Durable Object binding of the events fan-out".to_string()
            }
        }
    }
}

/// One binding a Worker declares: the name its `env` field and its
/// `wrangler.toml` entry share, and where that name came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkerBinding {
    pub name: String,
    pub source: BindingSource,
}

/// The bindings in `bindings` whose name an earlier one already took, each
/// paired with that earlier one, in list order.
pub fn duplicate_bindings(bindings: &[WorkerBinding]) -> Vec<(&WorkerBinding, &WorkerBinding)> {
    let mut first: HashMap<&str, &WorkerBinding> = HashMap::new();
    let mut duplicates = Vec::new();
    for binding in bindings {
        match first.get(binding.name.as_str()) {
            Some(earlier) => duplicates.push((*earlier, binding)),
            None => {
                first.insert(&binding.name, binding);
            }
        }
    }
    duplicates
}

/// `bynk.context.duplicate_binding`: each context's Worker declares every
/// binding name once. `bindings` holds each context's bindings as its
/// `wrangler.toml` would list them. One error per clash, at the declaration
/// that brought in the second binding (an agent, or a `consumes` clause),
/// preferring one the author wrote over `consumes bynk.cloudflare`, naming
/// both sources. A clash between two sources with no declaration of their own
/// (Service Bindings added for event subscribers) is reported at the
/// context's name.
pub fn phase_duplicate_bindings(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    bindings: &BTreeMap<String, Vec<WorkerBinding>>,
    errors: &mut ErrorSink,
) {
    for (ctx, ctx_bindings) in bindings {
        let indices = groups.get(ctx).map(Vec::as_slice).unwrap_or_default();
        for (earlier, later) in duplicate_bindings(ctx_bindings) {
            // At the declaration the author can change: the later source's,
            // else the earlier's, but never the `bynk.cloudflare` clause
            // while the other source has a declaration of its own.
            let mut sources = [later, earlier];
            sources.sort_by_key(|b| b.source == BindingSource::Kv);
            let (file, span) = sources
                .iter()
                .find_map(|b| declared_at(parsed, indices, b))
                .or_else(|| context_header(parsed, indices))
                .map_or((None, Span::default()), |(file, span)| (Some(file), span));
            errors.push_for(
                file.as_deref(),
                CompileError::new(
                    "bynk.context.duplicate_binding",
                    span,
                    format!(
                        "the Worker for `{ctx}` would declare two bindings named `{}`: {} and {}",
                        later.name,
                        earlier.source.describe(),
                        later.source.describe(),
                    ),
                )
                .with_note(
                    "a Worker's bindings share one `env`, so each name must be unique; binding names follow agent, context and derived capability names, so rename the declaration behind one of them",
                ),
            );
        }
    }
}

/// Where the context declares the source of `binding`: an agent's name, or
/// the `consumes` clause naming a context (or `bynk.cloudflare`, for the KV
/// namespace). `None` for a source with no declaration of its own: the events
/// fan-out, or a Service Binding added for an event subscriber.
fn declared_at(
    parsed: &[ParsedFile],
    indices: &[usize],
    binding: &WorkerBinding,
) -> Option<(PathBuf, Span)> {
    indices.iter().find_map(|&i| {
        let SourceUnit::Context(c) = parsed[i].unit() else {
            return None;
        };
        let consumed = |target: &str| {
            c.consumes
                .iter()
                .find(|d| d.target.joined() == target)
                .map(|d| d.span)
        };
        let span = match &binding.source {
            BindingSource::Agent { class_name } => c.items.iter().find_map(|item| {
                let CommonsItem::Agent(a) = item else {
                    return None;
                };
                (a.name.name == *class_name).then_some(a.name.span)
            }),
            BindingSource::Service { context } => consumed(context),
            BindingSource::Kv => consumed(firstparty::CLOUDFLARE_UNIT),
            BindingSource::KvInstance { capability }
            | BindingSource::ForeignService { capability, .. }
            | BindingSource::R2Instance { capability, .. }
            | BindingSource::RateLimitInstance { capability, .. }
            | BindingSource::QueueProducer { capability, .. }
            | BindingSource::AnalyticsDataset { capability }
            | BindingSource::D1Database { capability } => c.items.iter().find_map(|item| {
                let CommonsItem::Capability(d) = item else {
                    return None;
                };
                (d.name.name == *capability && d.derived.is_some()).then_some(d.span)
            }),
            BindingSource::EventsFanout { .. } => None,
        }?;
        Some((parsed[i].identity_path(), span))
    })
}

/// The context's name in its first file: where a clash between two sources
/// with no declaration of their own is reported.
fn context_header(parsed: &[ParsedFile], indices: &[usize]) -> Option<(PathBuf, Span)> {
    indices.iter().find_map(|&i| {
        let SourceUnit::Context(c) = parsed[i].unit() else {
            return None;
        };
        Some((parsed[i].identity_path(), c.name.span))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn service(name: &str, context: &str) -> WorkerBinding {
        WorkerBinding {
            name: name.to_string(),
            source: BindingSource::Service {
                context: context.to_string(),
            },
        }
    }

    #[test]
    fn distinct_names_do_not_clash() {
        assert!(duplicate_bindings(&[service("A", "a"), service("B", "b")]).is_empty());
    }

    /// Three bindings on one name are two clashes, each against the first.
    #[test]
    fn every_repeat_is_reported_against_the_first() {
        let bindings = [
            service("A_B", "a.b"),
            service("A_B", "a_b"),
            service("A_B", "x"),
        ];
        let duplicates = duplicate_bindings(&bindings);
        assert_eq!(duplicates.len(), 2);
        assert!(
            duplicates
                .iter()
                .all(|(earlier, _)| *earlier == &bindings[0])
        );
        assert_eq!(duplicates[1].1, &bindings[2]);
    }
}
