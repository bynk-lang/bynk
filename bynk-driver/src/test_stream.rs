//! `bynk test --format ndjson` (#2267, Slice D stage 1): the supported result
//! stream.
//!
//! The runner already writes one JSON event per line (`BYNK_TEST_FORMAT=ndjson`),
//! an internal protocol between the generated entry and the driver. `--format
//! json` collects all of it and prints one document at the end; `--format ndjson`
//! forwards it as it arrives, so a consumer sees each suite when that suite
//! finishes. The events the driver prints are its own, not the runner's lines
//! passed through, so the contract does not depend on the runner's wording:
//!
//! - `run-begin {suites}`
//! - `suite-begin {name, kind, tests}`
//! - `case {suite, <the document's case fields>}`: the same object the JSON
//!   document holds for the case, in the same field order, behind `type` and
//!   `suite`, so it carries `tier` and the generative fields too
//! - `suite-end {name}`
//! - `run-end {passed, failed, skipped?, transport?}`
//! - `error {kind, message?, diagnostics?, stderr?}`: the document's `error`
//!   object, for a compile failure, a run the driver could not start or finish,
//!   and a runner that died before `run-end`
//!
//! The stream ends with exactly one of `run-end` or `error`. A case a remote run
//! skipped (`--skip-unsupported`) is a `case` event with outcome `skipped`
//! between the last `suite-end` and `run-end`. A line that is not a recognised
//! runner event is dropped. Field order is the contract; a consumer ignores
//! event types and fields it does not know.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, ExitStatus, Stdio};

use serde::Serialize;

use crate::test_json::{
    Case, TestError, TestRun, Unmatched, case_from_event, unmatched_from_event,
};

#[derive(Serialize)]
struct RunBegin {
    r#type: &'static str,
    suites: usize,
}

#[derive(Serialize)]
struct SuiteBegin<'a> {
    r#type: &'static str,
    name: &'a str,
    kind: &'a str,
    tests: usize,
}

#[derive(Serialize)]
struct CaseEvent<'a> {
    r#type: &'static str,
    suite: &'a str,
    #[serde(flatten)]
    case: &'a Case,
}

#[derive(Serialize)]
struct SuiteEnd<'a> {
    r#type: &'static str,
    name: &'a str,
}

#[derive(Serialize)]
struct RunEnd<'a> {
    r#type: &'static str,
    passed: u32,
    failed: u32,
    #[serde(skip_serializing_if = "is_zero")]
    skipped: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    transport: Option<&'a str>,
}

#[derive(Serialize)]
struct ErrorEvent<'a> {
    r#type: &'static str,
    #[serde(flatten)]
    error: &'a TestError,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn line<T: Serialize>(event: &T) -> String {
    // The structs hold only strings and numbers, which always serialise.
    let mut text = serde_json::to_string(event).unwrap_or_default();
    text.push('\n');
    text
}

fn run_begin(suites: usize) -> String {
    line(&RunBegin {
        r#type: "run-begin",
        suites,
    })
}

fn suite_begin(name: &str, kind: &str, tests: usize) -> String {
    line(&SuiteBegin {
        r#type: "suite-begin",
        name,
        kind,
        tests,
    })
}

fn case_event(suite: &str, case: &Case) -> String {
    line(&CaseEvent {
        r#type: "case",
        suite,
        case,
    })
}

fn suite_end(name: &str) -> String {
    line(&SuiteEnd {
        r#type: "suite-end",
        name,
    })
}

fn run_end(passed: u32, failed: u32, skipped: u32, transport: Option<&str>) -> String {
    line(&RunEnd {
        r#type: "run-end",
        passed,
        failed,
        skipped,
        transport,
    })
}

fn error_event(error: &TestError) -> String {
    line(&ErrorEvent {
        r#type: "error",
        error,
    })
}

/// The stream for a document held in full: a run that could not start (a
/// compile failure, a launch error) is one `error` event, and a normal one is
/// its suites' events then `run-end`. Used where nothing is read live.
pub fn events_for(run: &TestRun) -> String {
    let mut out = String::new();
    let Some(suites) = &run.suites else {
        if let Some(error) = &run.error {
            out.push_str(&error_event(error));
        }
        return out;
    };
    out.push_str(&run_begin(suites.len()));
    for suite in suites {
        out.push_str(&suite_begin(&suite.name, &suite.kind, suite.cases.len()));
        for case in suite.cases.iter().filter(|c| c.outcome != "skipped") {
            out.push_str(&case_event(&suite.name, case));
        }
        out.push_str(&suite_end(&suite.name));
    }
    for suite in suites {
        for case in suite.cases.iter().filter(|c| c.outcome == "skipped") {
            out.push_str(&case_event(&suite.name, case));
        }
    }
    match &run.error {
        Some(error) => out.push_str(&error_event(error)),
        None => out.push_str(&run_end(
            run.passed,
            run.failed,
            run.skipped,
            run.transport.as_deref(),
        )),
    }
    out
}

