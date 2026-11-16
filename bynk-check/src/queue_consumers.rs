//! One consumer Worker per queue, across the whole project (#3644).
//!
//! Cloudflare Queues accept a single consumer Worker for each queue, so two
//! contexts consuming one queue could never both deploy: the second
//! `wrangler deploy` fails. `bynk.queue.duplicate_consumer` already refuses two
//! handlers for a queue inside one context; this is the same rule across
//! contexts, grouped or not (members of a `[deploy.workers]` group are one
//! Worker, and also have one consumer for a queue, so a group needs no check of
//! its own). Like `bynk.email.duplicate_address`, which it is modelled on, it
//! reports only a claim by a *different* context, on the later one (contexts in
//! name order, so the report does not depend on discovery order).
//!
//! It runs on every target, like the email check: `from queue` is a Cloudflare
//! Queues binding, which has one consumer per queue wherever the project is
//! built, and a project that names one queue from two contexts has no single
//! consumer to deploy. That is deliberate, not a Workers-only rule.

use std::collections::{BTreeMap, HashMap};

use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{CommonsItem, ServiceProtocol, SourceUnit};
use bynk_syntax::error::CompileError;

use crate::project_model::ErrorSink;

/// Refuse a queue that two different contexts consume.
pub fn phase_duplicate_queue_consumers(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    errors: &mut ErrorSink,
) {
    // queue name -> the first context to consume it.
    let mut first: HashMap<&str, &str> = HashMap::new();
    for (ctx_name, indices) in groups {
        if kinds.get(ctx_name) != Some(&UnitKind::Context) {
            continue;
        }
        let mut here = Vec::new();
        for &i in indices {
            let SourceUnit::Context(ctx) = parsed[i].unit() else {
                continue;
            };
            for item in &ctx.items {
                if let CommonsItem::Service(s) = item
                    && let ServiceProtocol::Queue { name } = &s.protocol
                {
                    here.push((s.name.span, name.as_str(), i));
                }
            }
        }
        here.sort_by_key(|(span, _, _)| *span);
        for (span, queue, i) in here {
            match first.get(queue) {
                Some(&other) if other != ctx_name.as_str() => errors.push_for(
                    Some(&parsed[i].identity_path()),
                    CompileError::new(
                        "bynk.queue.duplicate_consumer",
                        span,
                        format!(
                            "contexts `{other}` and `{ctx_name}` both consume queue `{queue}`"
                        ),
                    )
                    .with_note(
                        "Cloudflare allows one consumer Worker per queue, so the project could not deploy: move the queue to one context",
                    ),
                ),
                Some(_) => {}
                None => {
                    first.insert(queue, ctx_name.as_str());
                }
            }
        }
    }
}
