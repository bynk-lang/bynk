//! #1667: the server binary survives a malformed client message, and its exit
//! code says whether the session ended in an orderly `shutdown`.
//!
//! Before #1667 a `didOpen` whose text held a lone UTF-16 surrogate escape
//! (`JSON.stringify` writes one for a lone surrogate in a JS string) ended the
//! server with exit 0 and nothing on stderr, and every ending exited 0.

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde_json::Value;

fn frame(body: &str) -> Vec<u8> {
    format!("Content-Length: {}\r\n\r\n{body}", body.len()).into_bytes()
}

const INITIALIZE: &str =
    r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"capabilities":{}}}"#;
const INITIALIZED: &str = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
const SHUTDOWN: &str = r#"{"jsonrpc":"2.0","id":2,"method":"shutdown"}"#;
const EXIT: &str = r#"{"jsonrpc":"2.0","method":"exit"}"#;

/// How long any one wait on the server may take before the test fails, so a
/// broken server fails the test rather than hanging CI.
const TIMEOUT: Duration = Duration::from_secs(30);

/// What a session left behind: the exit code, every message the server
/// wrote (parsed), its raw stdout and its stderr.
struct Session {
    code: i32,
    messages: Vec<Value>,
    stdout: String,
    stderr: String,
}

impl Session {
    /// The server's response to the client request with `id`: a message
    /// carrying that id and no `method` (a server-to-client request can
    /// carry the same id).
    fn response(&self, id: i64) -> Option<&Value> {
        self.messages.iter().find(|m| is_response(m, id))
    }

    /// Assert that `shutdown` (id 2) was answered with success, not just
    /// answered: tower-lsp answers it with an error itself when the session
    /// isn't initialised yet, or when `exit` cancels it, and a test that
    /// raced its setup must report that rather than a bare exit code.
    fn assert_shutdown_succeeded(&self) {
        let response = self.response(2).unwrap_or_else(|| {
            panic!(
                "the server must still answer `shutdown` after the bad message; stdout:\n{}\nstderr:\n{}",
                self.stdout, self.stderr
            )
        });
        assert!(
            response.get("error").is_none() && response.get("result") == Some(&Value::Null),
            "`shutdown` must succeed (`\"result\": null`); got {response}\nstderr:\n{}",
            self.stderr
        );
    }
}

fn is_response(message: &Value, id: i64) -> bool {
    message.get("method").is_none() && message.get("id").and_then(Value::as_i64) == Some(id)
}

/// Read Content-Length framed messages from `pipe` until it closes, sending
/// each parsed body on `tx`. Returns everything read, for diagnostics.
fn read_frames(pipe: impl Read, tx: mpsc::Sender<Value>) -> String {
    let mut reader = BufReader::new(pipe);
    let mut raw = String::new();
    loop {
        let mut length = None;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return raw,
                Ok(_) => {}
            }
            raw.push_str(&line);
            let line = line.trim_end();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("content-length")
            {
                length = value.trim().parse::<usize>().ok();
            }
        }
        let Some(length) = length else {
            // A blank line with no header before it; keep reading.
            continue;
        };
        let mut body = vec![0; length];
        if reader.read_exact(&mut body).is_err() {
            return raw;
        }
        let body = String::from_utf8_lossy(&body).into_owned();
        raw.push_str(&body);
        if let Ok(message) = serde_json::from_str(&body) {
            // The receiver may have gone (the test is failing); keep
            // draining so the server never blocks on a full pipe.
            let _ = tx.send(message);
        }
    }
}

/// Kill the server and fail the test with `why` and whatever it wrote.
fn fail(
    tag: &str,
    mut child: Child,
    stdout: JoinHandle<String>,
    stderr: JoinHandle<String>,
    why: &str,
) -> ! {
    let _ = child.kill();
    let _ = child.wait();
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    panic!("{tag}: {why}; stdout:\n{stdout}\nstderr:\n{stderr}");
}

/// The id and method of `message` when it is a request (it has both), whose
/// response a client must wait for.
fn request(message: &str) -> Option<(i64, String)> {
    let value: Value = serde_json::from_str(message).ok()?;
    let id = value.get("id")?.as_i64()?;
    Some((id, value.get("method")?.as_str()?.to_owned()))
}

/// Wait for the response to request `id`, keeping whatever else arrives in
/// `received`.
fn await_response(
    rx: &mpsc::Receiver<Value>,
    id: i64,
    received: &mut Vec<Value>,
) -> Result<Value, &'static str> {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(message) if is_response(&message, id) => return Ok(message),
            Ok(message) => received.push(message),
            Err(mpsc::RecvTimeoutError::Timeout) => return Err("no response within 30s"),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("the server closed its output first");
            }
        }
    }
}