/// Print a document in `--format json`, or its events in `--format ndjson`.
pub fn print_run(stream: bool, run: &TestRun) {
    if stream {
        print!("{}", events_for(run));
    } else {
        print!("{}", run.render());
    }
}

/// A remote run's skipped case, as it goes into the stream: suite, case, reason.
pub type Skipped = (String, String, String);

/// What the stream knows about the run besides the runner's own events.
pub struct StreamContext<'a> {
    /// The compile's case table, for each case's tier.
    pub discovered: &'a [bynk_emit::project::DiscoveredSuite],
    /// The cases a remote run skipped, which the runner never saw.
    pub skipped: Vec<Skipped>,
    /// `remote:<target>` under `--against`.
    pub transport: Option<String>,
}

/// Turns the runner's event lines into the stream's, one at a time.
pub struct Stream<'a> {
    context: StreamContext<'a>,
    ended: bool,
    unmatched: Unmatched,
}

impl<'a> Stream<'a> {
    pub fn new(context: StreamContext<'a>) -> Self {
        Self {
            context,
            ended: false,
            unmatched: Unmatched::default(),
        }
    }

    /// The `--suite`/`--case` values the runner reported as selecting nothing:
    /// not part of the stream, but the driver turns them into an error (#2273).
    pub fn unmatched(&self) -> &Unmatched {
        &self.unmatched
    }

    fn tier_of(&self, suite: &str, case: &str) -> Option<&'static str> {
        self.context
            .discovered
            .iter()
            .find(|d| d.name == suite)?
            .cases
            .iter()
            .find(|c| c.name == case)?
            .tier
    }

    /// The stream's text for one line of the runner's output: nothing for a line
    /// that is not a recognised event.
    pub fn push(&mut self, raw: &str) -> String {
        let raw = raw.trim();
        let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
            return String::new();
        };
        let text = |key: &str| value.get(key).and_then(|v| v.as_str()).unwrap_or_default();
        let count = |key: &str| {
            value
                .get(key)
                .and_then(serde_json::Value::as_u64)
                .map_or(0, |n| usize::try_from(n).unwrap_or(usize::MAX))
        };
        match text("type") {
            "run-begin" => run_begin(count("suites")),
            "suite-begin" => suite_begin(text("name"), text("kind"), count("tests")),
            "case" => {
                let mut case = case_from_event(&value);
                case.tier = self.tier_of(text("suite"), &case.name).map(str::to_string);
                case_event(text("suite"), &case)
            }
            "suite-end" => suite_end(text("name")),
            "run-end" => {
                self.ended = true;
                self.unmatched = unmatched_from_event(&value);
                let mut out = String::new();
                for (suite, name, reason) in &self.context.skipped {
                    out.push_str(&case_event(
                        suite,
                        &Case {
                            name: name.clone(),
                            outcome: "skipped".to_string(),
                            message: Some(reason.clone()),
                            location: None,
                            retries: None,
                            tier: self.tier_of(suite, name).map(str::to_string),
                            kind: None,
                            seed: None,
                            counterexample: None,
                        },
                    ));
                }
                let passed = u32::try_from(count("passed")).unwrap_or(u32::MAX);
                let failed = u32::try_from(count("failed")).unwrap_or(u32::MAX);
                let skipped = u32::try_from(self.context.skipped.len()).unwrap_or(u32::MAX);
                out.push_str(&run_end(
                    passed,
                    failed,
                    skipped,
                    self.context.transport.as_deref(),
                ));
                out
            }
            _ => String::new(),
        }
    }

    /// What closes the stream when the runner's output has ended: nothing after
    /// a `run-end`, else an `error` event, the runner having died mid-run.
    pub fn finish(&self, stderr: &str) -> String {
        if self.ended {
            return String::new();
        }
        let trimmed = stderr.trim();
        error_event(&TestError {
            kind: "runtime".to_string(),
            message: Some("the test runner exited before completing".to_string()),
            diagnostics: Vec::new(),
            stderr: (!trimmed.is_empty()).then(|| trimmed.to_string()),
        })
    }
}

