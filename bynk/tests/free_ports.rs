//! #3526: the shared free-port helper never hands a port out twice in a
//! process, even when many tests ask at once, and a batch is distinct.

#[path = "../../bynk-tests/tests/support/ports.rs"]
mod ports;

use std::collections::HashSet;

#[test]
fn a_batch_is_distinct() {
    let batch = ports::free_ports::<8>();
    assert_eq!(batch.iter().collect::<HashSet<_>>().len(), batch.len());
}

#[test]
fn no_port_is_issued_twice_across_threads() {
    let threads: Vec<_> = (0..16)
        .map(|_| std::thread::spawn(|| (0..50).map(|_| ports::free_port()).collect::<Vec<_>>()))
        .collect();
    let all: Vec<u16> = threads
        .into_iter()
        .flat_map(|t| t.join().expect("a thread asking for ports"))
        .collect();
    assert_eq!(all.iter().collect::<HashSet<_>>().len(), all.len());
}

// Pins that an issued port is bindable. It shares the cross-process window the
// helper documents, so it is very unlikely, not impossible, to flake.
#[test]
fn an_issued_port_can_be_bound_by_the_child_that_receives_it() {
    let port = ports::free_port();
    std::net::TcpListener::bind(("127.0.0.1", port)).expect("the port is free to bind");
}
