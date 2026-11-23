use super::events::DevEvent;
use super::events::DevFormat;
use super::events::emit;
use super::run::DevOptions;
use crate::shell::exit_status_byte;
use bynk_driver::emsg;
use std::path::Path;
use std::time::Duration;

/// Whether `bynk dev` reads wrangler's output (#2474) rather than leaving it
/// attached to the terminal: when asked (`--rewrite-frames`, `--log-events`),
/// or when either of our streams is not a terminal, where wrangler has no
/// colour or hotkeys to lose.
pub(super) fn pipes_output(opts: &DevOptions) -> bool {
    use std::io::IsTerminal;
    opts.rewrite_frames
        || opts.log_events
        || opts.trace_events
        || !std::io::stdout().is_terminal()
        || !std::io::stderr().is_terminal()
}

/// Forward a spawned wrangler's stdout and stderr (#2474), a line at a time,
/// through the frame rewriter. Readable output goes where wrangler's own would
/// have: stdout to stdout, stderr to stderr, except under `--format json`
/// where stdout is the event stream and both go to stderr (#2094). With
/// `--log-events` each line is also a [`DevEvent::Log`]. A thread per stream,
/// ending at end of file; nothing is buffered beyond the line in hand.
pub(super) fn pump_output(
    child: &mut std::process::Child,
    worker: &str,
    rewriter: &std::sync::Arc<crate::logs::Rewriter>,
    opts: &DevOptions,
) -> Vec<std::thread::JoinHandle<()>> {
    let classifier = std::sync::Arc::new(std::sync::Mutex::new(crate::logs::Classifier::default()));
    let to_stderr_only = opts.format == DevFormat::Json;
    let events = opts.log_events && opts.format == DevFormat::Json;
    let trace = opts.trace_events && opts.format == DevFormat::Json;
    let streams: [(Option<Box<dyn std::io::Read + Send>>, bool); 2] = [
        (
            child.stdout.take().map(|s| Box::new(s) as Box<_>),
            !to_stderr_only,
        ),
        (child.stderr.take().map(|s| Box::new(s) as Box<_>), false),
    ];
    let mut handles = Vec::new();
    for (stream, to_stdout) in streams {
        let Some(stream) = stream else { continue };
        let (worker, rewriter, classifier) =
            (worker.to_string(), rewriter.clone(), classifier.clone());
        handles.push(std::thread::spawn(move || {
            use std::io::Write;
            let consume = |line: &str| trace && emit_request(&worker, &rewriter, line);
            forward_lines_with(
                stream,
                &worker,
                &rewriter,
                &classifier,
                consume,
                |line, level| {
                    if to_stdout {
                        let _ = writeln!(std::io::stdout().lock(), "{line}");
                    } else {
                        let _ = writeln!(std::io::stderr().lock(), "{line}");
                    }
                    if events {
                        emit(
                            DevFormat::Json,
                            &DevEvent::Log {
                                context: worker.clone(),
                                level,
                                text: crate::logs::strip_ansi(line),
                            },
                        );
                    }
                },
            );
        }));
    }
    handles
}

/// How long the end of a session waits for the output threads to drain.
pub(super) const DRAIN_GRACE: Duration = Duration::from_secs(2);

/// Wait for the output threads to reach end of file, so the lines that
/// explain why a wrangler exited are written before the session says it has
/// stopped. Bounded: a `workerd` a stopped wrangler left behind can hold its
/// pipe open, and that must not hang the exit.
pub(super) fn join_pumps(pumps: Vec<std::thread::JoinHandle<()>>, grace: Duration) {
    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline && !pumps.iter().all(|p| p.is_finished()) {
        std::thread::sleep(Duration::from_millis(10));
    }
    for pump in pumps {
        if pump.is_finished() {
            let _ = pump.join();
        }
    }
}

/// Read `stream` a line at a time until it ends, hand `sink` each line with
/// its frames rewritten and its level. The line classifier is shared by a
/// worker's two streams, so a frame on stderr still follows its headline.
pub(crate) fn forward_lines(
    stream: impl std::io::Read,
    worker: &str,
    rewriter: &crate::logs::Rewriter,
    classifier: &std::sync::Mutex<crate::logs::Classifier>,
    sink: impl FnMut(&str, crate::logs::LogLevel),
) {
    forward_lines_with(stream, worker, rewriter, classifier, |_| false, sink);
}

