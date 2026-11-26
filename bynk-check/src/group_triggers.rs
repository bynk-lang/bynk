//! What a Worker answers, when several contexts share it (#3562, G2b step 2c).
//!
//! A `[deploy.workers.<key>]` group is one Worker, with one `fetch`, one `queue`
//! and one `email` entry, so each thing it is routed by must belong to one
//! member. These are the rules a single context already keeps among its own
//! services (`bynk.http.duplicate_route`), kept across the members of a group,
//! with the same keys:
//!
//! - an HTTP route is its method and path.
//!
//! An email address and a queue are not checked here: a project already refuses
//! two contexts claiming one address (`bynk.email.duplicate_address`) or
//! consuming one queue (`bynk.queue.duplicate_consumer`), grouped or not.
//!
//! A cron expression is not one of these: two members may share it, and both
//! their handlers run.
//!
//! One error per clash, at the later member's declaration, naming both members.

use std::collections::{BTreeMap, HashMap};

use bynk_project::ParsedFile;
use bynk_syntax::ast::{CommonsItem, HandlerKind, HttpMethod, SourceUnit};
use bynk_syntax::error::CompileError;

use crate::project_model::ErrorSink;

/// What one member claimed first, by key: the member, so a later member's
/// clash can name it.
#[derive(Default)]
struct Claims<'a> {
    routes: HashMap<(HttpMethod, String), &'a str>,
}

/// One HTTP route of a group member, for the whole-group reachability check.
struct GroupRoute<'a> {
    ctx: &'a str,
    method: HttpMethod,
    path: String,
    span: bynk_syntax::span::Span,
    file: std::path::PathBuf,
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
    let mut all_routes: Vec<GroupRoute> = Vec::new();
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
                for handler in &service.handlers {
                    let HandlerKind::Http { method, path } = &handler.kind else {
                        continue;
                    };
                    all_routes.push(GroupRoute {
                        ctx: ctx.as_str(),
                        method: *method,
                        path: path.clone(),
                        span: handler.span,
                        file: parsed[i].identity_path(),
                    });
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
    unreachable_across_members(key, &all_routes, errors);
}

/// #3909: a route one member declares that the routes of the whole group answer
/// first. One that its own context's routes already shadow is that context's
/// `bynk.http.unreachable_route`, reported with the context.
fn unreachable_across_members(key: &str, routes: &[GroupRoute], errors: &mut ErrorSink) {
    let keys: Vec<(&str, &str)> = routes
        .iter()
        .map(|r| (r.method.as_str(), r.path.as_str()))
        .collect();
    for (r, covering) in crate::route_order::shadowed(&keys) {
        let own: Vec<usize> = (0..routes.len())
            .filter(|&i| routes[i].ctx == routes[r].ctx)
            .collect();
        let own_keys: Vec<(&str, &str)> = own.iter().map(|&i| keys[i]).collect();
        let own_pos = own.iter().position(|&i| i == r);
        if crate::route_order::shadowed(&own_keys)
            .iter()
            .any(|(j, _)| Some(*j) == own_pos)
        {
            continue;
        }
        let route = &routes[r];
        let mut err = CompileError::new(
            "bynk.http.unreachable_route",
            route.span,
            format!(
                "route `{} {}` can never be reached: every path it matches is answered first by another route of Worker `{key}`",
                route.method.as_str(),
                route.path
            ),
        )
        .with_note(
            "contexts that share a Worker share one router: at the first segment where two routes differ, a literal segment beats a `:name` one",
        );
        for c in covering {
            let other = &routes[c];
            if other.file == route.file {
                err = err.with_label(
                    other.span,
                    format!(
                        "`{} {}` answers it first",
                        other.method.as_str(),
                        other.path
                    ),
                );
            } else {
                err = err.with_note(format!(
                    "`{} {}` of context `{}` answers it first",
                    other.method.as_str(),
                    other.path,
                    other.ctx
                ));
            }
        }
        errors.push_for(Some(&route.file), err);
    }
}
