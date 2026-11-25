//! #3895: every entry point runs inside a trace.
//!
//! The Worker's `fetch`, `scheduled`, `queue` and `email` each move their body
//! into `__enterTrace(async () => { ... })`, which starts a trace (the platform's
//! own id when it gives one, otherwise a random one) and holds it in the request
//! scope for everything the entry awaits. A signed call or event delivery is also
//! an entry: `__verifyCall` replaces its trace with the one the caller carried,
//! after the signature has held, so a public request, which never reaches
//! `__verifyCall`, can never choose one.
//!
//! Where the platform has no request scope (a Worker without `nodejs_als`), the
//! runtime runs the body as it is and nothing is carried.

use crate::ts_build::{block_arrow, call, ident, return_};
use bynk_ts::{TsExpr, TsObjectEntry};

/// The entry points a request, a cron run, a queue batch or an email reaches.
const ENTRY_POINTS: &[&str] = &["fetch", "scheduled", "queue", "email"];

/// The Worker's default export: its entries, each entry point run under a trace.
pub(crate) fn export_default(entries: Vec<TsObjectEntry>) -> TsExpr {
    TsExpr::multiline_object_entries(enter_trace(entries))
}

/// `entries` with each entry point's body run under a new trace.
pub(crate) fn enter_trace(entries: Vec<TsObjectEntry>) -> Vec<TsObjectEntry> {
    entries
        .into_iter()
        .map(|entry| match entry {
            TsObjectEntry::Method {
                name,
                is_async,
                generics,
                params,
                return_type,
                doc,
                inline: _,
                body,
                span,
            } if ENTRY_POINTS.contains(&name.as_str()) => TsObjectEntry::Method {
                name,
                is_async,
                generics,
                params,
                return_type,
                doc,
                inline: false,
                body: vec![return_(Some(call(
                    ident("__enterTrace"),
                    vec![block_arrow(Vec::new(), true, body)],
                )))],
                span,
            },
            other @ (TsObjectEntry::Prop { .. }
            | TsObjectEntry::Shorthand(_)
            | TsObjectEntry::Method { .. }
            | TsObjectEntry::Spread(_)) => other,
        })
        .collect()
}