/// [`forward_lines`], with `consume` shown each line first, before the frame
/// rewriter touches it (#3580: a trace line's JSON must reach the parser as
/// printed). A line `consume` takes (answers `true`) is not forwarded.
pub(crate) fn forward_lines_with(
    stream: impl std::io::Read,
    worker: &str,
    rewriter: &crate::logs::Rewriter,
    classifier: &std::sync::Mutex<crate::logs::Classifier>,
    mut consume: impl FnMut(&str) -> bool,
    mut sink: impl FnMut(&str, crate::logs::LogLevel),
) {
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(stream);
    let mut raw = Vec::new();
    loop {
        raw.clear();
        match reader.read_until(b'\n', &mut raw) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let text = String::from_utf8_lossy(&raw);
        let plain = text.trim_end_matches(['\n', '\r']);
        if consume(plain) {
            continue;
        }
        let line = rewriter.rewrite(worker, plain);
        let level = classifier
            .lock()
            .map(|mut c| c.classify(&crate::logs::strip_ansi(&line)))
            .unwrap_or(crate::logs::LogLevel::Info);
        sink(&line, level);
    }
}

/// #3580: a trace line a worker printed as a [`DevEvent::Request`], with its
/// fault's frames mapped to `.bynk` locations, or `None` for a line that is not a
/// trace (or is malformed), which is then forwarded like any other.
fn request_event(worker: &str, rewriter: &crate::logs::Rewriter, line: &str) -> Option<DevEvent> {
    let t = crate::trace::parse(&crate::logs::strip_ansi(line))?;
    let fault = t
        .fault
        .as_deref()
        .map(|stack| crate::trace::fault(stack, |l| rewriter.rewrite(worker, l)));
    Some(DevEvent::Request {
        context: worker.to_string(),
        id: t.id,
        parent: t.parent,
        route: t.route,
        status: t.status,
        duration_ms: t.duration_ms,
        steps: t.steps,
        truncated: t.truncated,
        fault,
    })
}

/// Write the [`DevEvent::Request`] for `line`; `false` when it is not a trace.
fn emit_request(worker: &str, rewriter: &crate::logs::Rewriter, line: &str) -> bool {
    let Some(event) = request_event(worker, rewriter, line) else {
        return false;
    };
    emit(DevFormat::Json, &event);
    true
}

/// The watch loop: every 500ms, end the session if it was asked to stop or a
/// worker exited, and otherwise run `tick` (the rebuild check). Returns the
/// session's exit byte once every child is stopped and reaped.
pub(super) fn supervise(
    children: &mut Vec<(String, Served)>,
    workers_dir: &Path,
    mut tick: impl FnMut(),
) -> u8 {
    loop {
        // #1955: a SIGINT/SIGTERM recorded by `stop::install`. Checked before
        // the children, so a Ctrl-C — which reaches the wranglers too — reads
        // as the stop it is rather than as a worker exiting.
        if let Some(sig) = crate::stop::received() {
            emsg!(
                "bynk dev: received {} — stopping the contexts.",
                crate::stop::name(sig)
            );
            terminate(children, workers_dir);
            return crate::stop::exit_byte(sig);
        }
        // Any worker exiting ends the session: the survivors' bindings now
        // point at a context that is gone, so a half-served project would fail
        // in a way that looks like a code bug. Stop them and propagate the
        // first exit code.
        for i in 0..children.len() {
            let status = match children[i].1.child.try_wait() {
                Ok(status) => status,
                Err(e) => {
                    emsg!("bynk: error: could not poll wrangler: {e}");
                    terminate(children, workers_dir);
                    return 1;
                }
            };
            if let Some(status) = status {
                // Dropped here. On Windows that stops its job (#1762), and
                // with it the `workerd`s the exited wrangler left running.
                let (name, _) = children.remove(i);
                if !children.is_empty() {
                    emsg!("bynk dev: `{name}` exited — stopping the other contexts.");
                }
                terminate(children, workers_dir);
                return exit_status_byte(&status);
            }
        }
        std::thread::sleep(Duration::from_millis(500));
        tick();
    }
}

