//! `bynk dev --trace-events` (#3580): the per-request trace a traced Worker
//! prints, and the `request` event `bynk dev` makes of it.
//!
//! A traced Worker prints one `__bynk_trace:` line per request (the `trace`
//! runtime module). The driver recognises it before the frame rewriter sees the
//! line, parses it, maps the fault's stack frames to `.bynk` locations, and
//! emits a [`crate::dev::DevEvent::Request`]. A line that carries the prefix but
//! does not parse is left to the ordinary output, so nothing is lost.

use serde::{Deserialize, Serialize};

/// The prefix a traced Worker puts on its trace line.
pub const TRACE_PREFIX: &str = "__bynk_trace:";

/// The `.bynk` call site a step came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceSpan {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

/// One recorded step of a request: what it called and how it went. It carries no
/// argument values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraceStep {
    /// `capability`, `call` (a cross-context call) or `agent`.
    pub kind: String,
    /// What was called: `Db.query`, `shop.payment.authorise`, `Ledger.tally`.
    pub target: String,
    pub span: Option<TraceSpan>,
    /// When the step began, in milliseconds after its request began, on the clock of the request's
    /// `duration_ms`: `[start_ms, start_ms + ms)` places the step on the request's timeline, so steps
    /// that overlap ran concurrently and the gap between steps is the handler's own time (#3787).
    /// Absent from a trace an older runtime printed, which still reads: the field is optional on the
    /// wire. A callee's own trace is measured from the callee's start, not the caller's: the client
    /// places it at its caller's `call` step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_ms: Option<f64>,
    pub ms: f64,
    /// `ok`, `fault`, or `pending` (not settled when the response was ready).
    pub outcome: String,
    /// A `call` step: the id of the callee's own request, so a client can join
    /// this step to the callee's `request` event. Absent on other steps.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
}

/// A request's fault: the error's first line and its stack frames, with frames
/// into generated code rewritten to `.bynk` locations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TraceFault {
    pub message: String,
    pub frames: Vec<String>,
}

/// The line as the Worker printed it.
#[derive(Debug, Deserialize)]
pub struct RawTrace {
    pub id: String,
    /// The calling request's id, when a cross-context call carried one.
    #[serde(default)]
    pub parent: Option<String>,
    pub route: String,
    pub status: u16,
    pub duration_ms: f64,
    pub steps: Vec<TraceStep>,
    pub truncated: bool,
    pub fault: Option<String>,
}

/// The trace JSON on `line`, when it carries the prefix (anywhere in the line, as
/// a host may put its own text before it).
pub fn trace_json(line: &str) -> Option<&str> {
    line.find(TRACE_PREFIX)
        .map(|at| line[at + TRACE_PREFIX.len()..].trim_end())
}

/// Parse a trace line, or `None` when it is not one or is malformed.
pub fn parse(line: &str) -> Option<RawTrace> {
    serde_json::from_str(trace_json(line)?).ok()
}

/// A fault's stack as a [`TraceFault`], each frame passed through `rewrite`.
pub fn fault(stack: &str, rewrite: impl Fn(&str) -> String) -> TraceFault {
    let mut lines = stack.lines();
    let message = lines.next().unwrap_or("").to_string();
    TraceFault {
        message,
        frames: lines.map(|l| rewrite(l).trim().to_string()).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINE: &str = r#"__bynk_trace:{"context":"shop.orders","id":"r1","parent":"p0","route":"POST /orders","status":500,"duration_ms":12.5,"steps":[{"kind":"call","target":"shop.payment.authorise","span":{"file":"src/shop/orders.bynk","line":23,"col":14},"start_ms":1,"ms":3.1,"outcome":"fault","request":"c1"},{"kind":"agent","target":"Ledger.tally","span":null,"start_ms":1.2,"ms":0,"outcome":"pending"}],"truncated":false,"fault":"Error: boom\n    at f (index.js:1:2)"}"#;
    /// What a runtime from before #3787 printed: no `start_ms` on a step.
    const OLD_LINE: &str = r#"__bynk_trace:{"context":"shop.orders","id":"r1","parent":null,"route":"GET /","status":200,"duration_ms":4,"steps":[{"kind":"capability","target":"Db.query","span":null,"ms":2.5,"outcome":"ok"}],"truncated":false,"fault":null}"#;

    #[test]
    fn a_trace_line_parses() {
        let t = parse(LINE).expect("parses");
        assert_eq!(t.route, "POST /orders");
        assert_eq!(t.parent.as_deref(), Some("p0"));
        assert_eq!(t.steps[0].request.as_deref(), Some("c1"));
        assert_eq!(t.steps[1].request, None);
        assert_eq!(t.status, 500);
        assert_eq!(t.steps.len(), 2);
        assert_eq!(t.steps[0].span.as_ref().unwrap().line, 23);
        assert_eq!(t.steps[1].span, None);
    }

    #[test]
    fn a_step_carries_its_start_offset() {
        let t = parse(LINE).expect("parses");
        assert_eq!(t.steps[0].start_ms, Some(1.0));
        assert_eq!(t.steps[1].start_ms, Some(1.2));
        // The two overlap: the agent step began inside the call step.
        let (a, b) = (&t.steps[0], &t.steps[1]);
        assert!(b.start_ms.unwrap() < a.start_ms.unwrap() + a.ms);
    }

    /// A trace an older runtime printed has no `start_ms`; it still parses, and says nothing
    /// about start times rather than inventing one.
    #[test]
    fn a_trace_without_start_offsets_still_reads() {
        let t = parse(OLD_LINE).expect("an older trace parses");
        assert_eq!(t.steps[0].start_ms, None);
        assert_eq!(t.steps[0].ms, 2.5);
        let json = serde_json::to_string(&t.steps[0]).unwrap();
        assert!(
            !json.contains("start_ms"),
            "an absent offset is not written as null: {json}"
        );
    }

    #[test]
    fn host_text_before_the_prefix_is_tolerated() {
        assert!(parse(&format!("[wrangler:inf] {LINE}")).is_some());
    }

    #[test]
    fn other_lines_and_malformed_traces_are_not_traces() {
        assert!(parse("hello").is_none());
        assert!(parse("__bynk_trace:{not json").is_none());
        assert!(trace_json("hello").is_none());
    }

    #[test]
    fn a_fault_keeps_its_message_and_rewrites_each_frame() {
        let f = fault(
            "Error: boom\n    at f (index.js:1:2)\n    at g (index.js:3:4)",
            |l| l.replace("index.js:1:2", "src/a.bynk:9:1"),
        );
        assert_eq!(f.message, "Error: boom");
        assert_eq!(f.frames, ["at f (src/a.bynk:9:1)", "at g (index.js:3:4)"]);
    }
}
