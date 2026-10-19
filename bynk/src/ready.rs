//! `bynk dev --ready-json <FILE>` (#2138, end-to-end testing track §3.5,
//! DECISION F): a machine-readable readiness signal.
//!
//! Once every served context has answered an HTTP probe, `bynk dev` writes
//! `{ "<worker>": "<url>" }` to the file. A test runner (`bynk test --against
//! dev`) or a wrapper such as `bynk-lang/bynk-dev` waits for the file instead
//! of looping on a warm-up request. The file is removed when the session starts
//! (so a stale one is never read as ready) and when it stops.
//!
//! Readiness means each Worker answers a request. It does not mean the
//! dev-registry Service Bindings between them have connected yet: they
//! converge shortly after start.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// How long one probe waits to connect, and then for the response's first
/// bytes. Short, because the probe runs inside the dev watch loop.
const PROBE_TIMEOUT: Duration = Duration::from_millis(300);

/// How long without readiness before `bynk dev` notes, once, which contexts
/// haven't answered.
pub const SLOW_NOTE_AFTER: Duration = Duration::from_secs(30);

/// Whether something answering HTTP listens at `host:port`. Sends a `HEAD /`
/// (safe per RFC 9110, so a `GET /` route with side effects is never
/// tripped) and takes any `HTTP/` status line as an answer, a 404 included:
/// every emitted Worker has a `fetch`, so any status means it is up.
pub fn probe(host: &str, port: u16) -> bool {
    // `localhost` may resolve to `::1` and `127.0.0.1`; the server may listen
    // on either, so try each address.
    let Some(mut stream) = resolve(host, port)
        .into_iter()
        .find_map(|addr| TcpStream::connect_timeout(&addr, PROBE_TIMEOUT).ok())
    else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(PROBE_TIMEOUT));
    let _ = stream.set_write_timeout(Some(PROBE_TIMEOUT));
    let request = format!("HEAD / HTTP/1.1\r\nHost: {host}:{port}\r\nConnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return false;
    }
    let mut head = [0u8; 5];
    let mut read = 0;
    while read < head.len() {
        match stream.read(&mut head[read..]) {
            Ok(0) | Err(_) => return false,
            Ok(n) => read += n,
        }
    }
    &head == b"HTTP/"
}

fn resolve(host: &str, port: u16) -> Vec<SocketAddr> {
    (host, port)
        .to_socket_addrs()
        .map(Iterator::collect)
        .unwrap_or_default()
}

/// Why the wrangler passthrough `args` serve somewhere the readiness probe
/// (plain HTTP on localhost) can't reach: `-- --local-protocol https`, or
/// `-- --ip` bound to an address that isn't loopback.
pub fn unprobeable(args: &[String]) -> Option<String> {
    let value = |flag: &str| {
        args.iter().enumerate().find_map(|(i, a)| {
            a.strip_prefix(&format!("{flag}="))
                .map(str::to_string)
                .or_else(|| (a == flag).then(|| args.get(i + 1).cloned()).flatten())
        })
    };
    if value("--local-protocol").as_deref() == Some("https") {
        return Some("`-- --local-protocol https` serves over TLS".to_string());
    }
    match value("--ip") {
        Some(ip)
            if !matches!(
                ip.as_str(),
                "localhost" | "127.0.0.1" | "::1" | "0.0.0.0" | "*"
            ) =>
        {
            Some(format!("`-- --ip {ip}` serves away from localhost"))
        }
        _ => None,
    }
}

/// The readiness file's contents: each worker's URL, by worker name.
pub fn ready_json(urls: &BTreeMap<String, String>) -> String {
    serde_json::to_string_pretty(urls).expect("a string map serialises") + "\n"
}

