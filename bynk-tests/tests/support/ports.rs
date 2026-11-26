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
//! directory (nextest runs each test in its own process; the claim goes stale
//! after [`CLAIM_FOR`]), and recorded in the process-wide set.

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

/// How long a port claimed by a process stays claimed against the others: a
/// test's whole run, with room to spare. A claim file left behind by a process
/// that has gone is taken over after this.
const CLAIM_FOR: Duration = Duration::from_secs(15 * 60);

/// Ports this process has already handed out.
fn issued() -> &'static Mutex<HashSet<u16>> {
    static ISSUED: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
    ISSUED.get_or_init(Default::default)
}

/// Claim `port` against the other processes: the claim file is created, or an
/// older one than [`CLAIM_FOR`] is taken over. `false` if another holds it.
fn claim(port: u16) -> bool {
    let dir = std::env::temp_dir().join("bynk-test-ports");
    let _ = std::fs::create_dir_all(&dir);
    let file = dir.join(port.to_string());
    let create = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
            .is_ok()
    };
    if create() {
        return true;
    }
    let stale = std::fs::metadata(&file)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| SystemTime::now().duration_since(t).ok())
        .is_some_and(|age| age > CLAIM_FOR);
    stale && std::fs::remove_file(&file).is_ok() && create()
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
