//! The text-exit counters on the driver path (1A.4's `TEXT_EXITS`, 1A.8's
//! `TEXT_STMTS`).
//!
//! `bynk-emit`'s own tests count text exits with `cfg(test)` thread-locals, which
//! exist only in that crate's own test build. A project lowered through
//! `bynk_driver` runs the driver's non-test copy of the emitter, so a counter
//! read from inside `bynk-emit`'s tests is 0 whatever the lowering did, and a
//! `== 0` assertion there proves nothing. This file reads the counters where
//! they are real: `bynk-tests` enables `bynk-emit`'s `text-exit-counters`
//! feature, which compiles them into the copy the driver links.

use bynk_check::project_model::BuildTarget;
use bynk_emit::emitter::text_counters;

/// Emit the one-context project `src` for Workers and return `(text exits, text
/// statements, every emitted document's text)`.
fn workers_counts(src: &str) -> (usize, usize, String) {
    text_counters::reset();
    let out = bynk_driver::project::compile_files_in_memory(
        &[("ops/jobs.bynk", src)],
        BuildTarget::Workers,
        Default::default(),
        true,
    )
    .unwrap_or_else(|_| panic!("fixture should compile:\n{src}"));
    let text: String = out
        .artefacts
        .docs
        .values()
        .map(|d| d.text())
        .collect::<Vec<_>>()
        .join("\n");
    (text_counters::exits(), text_counters::stmts(), text)
}

fn queue_service(body: &str) -> String {
    format!(
        "context ops.jobs\n\nservice inbox from queue(\"jobs\") {{\n\ton message(n: Int) -> Effect[QueueResult] {{\n\t\t{body}\n\t}}\n}}\n"
    )
}

/// The counter is live on this path: an expression that stays text (a string
/// `contains`, a method-call family) is counted. Without this, the `==` below could pass
/// because nothing is counted at all.
#[test]
fn the_counter_counts_on_the_driver_path() {
    let src = "context ops.jobs\n\nservice inbox from queue(\"jobs\") {\n\ton message(n: Int) -> Effect[QueueResult] {\n\t\tlet ok = \"a\".splitRegex(\",\").length() > 0\n\t\tAck\n\t}\n}\n";
    let (exits, _, text) = workers_counts(src);
    assert!(exits > 0, "no text exit counted:\n{text}");
}

/// `Retry("x")` instead of `Ack` adds no text exit and prints as the
/// `QueueResult.Retry(...)` call. (Moved from `bynk-emit`'s `lower` tests, where
/// the counts it compared were both 0.)
#[test]
fn a_queue_result_variant_call_is_a_node() {
    let (ack, _, _) = workers_counts(&queue_service("Ack"));
    let (retry, _, text) = workers_counts(&queue_service("Retry(\"x\")"));
    assert_eq!(retry, ack);
    assert!(text.contains("QueueResult.Retry(\"x\")"), "{text}");
}