/// One `wrangler dev` that `bynk dev` is serving: the child it spawned and, on
/// Windows, the job object that holds the child's whole process tree (#1762).
pub(super) struct Served {
    child: std::process::Child,
    /// `None` when the child could not be put in a job. It is then stopped
    /// with `Child::kill` alone, as before #1762, which leaves its descendants
    /// running.
    #[cfg(windows)]
    job: Option<crate::job::Job>,
}

impl Served {
    pub(super) fn new(child: std::process::Child) -> Served {
        #[cfg(windows)]
        {
            let job = match crate::job::Job::assign(&child) {
                Ok(job) => Some(job),
                Err(e) => {
                    emsg!(
                        "bynk dev: warning: could not put wrangler in a job object ({e}); \
                         stopping it may leave its workerd processes running."
                    );
                    None
                }
            };
            Served { child, job }
        }
        #[cfg(not(windows))]
        Served { child }
    }
}

/// Stop every remaining `wrangler dev` and reap it, so a session that ends on
/// one worker's exit does not strand the others — each holds a port and a
/// `workerd` child, and a stranded one makes the *next* `bynk dev` fail on a
/// port clash. Signal them all first, then reap, so the shutdowns overlap.
///
/// Then [`sweep`](crate::sweep::sweep) what is still running under
/// `workers_dir` (#1742). Signalling the children is not enough: via npx the
/// child is `npx`, which does not pass the signal on, and a worker that exited
/// on its own has already orphaned its `workerd`s. On Windows the sweep does
/// nothing, and each child's job object stops its tree instead (#1762).
pub(super) fn terminate(children: &mut Vec<(String, Served)>, workers_dir: &Path) {
    for (_, served) in children.iter_mut() {
        request_stop(served);
    }
    for (_, served) in children.iter_mut() {
        reap(&mut served.child);
    }
    children.clear();
    crate::sweep::sweep(workers_dir, STOP_GRACE);
}

/// Ask one `wrangler dev` to stop **and take its own process tree with it**.
///
/// SIGTERM, not [`std::process::Child::kill`]'s SIGKILL: wrangler traps SIGTERM
/// and tears down the `node` and `workerd` processes it spawned, whereas SIGKILL
/// is untrappable — verified, a SIGKILLed wrangler strands an orphaned `workerd
/// serve` still holding the port. std exposes no SIGTERM, so we go through POSIX
/// `kill(1)`.
///
/// On Windows the child is a `cmd.exe` wrapper, and `Child::kill` stops only
/// that. Terminating the child's job object stops the whole tree instead
/// (#1762). It is a hard stop, but inside a job nothing is stranded by one.
fn request_stop(served: &mut Served) {
    #[cfg(windows)]
    if let Some(job) = &served.job {
        job.terminate();
        return;
    }
    let child = &mut served.child;
    #[cfg(unix)]
    {
        let sent = std::process::Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status()
            .is_ok_and(|s| s.success());
        if sent {
            return;
        }
        // `kill` missing or the process already gone — fall through.
    }
    let _ = child.kill();
}

/// How long a stopping wrangler gets to run its own teardown before SIGKILL.
const STOP_GRACE: Duration = Duration::from_secs(10);

