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
//! be handed across `exec` portably. Across *processes* (nextest runs each test
//! in its own) the kernel's choice of ephemeral port is random over about 28 000
//! ports, so that gap is a rare collision rather than a systematic one.

// Each test binary includes this whole module and uses part of it.
#![allow(dead_code)]

use std::collections::HashSet;
use std::net::TcpListener;
use std::sync::{Mutex, OnceLock};

/// Ports this process has already handed out.
fn issued() -> &'static Mutex<HashSet<u16>> {
    static ISSUED: OnceLock<Mutex<HashSet<u16>>> = OnceLock::new();
    ISSUED.get_or_init(Default::default)
}

/// `N` distinct localhost ports, none handed out before by this process.
pub fn free_ports<const N: usize>() -> [u16; N] {
    // Held until the batch is complete, so the OS can't hand one out twice.
    let mut held: Vec<TcpListener> = Vec::with_capacity(N);
    let mut ports = [0u16; N];
    let mut issued = issued().lock().unwrap_or_else(|e| e.into_inner());
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
