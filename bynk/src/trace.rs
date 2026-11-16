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
    pub ms: f64,
    /// `ok`, `fault`, or `pending` (not settled when the response was ready).
    pub outcome: String,
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

    const LINE: &str = r#"__bynk_trace:{"context":"shop.orders","id":"r1","route":"POST /orders","status":500,"duration_ms":12.5,"steps":[{"kind":"call","target":"shop.payment.authorise","span":{"file":"src/shop/orders.bynk","line":23,"col":14},"ms":3.1,"outcome":"fault"},{"kind":"agent","target":"Ledger.tally","span":null,"ms":0,"outcome":"pending"}],"truncated":false,"fault":"Error: boom\n    at f (index.js:1:2)"}"#;

    #[test]
    fn a_trace_line_parses() {
        let t = parse(LINE).expect("parses");
        assert_eq!(t.route, "POST /orders");
        assert_eq!(t.status, 500);
        assert_eq!(t.steps.len(), 2);
        assert_eq!(t.steps[0].span.as_ref().unwrap().line, 23);
        assert_eq!(t.steps[1].span, None);
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