/// Run `cmd`, forwarding each event to `out` as the runner writes it, and return
/// its exit status. The runner's stderr is collected on a thread, so a full pipe
/// cannot stall it. If `out` stops accepting output (a consumer that went away),
/// the runner is killed rather than left running.
pub fn forward(
    mut cmd: Command,
    stream: &mut Stream<'_>,
    out: &mut impl Write,
) -> std::io::Result<ExitStatus> {
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()?;
    let stderr_pipe = child.stderr.take();
    let collector = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stderr_pipe {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                text.push_str(&line);
                text.push('\n');
            }
        }
        text
    });
    let mut gone = false;
    if let Some(pipe) = child.stdout.take() {
        // Lossy, so a stray byte in a line cannot end the stream early.
        let mut reader = BufReader::new(pipe);
        let mut buffer = Vec::new();
        loop {
            buffer.clear();
            match reader.read_until(b'\n', &mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let text = stream.push(&String::from_utf8_lossy(&buffer));
            if !text.is_empty() && write_and_flush(out, &text).is_err() {
                gone = true;
                break;
            }
        }
    }
    if gone {
        kill(&mut child);
    }
    let status = child.wait()?;
    let stderr = collector.join().unwrap_or_default();
    if !gone {
        let closing = stream.finish(&stderr);
        if !closing.is_empty() {
            let _ = write_and_flush(out, &closing);
        }
    }
    Ok(status)
}

fn write_and_flush(out: &mut impl Write, text: &str) -> std::io::Result<()> {
    out.write_all(text.as_bytes())?;
    out.flush()
}

