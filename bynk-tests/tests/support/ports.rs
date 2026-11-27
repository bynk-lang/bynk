//! Free localhost ports for tests that start a server and hand it the port (#3526).
//!
//! The pattern was `bind("127.0.0.1:0")`, read the port, drop the listener, once
//! per port wanted, copied into a dozen test files. It races in two ways under
//! parallel load, and both show as `Address already in use` in whatever test
//! happens to be starting a server:
//!
//! - **Within one test**: two calls in a row (`free_port(), free_port()` for the
//!   Worker and its inspector) each release their port before the next call, so
//!   the OS may hand the same one out again and the Worker and the inspector
//!   collide.
//! - **Across tests in one process**: `cargo test` runs tests on threads, and a
//!   port one test has released and not yet used is a port another test's
//!   `bind(":0")` may be given.
//!
//! [`free_port`] removes both, among callers of the helper. Every port it returns has been recorded in a
//! process-wide set, so it is never handed out twice in one process, and a
//! whole batch ([`free_ports`]) is bound at once so a batch is distinct.
//!
//! A test that still binds `127.0.0.1:0` directly (an upstream listener it serves
//! itself) is outside the registry, and may be given a port that was issued and
//! released before its child bound it; the odds are low for the same reason as below.
//!
//! What this cannot remove is the gap between returning the port and the child
//! binding it, because the child process binds it, not us, and a listener can't
//! be handed across `exec` portably. That gap is seconds long on a loaded machine
//! (a `wrangler dev` boots slowly), and (#4479) it is not a rare collision when
//! the ports come from the OS's ephemeral range: every outgoing connection of
//! the suite's own polling, and every `bind(":0")` of wrangler's own workerds, is
//! drawn from the same range, so a port that was free when returned is taken by
//! the time the child binds it. It showed as `Address already in use`, and as a
//! request answered by the *other* Worker that had the port.
//!
//! So the ports are not taken from the ephemeral range at all. [`BAND`] is a
//! band below it, which no connection and no `bind(":0")` is ever given, and
//! which the fixed per-test bands in `system/` do not use. A port in it is
//! tried for being bound, claimed across processes by a file in the temp
//! directory (nextest runs each test in its own process; the claim ends with its
//! owner, or after [`CLAIM_FOR`] where that cannot be told), and recorded in the process-wide set.

// Each test binary includes this whole module and uses part of it.
#![allow(dead_code)]

use std::collections::HashSet;
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

/// The band the ports come from: below every OS's ephemeral range (Linux's starts
/// at 32768), and between the fixed bands of the tests that still take one from
/// their process id (20 000 + 2·pid, 30 000 + 4·pid, and the rest).
const BAND: std::ops::Range<u16> = 28_000..30_000;

/// How long a claim stays valid where a dead owner cannot be told (off Linux, which has no
/// `/proc`): a test's whole run, with room to spare. On Linux a claim also ends with its owner.
const CLAIM_FOR: Duration = Duration::from_secs(10 * 60);

/// Ports this process has already handed out.
fn issued() -> &'static Mutex<HashSet<u16>> {
    static ISSUED: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
    ISSUED.get_or_init(Default::default)
}

/// Whether the process `pid` can still be running. Where that cannot be told, it can.
fn alive(pid: u32) -> bool {
    let proc = std::path::Path::new("/proc");
    !proc.join("self").exists() || proc.join(pid.to_string()).exists()
}

/// When the claim file at `path` was made, as nanoseconds since the epoch.
fn made(path: &std::path::Path) -> Option<u128> {
    let at = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    Some(at.duration_since(SystemTime::UNIX_EPOCH).ok()?.as_nanos())
}

/// Claim `port` against the other test processes. A claim is a file named for its owner's pid in
/// a directory of the port's own, so a claim needs no takeover: one whose owner is gone (or whose
/// age passes [`CLAIM_FOR`]) is simply ignored, and removed. Two processes that claim at once
/// each see the other's file, and the earlier one (then the lower pid) keeps the port. `false`
/// if another process holds it.
fn claim(port: u16) -> bool {
    let dir = std::env::temp_dir()
        .join("bynk-test-ports")
        .join(port.to_string());
    let _ = std::fs::create_dir_all(&dir);
    let me = std::process::id();
    let mine = dir.join(me.to_string());
    // `create_new`: the file is also this process's own record of the port. The harness is compiled
    // into the one test binary once per module that includes it, each copy with its own `issued`
    // set, so a port another copy issued is found here, not in `issued`, and is refused (#4578).
    if std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&mine)
        .is_err()
    {
        return false;
    }
    let mine_key = (made(&mine).unwrap_or(0), me);
    for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == me {
            continue;
        }
        let path = entry.path();
        let at = made(&path).unwrap_or(0);
        let age = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |now| now.as_nanos().saturating_sub(at));
        if !alive(pid) || age > CLAIM_FOR.as_nanos() {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        if (at, pid) < mine_key {
            let _ = std::fs::remove_file(&mine);
            return false;
        }
    }
    true
}

/// A start for the scan of [`BAND`] that differs between processes and calls.
fn start() -> usize {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as usize)
        .unwrap_or(0);
    nanos.wrapping_mul(2_654_435_761) ^ std::process::id() as usize
}

/// `N` distinct localhost ports, none handed out before by this process or
/// claimed by another test process, from [`BAND`]; from the OS (the old way)
/// only if the whole band is taken.
pub fn free_ports<const N: usize>() -> [u16; N] {
    // Held until the batch is complete, so a port can't be given out twice.
    let mut held: Vec<TcpListener> = Vec::with_capacity(N);
    let mut ports = [0u16; N];
    let mut issued = issued().lock().unwrap_or_else(|e| e.into_inner());
    let width = BAND.len();
    let from = start();
    for step in 0..width {
        if held.len() == N {
            break;
        }
        let port = BAND.start + ((from + step) % width) as u16;
        if issued.contains(&port) {
            continue;
        }
        let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) else {
            continue;
        };
        if claim(port) {
            issued.insert(port);
            ports[held.len()] = port;
            held.push(listener);
        }
    }
    if held.len() < N {
        eprintln!(
            "ports: the band {BAND:?} has no port left to give; taking the rest from the OS, \
             whose ephemeral range can collide with a booting wrangler (#4479)"
        );
    }
    let mut attempts = 0;
    while held.len() < N {
        attempts += 1;
        assert!(attempts < 1_000, "the OS keeps handing out used ports");
        let listener = TcpListener::bind("127.0.0.1:0").expect("the OS hands out a free port");
        let port = listener.local_addr().expect("local addr").port();
        if issued.insert(port) {
            ports[held.len()] = port;
            held.push(listener);
        }
    }
    ports
}

/// One free localhost port; see [`free_ports`].
pub fn free_port() -> u16 {
    free_ports::<1>()[0]
}
