//! Durable event delivery D2 (#3898): the queue names Bynk reserves for event delivery.
//!
//! Every subscribing service gets a compiler-owned event queue (`bynk-ev-…`) and dead-letter
//! queue (`bynk-dl-…`), named by `bynk_project::environment::event_queue_name`. A queue that
//! user code names under either prefix could collide with one of them, or read or write event
//! traffic the compiler owns, so `bynk.queue.reserved_name` refuses it wherever a queue is
//! named: a `Queue @name("…")` instance, a `from queue("…")` consumer and a `deadLetter: "…"`.

use bynk_project::environment::reserved_queue_prefix;
use bynk_syntax::error::CompileError;
use bynk_syntax::span::Span;

/// `bynk.queue.reserved_name` for `name` at `span`, when it starts with a reserved prefix.
pub(crate) fn reserved_name_error(name: &str, span: Span) -> Option<CompileError> {
    let prefix = reserved_queue_prefix(name)?;
    Some(
        CompileError::new(
            "bynk.queue.reserved_name",
            span,
            format!("`{name}` starts with `{prefix}`, which Bynk reserves for its event queues"),
        )
        .with_note(
            "the compiler creates and names each event subscriber's queue (`bynk-ev-`) and dead-letter queue (`bynk-dl-`); give this queue a name outside both prefixes",
        ),
    )
}