/// Run the server: send `initialize`, then `messages`, then close its input.
/// Like a real client, wait for each request's response before sending the
/// next message.
///
/// #2222: waiting matters. tower-lsp answers a request that arrives while
/// `initialize` is still being handled with "server not initialized", even
/// `shutdown`; and an `exit` that arrives while `shutdown` is pending cancels
/// it. Either way `Backend::shutdown` never runs and the server exits 1.
fn session(tag: &str, messages: &[&str]) -> Session {
    // A private HOME, so the server's log goes to a scratch file.
    let home = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("lsp-{tag}"));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(&home).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_bynkc-lsp"))
        .env("HOME", &home)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the server starts");
    // #1771 review: the pipes are read on threads so a full one can't stall
    // the child.
    let (tx, rx) = mpsc::channel();
    let pipe = child.stdout.take().unwrap();
    let stdout = std::thread::spawn(move || read_frames(pipe, tx));
    let mut pipe = child.stderr.take().unwrap();
    let stderr = std::thread::spawn(move || {
        let mut text = String::new();
        let _ = pipe.read_to_string(&mut text);
        text
    });

    let mut stdin = child.stdin.take().unwrap();
    let mut received = Vec::new();
    for message in std::iter::once(&INITIALIZE).chain(messages) {
        // A write can fail once the server has gone; the exit code says why.
        if stdin.write_all(&frame(message)).is_err() {
            break;
        }
        let _ = stdin.flush();
        let Some((id, method)) = request(message) else {
            std::thread::sleep(Duration::from_millis(100));
            continue;
        };
        let response = match await_response(&rx, id, &mut received) {
            Ok(response) => response,
            Err(why) => {
                let why = format!("waiting for the `{method}` response: {why}");
                fail(tag, child, stdout, stderr, &why);
            }
        };
        if method == "initialize" && response.get("result").is_none() {
            let why = format!("`initialize` failed: {response}");
            fail(tag, child, stdout, stderr, &why);
        }
        received.push(response);
    }
    drop(stdin);

    // #1771 review: bounded, so a regression into a hang fails the test
    // rather than blocking CI.
    let deadline = Instant::now() + TIMEOUT;
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll the server") {
            break status;
        }
        if Instant::now() > deadline {
            fail(
                tag,
                child,
                stdout,
                stderr,
                "the server didn't exit within 30s of its input closing",
            );
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let stdout = stdout.join().unwrap();
    received.extend(rx.try_iter());
    Session {
        code: status.code().unwrap_or(-1),
        messages: received,
        stdout,
        stderr: stderr.join().unwrap(),
    }
}

#[test]
fn a_lone_surrogate_in_a_message_does_not_end_the_session() {
    let did_open = r#"{"jsonrpc":"2.0","method":"textDocument/didOpen","params":{"textDocument":{"uri":"file:///tmp/x.bynk","languageId":"bynk","version":1,"text":"a\udcffb"}}}"#;
    let s = session("surrogate", &[INITIALIZED, did_open, SHUTDOWN, EXIT]);
    s.assert_shutdown_succeeded();
    assert_eq!(
        s.code, 0,
        "an orderly shutdown exits 0; stderr:\n{}",
        s.stderr
    );
}

#[test]
fn an_unparseable_message_is_skipped() {
    let s = session("garbage", &[INITIALIZED, "{not json", SHUTDOWN, EXIT]);
    s.assert_shutdown_succeeded();
    assert_eq!(s.code, 0, "stderr:\n{}", s.stderr);
}

/// #2239: a message after `exit` no longer leaves the server running.
/// tower-lsp 0.20 left `Server::serve` waiting for good when one arrived, even
/// once input closed.
#[test]
fn a_message_after_exit_does_not_keep_the_server_running() {
    let s = session("after-exit", &[INITIALIZED, SHUTDOWN, EXIT, INITIALIZED]);
    s.assert_shutdown_succeeded();
    assert_eq!(s.code, 0, "stderr:\n{}", s.stderr);
}

#[test]
fn ending_without_shutdown_exits_non_zero_with_a_reason() {
    let s = session("no-shutdown", &[INITIALIZED, EXIT]);
    assert_eq!(s.code, 1, "stderr:\n{}", s.stderr);
    assert!(
        s.stderr.contains("without a `shutdown` request"),
        "stderr:\n{}",
        s.stderr
    );

    let s = session("eof", &[INITIALIZED]);
    assert_eq!(
        s.code, 1,
        "closing input without `shutdown` is not orderly; stderr:\n{}",
        s.stderr
    );
}