/// Reap a signalled child, giving it a moment to run wrangler's own teardown
/// before escalating to SIGKILL. Without the escalation a wrangler wedged in
/// shutdown would hang `bynk dev` forever; without the grace period we would be
/// back to stranding `workerd`.
pub(super) fn reap(child: &mut std::process::Child) {
    const TICK: Duration = Duration::from_millis(50);
    let mut waited = Duration::ZERO;
    while waited < STOP_GRACE {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => {}
            Err(_) => break,
        }
        std::thread::sleep(TICK);
        waited += TICK;
    }
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dev::events::DevEvent;
    use crate::dev::run::DevOptions;

    use crate::shell::exit_status_byte;

    use std::time::Duration;

    #[test]
    fn exit_status_byte_maps_codes_and_signals() {
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            use std::process::ExitStatus;
            // Wait statuses: exit codes sit in the high byte; the low byte is
            // the terminating signal.
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(0)), 0);
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(1 << 8)), 1);
            // A shared Ctrl-C (SIGINT = 2) is a clean stop…
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(2)), 0);
            // …but a SIGSEGV (11) or SIGKILL (9, the OOM killer) is a real
            // failure — previously these read as passing in CI.
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(11)), 128 + 11);
            assert_eq!(exit_status_byte(&ExitStatus::from_raw(9)), 128 + 9);
        }
    }

    /// #1955: a SIGTERM sent to `bynk dev` alone stops its contexts and what
    /// they started. `sh` stands in for wrangler and its `sleep`s for the
    /// `workerd`s: the `sh` dies of the SIGTERM `terminate` sends and orphans
    /// them, as a wrangler resolved via npx does, and the sweep must stop them.
    #[cfg(unix)]
    #[test]
    fn a_sigterm_to_the_driver_stops_every_context_and_its_children() {
        use std::process::{Command, Stdio};
        // #2055: the deploy tests that change signal dispositions must not
        // restore the default while this one signals the process.
        let _lock = crate::stop::TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let root = std::env::temp_dir().join(format!("bynk-dev-sigterm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut children = Vec::new();
        for ctx in ["api", "worker"] {
            let dir = root.join(ctx);
            std::fs::create_dir_all(&dir).unwrap();
            let child = Command::new("sh")
                .args(["-c", "sleep 300 & sleep 300 & wait"])
                .current_dir(&dir)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("sh spawns");
            children.push((ctx.to_string(), Served::new(child)));
        }
        let started = std::time::Instant::now();
        while crate::sweep::stragglers(&root).len() < 6 {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the stand-ins never started their sleeps"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        // What a CI step's `kill -TERM <bynk dev>` does. Without the handler
        // this would end the test binary.
        crate::stop::install();
        let sent = Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .is_ok_and(|s| s.success());
        assert!(sent, "kill(1) runs");
        let started = std::time::Instant::now();
        while crate::stop::received().is_none() {
            assert!(
                started.elapsed() < Duration::from_secs(10),
                "the SIGTERM was never recorded"
            );
            std::thread::sleep(Duration::from_millis(10));
        }

        let code = supervise(&mut children, &root, || {
            panic!("a recorded stop must end the session before any rebuild")
        });
        crate::stop::reset();
        let left = crate::sweep::stragglers(&root);
        let _ = std::fs::remove_dir_all(&root);
        assert_eq!(code, 143, "a SIGTERM stop exits 128 + 15");
        assert!(children.is_empty());
        assert!(left.is_empty(), "processes survived the stop: {left:?}");
    }

    /// The flags always pipe, wherever the tests run; without them the answer
    /// is the terminal's, which a test cannot choose.
    #[test]
    fn the_rewrite_flags_pipe_wrangler_s_output() {
        let flagged = |rewrite_frames, log_events| {
            pipes_output(&DevOptions {
                rewrite_frames,
                log_events,
                ..DevOptions::default()
            })
        };
        assert!(flagged(true, false));
        assert!(flagged(false, true));
    }

    /// The real path from a printed line to the event: colour codes and a host
    /// prefix are tolerated, and a fault's frames are rewritten through the
    /// worker's own source map to `.bynk` locations.
    #[test]
    fn a_printed_trace_becomes_a_request_event_with_its_frames_mapped() {
        let build = std::env::temp_dir().join(format!("bynk-trace-event-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&build);
        let worker = build.join("workers").join("shop");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::write(
            worker.join("handlers.ts.map"),
            r#"{"version":3,"sources":["/p/src/shop.bynk"],"mappings":";AASA"}"#,
        )
        .unwrap();
        let rewriter = crate::logs::Rewriter::new(&build);
        let stack = format!(
            "Error: boom\n    at http_GET (file://{}/handlers.ts:2:5)",
            worker.display()
        );
        // Built with `serde_json`, so a Windows path's backslashes are escaped.
        let json = serde_json::json!({
            "id": "r1", "parent": "p0", "route": "GET /", "status": 500, "duration_ms": 2.0,
            "steps": [], "truncated": false, "fault": stack,
        });
        let line = format!("\u{1b}[34m[wrangler:inf]\u{1b}[0m __bynk_trace:{json}");
        let event = request_event("shop", &rewriter, &line).expect("a trace line");
        let _ = std::fs::remove_dir_all(&build);
        let DevEvent::Request {
            context,
            id,
            parent,
            status,
            fault,
            ..
        } = event
        else {
            panic!("not a request event");
        };
        assert_eq!((context.as_str(), id.as_str(), status), ("shop", "r1", 500));
        assert_eq!(parent.as_deref(), Some("p0"));
        let fault = fault.expect("the fault is carried");
        assert_eq!(fault.message, "Error: boom");
        assert_eq!(fault.frames, ["at http_GET (/p/src/shop.bynk:10:1)"]);
        assert!(request_event("shop", &rewriter, "plain output").is_none());
    }

    /// A trace line is consumed before the frame rewriter and never forwarded;
    /// every other line, and a malformed trace, is forwarded as before.
    #[test]
    fn a_trace_line_is_consumed_and_other_lines_are_forwarded() {
        let dir = std::env::temp_dir().join("bynk_trace_pump");
        let rewriter = crate::logs::Rewriter::new(&dir);
        let classifier = std::sync::Mutex::new(crate::logs::Classifier::default());
        let input = "hello\n__bynk_trace:{\"id\":\"r\",\"route\":\"GET /\",\"status\":200,\"duration_ms\":1.0,\"steps\":[],\"truncated\":false,\"fault\":null}\n__bynk_trace:{broken\n";
        let (mut consumed, mut forwarded) = (0, Vec::new());
        forward_lines_with(
            input.as_bytes(),
            "shop",
            &rewriter,
            &classifier,
            |l| {
                let is_trace = crate::trace::parse(l).is_some();
                consumed += usize::from(is_trace);
                is_trace
            },
            |l, _| forwarded.push(l.to_string()),
        );
        assert_eq!(consumed, 1);
        assert_eq!(forwarded, ["hello", "__bynk_trace:{broken"]);
    }

    /// The path a handler error takes: wrangler prints a headline and frames
    /// into generated TypeScript; the driver's stream has them at `.bynk`.
    #[test]
    fn forwarded_lines_have_their_frames_rewritten_and_a_level() {
        let build = std::env::temp_dir().join(format!("bynk-dev-forward-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&build);
        let worker = build.join("workers").join("shop");
        std::fs::create_dir_all(&worker).unwrap();
        std::fs::write(
            worker.join("handlers.ts.map"),
            r#"{"version":3,"sources":["/p/src/shop.bynk"],"mappings":";AASA"}"#,
        )
        .unwrap();
        let rewriter = crate::logs::Rewriter::new(&build);
        let classifier = std::sync::Mutex::new(crate::logs::Classifier::default());
        let stream = format!(
            "GET / 200 OK (3ms)\r\nError: boom\n    at http_GET (file://{}/handlers.ts:2:5)\n    at runtime (file:///x/runtime.js:9:9)\n",
            worker.display()
        );
        let mut seen = Vec::new();
        forward_lines(
            stream.as_bytes(),
            "shop",
            &rewriter,
            &classifier,
            |l, lv| seen.push((l.to_string(), lv)),
        );
        let _ = std::fs::remove_dir_all(&build);
        use crate::logs::LogLevel::{Error, Info};
        assert_eq!(
            seen,
            vec![
                ("GET / 200 OK (3ms)".to_string(), Info),
                ("Error: boom".to_string(), Error),
                ("    at http_GET (/p/src/shop.bynk:10:1)".to_string(), Error),
                (
                    "    at runtime (file:///x/runtime.js:9:9)".to_string(),
                    Error
                ),
            ]
        );
    }

    /// A wrangler that prints and exits at once: the drain waits for its
    /// threads, and gives up on one whose pipe never closes.
    #[test]
    fn joining_the_output_waits_for_finished_threads_and_not_for_stuck_ones() {
        let done = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = done.clone();
        let slow = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            flag.store(true, std::sync::atomic::Ordering::SeqCst);
        });
        join_pumps(vec![slow], Duration::from_secs(5));
        assert!(done.load(std::sync::atomic::Ordering::SeqCst));

        let (release, held) = std::sync::mpsc::channel::<()>();
        let stuck = std::thread::spawn(move || {
            let _ = held.recv();
        });
        let started = std::time::Instant::now();
        join_pumps(vec![stuck], Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(2));
        drop(release);
    }
}
