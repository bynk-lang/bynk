//! Durable event delivery D2 (#3898): every subscribing service gets a compiler-owned event
//! queue named `bynk-ev-<slug>-<hash>` (`bynk_project::environment::event_queue_name`). The
//! names are collision-checked, not injective, and two services that shared one would share
//! a queue, which allows only one consumer. Every subscriber in a project is known when it is
//! compiled, so `bynk.event.queue_name_collision` refuses such a pair here and names both.

use std::collections::BTreeMap;

use bynk_project::environment::{event_queue_collisions, event_queue_name};
use bynk_project::{ParsedFile, UnitKind};
use bynk_syntax::ast::{CommonsItem, ServiceProtocol, SourceUnit};
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

use crate::project_model::ErrorSink;

struct Subscriber<'a> {
    context: &'a str,
    service: &'a str,
    span: Span,
    file: usize,
}

/// `bynk.event.queue_name_collision`: two `from Events` services whose event queues would
/// have the same name in some environment. Reported once per pair, at the later service.
pub fn phase_event_queue_collisions(
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
    errors: &mut ErrorSink,
) {
    let subscribers = subscribers(parsed, groups, kinds);
    let pairs: Vec<(&str, &str)> = subscribers.iter().map(|s| (s.context, s.service)).collect();
    for (a, b) in event_queue_collisions(&pairs) {
        let (first, second) = (&subscribers[a], &subscribers[b]);
        let queue = event_queue_name("default", second.context, second.service).unwrap_or_default();
        errors.push_for(
            Some(&parsed[second.file].identity_path()),
            CompileError::new(
                "bynk.event.queue_name_collision",
                second.span,
                format!(
                    "event subscribers `{}.{}` and `{}.{}` would share an event queue (`{queue}` and its environment variants)",
                    first.context, first.service, second.context, second.service,
                ),
            )
            .with_note(
                "an event queue's name keeps a short hash of its context and service, and a queue has one consumer; rename one of the two services",
            ),
        );
    }
}

/// Every `from Events` service of the project's contexts, in context then source order.
fn subscribers<'a>(
    parsed: &'a [ParsedFile],
    groups: &'a BTreeMap<String, Vec<usize>>,
    kinds: &BTreeMap<String, UnitKind>,
) -> Vec<Subscriber<'a>> {
    let mut out = Vec::new();
    for (context, indices) in groups {
        if kinds.get(context) != Some(&UnitKind::Context) {
            continue;
        }
        let mut here = Vec::new();
        for &file in indices {
            let SourceUnit::Context(ctx) = parsed[file].unit() else {
                continue;
            };
            for item in &ctx.items {
                if let CommonsItem::Service(s) = item
                    && matches!(s.protocol, ServiceProtocol::Events { .. })
                {
                    here.push(Subscriber {
                        context: context.as_str(),
                        service: s.name.name.as_str(),
                        span: s.name.span,
                        file,
                    });
                }
            }
        }
        here.sort_by_key(|s| s.span);
        out.extend(here);
    }
    out
}