fn kill(child: &mut Child) {
    let _ = child.kill();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_json::{Location, Suite};

    fn context<'a>() -> StreamContext<'a> {
        StreamContext {
            discovered: &[],
            skipped: Vec::new(),
            transport: None,
        }
    }

    fn case(name: &str, outcome: &str) -> Case {
        Case {
            name: name.to_string(),
            outcome: outcome.to_string(),
            message: None,
            location: None,
            retries: None,
            tier: None,
            kind: None,
            seed: None,
            counterexample: None,
        }
    }

    #[test]
    fn a_case_event_is_the_documents_case_behind_type_and_suite() {
        let mut failed = case("never seventy three", "fail");
        failed.message = Some("property failed".to_string());
        failed.location = Some(Location {
            path: "tests/m.test.bynk".to_string(),
            line: 3,
            col: 4,
        });
        failed.tier = Some("unit".to_string());
        failed.kind = Some("property".to_string());
        failed.seed = Some("0x5f3a".to_string());
        failed.counterexample = Some("p = 73".to_string());
        assert_eq!(
            case_event("demo.m", &failed),
            concat!(
                r#"{"type":"case","suite":"demo.m","name":"never seventy three","outcome":"fail","#,
                r#""message":"property failed","location":{"path":"tests/m.test.bynk","line":3,"col":4},"#,
                r#""tier":"unit","kind":"property","seed":"0x5f3a","counterexample":"p = 73"}"#,
                "\n"
            )
        );
    }

    #[test]
    fn the_runners_events_become_the_streams_with_tiers() {
        let discovered = [bynk_emit::project::DiscoveredSuite {
            name: "demo.m".to_string(),
            kind: "unit",
            cases: vec![bynk_emit::project::DiscoveredCase {
                name: "one".to_string(),
                location: None,
                tier: Some("unit"),
                generative: None,
            }],
            skipped: Vec::new(),
            envs: Vec::new(),
        }];
        let mut stream = Stream::new(StreamContext {
            discovered: &discovered,
            ..context()
        });
        let out: String = [
            r#"{"type":"run-begin","suites":1}"#,
            r#"{"type":"suite-begin","name":"demo.m","kind":"unit","tests":1}"#,
            r#"{"type":"case","suite":"demo.m","name":"one","outcome":"pass"}"#,
            r#"{"type":"suite-end","name":"demo.m"}"#,
            "not an event",
            r#"{"type":"something-new","x":1}"#,
            r#"{"type":"run-end","passed":1,"failed":0,"unmatched":{"cases":["x"]}}"#,
        ]
        .iter()
        .map(|l| stream.push(l))
        .collect();
        assert_eq!(
            out,
            concat!(
                "{\"type\":\"run-begin\",\"suites\":1}\n",
                "{\"type\":\"suite-begin\",\"name\":\"demo.m\",\"kind\":\"unit\",\"tests\":1}\n",
                "{\"type\":\"case\",\"suite\":\"demo.m\",\"name\":\"one\",\"outcome\":\"pass\",\"tier\":\"unit\"}\n",
                "{\"type\":\"suite-end\",\"name\":\"demo.m\"}\n",
                "{\"type\":\"run-end\",\"passed\":1,\"failed\":0}\n",
            ),
            "an unrecognised line is dropped, and `unmatched` is not part of the stream"
        );
        assert_eq!(stream.finish(""), "", "nothing closes a stream that ended");
    }

    #[test]
    fn skipped_cases_and_the_transport_close_a_remote_run() {
        let mut stream = Stream::new(StreamContext {
            skipped: vec![(
                "demo.s".to_string(),
                "advances".to_string(),
                "advance Clock".to_string(),
            )],
            transport: Some("remote:staging".to_string()),
            ..context()
        });
        assert_eq!(
            stream.push(r#"{"type":"run-end","passed":2,"failed":0}"#),
            concat!(
                "{\"type\":\"case\",\"suite\":\"demo.s\",\"name\":\"advances\",\"outcome\":\"skipped\",\"message\":\"advance Clock\"}\n",
                "{\"type\":\"run-end\",\"passed\":2,\"failed\":0,\"skipped\":1,\"transport\":\"remote:staging\"}\n",
            )
        );
    }

    #[test]
    fn a_runner_that_died_is_an_error_event() {
        let mut stream = Stream::new(context());
        stream.push(r#"{"type":"run-begin","suites":1}"#);
        assert_eq!(
            stream.finish("boom\n"),
            "{\"type\":\"error\",\"kind\":\"runtime\",\"message\":\"the test runner exited before completing\",\"stderr\":\"boom\"}\n"
        );
    }

    #[test]
    fn a_document_held_in_full_becomes_its_events() {
        let compile = TestRun::compile_error(vec!["a.bynk:1:1: error[x]: no".to_string()]);
        assert_eq!(
            events_for(&compile),
            "{\"type\":\"error\",\"kind\":\"compile\",\"diagnostics\":[\"a.bynk:1:1: error[x]: no\"]}\n"
        );

        let mut run = TestRun::empty();
        run.suites = Some(vec![Suite {
            name: "demo.m".to_string(),
            kind: "unit".to_string(),
            cases: vec![case("one", "pass"), case("two", "skipped")],
        }]);
        run.passed = 1;
        run.skipped = 1;
        assert_eq!(
            events_for(&run),
            concat!(
                "{\"type\":\"run-begin\",\"suites\":1}\n",
                "{\"type\":\"suite-begin\",\"name\":\"demo.m\",\"kind\":\"unit\",\"tests\":2}\n",
                "{\"type\":\"case\",\"suite\":\"demo.m\",\"name\":\"one\",\"outcome\":\"pass\"}\n",
                "{\"type\":\"suite-end\",\"name\":\"demo.m\"}\n",
                "{\"type\":\"case\",\"suite\":\"demo.m\",\"name\":\"two\",\"outcome\":\"skipped\"}\n",
                "{\"type\":\"run-end\",\"passed\":1,\"failed\":0,\"skipped\":1}\n",
            )
        );
    }

    /// A sink that records when each write arrived.
    struct Timed(Vec<(std::time::Instant, String)>);

    impl Write for Timed {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.push((
                std::time::Instant::now(),
                String::from_utf8_lossy(buf).into_owned(),
            ));
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// An event reaches the consumer while the runner is still running, not when
    /// it exits; an event split across two writes is still one event; a runner
    /// that never says `run-end` ends the stream with an `error`.
    #[cfg(unix)]
    #[test]
    fn events_are_forwarded_as_they_are_written() {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg(
            r#"printf '{"type":"run-begin","suites":1}\n{"type":"suite-be'; sleep 0.4;
               printf 'gin","name":"s","kind":"unit","tests":0}\n'; sleep 0.4; echo oops >&2"#,
        );
        let mut stream = Stream::new(context());
        let mut sink = Timed(Vec::new());
        let started = std::time::Instant::now();
        let status = forward(cmd, &mut stream, &mut sink).unwrap();
        let finished = started.elapsed();
        assert!(status.success());
        let first = sink.0.first().expect("an event");
        assert_eq!(first.1, "{\"type\":\"run-begin\",\"suites\":1}\n");
        assert!(
            first.0.duration_since(started) < finished - std::time::Duration::from_millis(500),
            "the first event arrived well before the runner exited"
        );
        let all: String = sink.0.iter().map(|(_, t)| t.as_str()).collect();
        assert_eq!(
            all,
            concat!(
                "{\"type\":\"run-begin\",\"suites\":1}\n",
                "{\"type\":\"suite-begin\",\"name\":\"s\",\"kind\":\"unit\",\"tests\":0}\n",
                "{\"type\":\"error\",\"kind\":\"runtime\",\"message\":\"the test runner exited before completing\",\"stderr\":\"oops\"}\n",
            ),
            "a split event is one event, and the missing run-end is an error"
        );
    }
}