/// Write `contents` to `path` atomically: to a sibling temporary file first,
/// then renamed over `path`, so a reader never sees a half-written file.
pub fn write_atomically(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

/// Remove `path` if it is there.
pub fn remove(path: &Path) {
    let _ = std::fs::remove_file(path);
}

/// The pending readiness of one `bynk dev` session: the workers still to
/// answer, and where to write the file once none remain.
#[derive(Debug)]
pub struct Readiness {
    file: PathBuf,
    /// Every served worker's port and URL.
    workers: BTreeMap<String, (u16, String)>,
    /// The workers that have answered a probe; each is probed until it does.
    answered: BTreeSet<String>,
    written: bool,
    started: Instant,
    noted_slow: bool,
}

impl Readiness {
    /// A session serving `contexts` (worker name, port, URL), writing `file`
    /// once all of them answer. Removes a stale `file` now.
    pub fn new(file: PathBuf, contexts: impl IntoIterator<Item = (String, u16, String)>) -> Self {
        remove(&file);
        Readiness {
            file,
            workers: contexts
                .into_iter()
                .map(|(name, port, url)| (name, (port, url)))
                .collect(),
            answered: BTreeSet::new(),
            written: false,
            started: Instant::now(),
            noted_slow: false,
        }
    }

    /// Probe each worker that hasn't answered yet once (via `probe`, at
    /// `localhost` and its port) and, the first time every worker has
    /// answered, write the file. Returns the write's result that one time,
    /// and `None` otherwise: while a worker is still booting, and on every
    /// poll after the write. A failed write is reported once and not retried:
    /// the session keeps serving, without a readiness file.
    pub fn poll(&mut self, probe: impl Fn(&str, u16) -> bool) -> Option<std::io::Result<()>> {
        if self.written {
            return None;
        }
        for (name, (port, _)) in &self.workers {
            if !self.answered.contains(name) && probe("localhost", *port) {
                self.answered.insert(name.clone());
            }
        }
        if self.answered.len() < self.workers.len() {
            return None;
        }
        self.written = true;
        let urls: BTreeMap<String, String> = self
            .workers
            .iter()
            .map(|(name, (_, url))| (name.clone(), url.clone()))
            .collect();
        Some(write_atomically(&self.file, &ready_json(&urls)))
    }

    /// Once, when `now` is [`SLOW_NOTE_AFTER`] past the session's start and
    /// the file still isn't written: a note naming the contexts that haven't
    /// answered, so a probe that can never succeed (a TLS-only or
    /// non-loopback server) doesn't hang silently.
    pub fn slow_note(&mut self, now: Instant) -> Option<String> {
        if self.written || self.noted_slow || now.duration_since(self.started) < SLOW_NOTE_AFTER {
            return None;
        }
        self.noted_slow = true;
        let pending: Vec<String> = self
            .workers
            .iter()
            .filter(|(name, _)| !self.answered.contains(*name))
            .map(|(name, (_, url))| format!("`{name}` ({url})"))
            .collect();
        Some(format!(
            "bynk dev: still waiting for {} to answer before writing the `--ready-json` file; it's written only once every context answers a plain-HTTP probe on localhost",
            pending.join(", ")
        ))
    }

    /// The session is ending: remove the file, so it is never read as ready.
    pub fn stop(&self) {
        remove(&self.file);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// A stand-in that answers one request with `reply`, then closes.
    fn stand_in(reply: &'static [u8]) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut buf = [0u8; 512];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(reply);
            }
        });
        port
    }

    fn free_port() -> u16 {
        // Bound and dropped: nothing listens on it afterwards.
        TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    #[test]
    fn tls_and_non_loopback_passthroughs_are_flagged() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(unprobeable(&args(&["--local-protocol", "https"])).is_some());
        assert!(unprobeable(&args(&["--local-protocol=https"])).is_some());
        assert!(unprobeable(&args(&["--ip", "192.168.1.5"])).is_some());
        assert!(unprobeable(&args(&["--ip", "127.0.0.1"])).is_none());
        assert!(unprobeable(&args(&["--local-protocol", "http", "--var", "K:V"])).is_none());
        assert!(unprobeable(&[]).is_none());
    }

    #[test]
    fn a_404_is_an_answer() {
        let port = stand_in(b"HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\n\r\n");
        assert!(probe("127.0.0.1", port));
    }

    #[test]
    fn nothing_listening_is_no_answer() {
        assert!(!probe("127.0.0.1", free_port()));
    }

    #[test]
    fn a_listener_that_never_replies_is_no_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let port = listener.local_addr().unwrap().port();
        // Accepts, then holds the connection open without a byte.
        std::thread::spawn(move || {
            let held: Vec<_> = listener.incoming().take(4).collect();
            std::thread::sleep(Duration::from_secs(5));
            drop(held);
        });
        let started = Instant::now();
        assert!(!probe("127.0.0.1", port));
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the probe gives up at its timeout"
        );
    }

    #[test]
    fn only_unanswered_workers_are_probed_and_a_slow_session_is_noted_once() {
        let dir = std::env::temp_dir().join(format!("bynk-ready-slow-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut r = Readiness::new(
            dir.join("ready.json"),
            [
                ("a".to_string(), 1, "http://localhost:1".to_string()),
                ("b".to_string(), 2, "http://localhost:2".to_string()),
            ],
        );
        let probed = std::cell::RefCell::new(Vec::new());
        let probe = |_: &str, port: u16| {
            probed.borrow_mut().push(port);
            port == 1
        };
        assert!(r.poll(probe).is_none());
        assert!(r.poll(probe).is_none());
        assert_eq!(
            *probed.borrow(),
            [1, 2, 2],
            "`a` is probed until it answers, not after"
        );
        assert!(r.slow_note(Instant::now()).is_none(), "not yet");
        let later = Instant::now() + SLOW_NOTE_AFTER;
        let note = r.slow_note(later).expect("noted after the delay");
        assert!(
            note.contains("`b` (http://localhost:2)") && !note.contains("`a`"),
            "{note}"
        );
        assert!(r.slow_note(later).is_none(), "noted once");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_non_http_reply_is_no_answer() {
        let port = stand_in(b"SSH-2.0-nope\r\n");
        assert!(!probe("127.0.0.1", port));
    }

    #[test]
    fn the_file_is_written_once_every_worker_answers_and_removed_on_stop() {
        let dir = std::env::temp_dir().join(format!("bynk-ready-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("ready.json");
        std::fs::write(&file, "stale").unwrap();
        let mut r = Readiness::new(
            file.clone(),
            [
                ("a".to_string(), 1, "http://localhost:1".to_string()),
                ("b".to_string(), 2, "http://localhost:2".to_string()),
            ],
        );
        assert!(!file.exists(), "a stale file is removed at start");
        assert!(r.poll(|_, port| port == 1).is_none());
        assert!(
            !file.exists(),
            "not written while a worker is still booting"
        );
        assert!(r.poll(|_, _| true).expect("written now").is_ok());
        assert!(r.poll(|_, _| true).is_none(), "written once");
        let written: BTreeMap<String, String> =
            serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(written["a"], "http://localhost:1");
        assert_eq!(written["b"], "http://localhost:2");
        r.stop();
        assert!(!file.exists(), "removed when the session stops");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
