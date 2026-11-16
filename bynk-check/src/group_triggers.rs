//! What a Worker answers, when several contexts share it (#3562, G2b step 2c).
//!
//! A `[deploy.workers.<key>]` group is one Worker, with one `fetch`, one `queue`
//! and one `email` entry, so each thing it is routed by must belong to one
//! member. These are the rules a single context already keeps among its own
//! services (`bynk.http.duplicate_route`), kept across the members of a group,
//! with the same keys:
//!
//! - an HTTP route is its method and path;
//! - a queue is its name, and a Worker has one consumer for it.
//!
//! An email address is not checked here: a project already refuses two contexts
//! claiming one (`bynk.email.duplicate_address`), grouped or not.
//!
//! A cron expression is not one of these: two members may share it, and both
//! their handlers run.
//!
//! One error per clash, at the later member's declaration, naming both members.

use std::collections::{BTreeMap, HashMap};

use bynk_project::ParsedFile;
use bynk_syntax::ast::{CommonsItem, HandlerKind, HttpMethod, ServiceProtocol, SourceUnit};
use bynk_syntax::error::CompileError;

use crate::project_model::ErrorSink;

/// What one member claimed first, by key: the member, so a later member's
/// clash can name it.
#[derive(Default)]
struct Claims<'a> {
    routes: HashMap<(HttpMethod, String), &'a str>,
    queues: HashMap<String, &'a str>,
}

/// Check the members of one group for a route or queue two of them claim.
pub fn group_trigger_conflicts(
    key: &str,
    members: &[String],
    parsed: &[ParsedFile],
    groups: &BTreeMap<String, Vec<usize>>,
    errors: &mut ErrorSink,
) {
    let mut claims = Claims::default();
    for ctx in members {
        let indices = groups.get(ctx).map(Vec::as_slice).unwrap_or_default();
        for &i in indices {
            let SourceUnit::Context(c) = parsed[i].unit() else {
                continue;
            };
            let path = parsed[i].identity_path();
            let mut clash = |code: &'static str, span, what: String, earlier: &str, note: &str| {
                errors.push_for(
                    Some(&path),
                    CompileError::new(
                        code,
                        span,
                        format!(
                            "contexts `{earlier}` and `{ctx}` share Worker `{key}`, and both {what}"
                        ),
                    )
                    .with_note(note.to_string()),
                );
            };
            for item in &c.items {
                let CommonsItem::Service(service) = item else {
                    continue;
                };
                if let ServiceProtocol::Queue { name } = &service.protocol {
                    match claims.queues.get(name) {
                        Some(&earlier) if earlier != ctx => clash(
                            "bynk.deploy.queue_consumer_conflict",
                            service.name.span,
                            format!("consume queue `{name}`"),
                            earlier,
                            "a Worker has one consumer for a queue: move the queue to one context, or put the contexts in different groups",
                        ),
                        Some(_) => {}
                        None => {
                            claims.queues.insert(name.clone(), ctx);
                        }
                    }
                }
                for handler in &service.handlers {
                    let HandlerKind::Http { method, path } = &handler.kind else {
                        continue;
                    };
                    match claims.routes.get(&(*method, path.clone())) {
                        Some(&earlier) if earlier != ctx => clash(
                            "bynk.deploy.route_conflict",
                            handler.span,
                            format!("declare the route `{} {path}`", method.as_str()),
                            earlier,
                            "a Worker answers a route with one handler: change one route, or put the contexts in different groups",
                        ),
                        Some(_) => {}
                        None => {
                            claims.routes.insert((*method, path.clone()), ctx);
                        }
                    }
                }
            }
        }
    }
}
