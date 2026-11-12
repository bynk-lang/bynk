//! A tiny worker pool for the corpus-wide tests (#3387).
//!
//! Those tests are each ONE nextest test, so nextest cannot spread their
//! per-fixture work over cores; they do it themselves. `par_map` runs `f` over
//! `items` on up to 8 threads and returns the results in input order, so a
//! caller's output stays deterministic. A panic in `f` propagates out of the
//! scope and fails the test, as it would serially.

use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

pub fn par_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .clamp(1, 8);
    let next = AtomicUsize::new(0);
    let slots: Vec<Mutex<Option<R>>> = items.iter().map(|_| Mutex::new(None)).collect();
    std::thread::scope(|s| {
        for _ in 0..workers {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(item) = items.get(i) else { break };
                    *slots[i].lock().unwrap() = Some(f(item));
                }
            });
        }
    });
    slots
        .into_iter()
        .map(|m| m.into_inner().unwrap().expect("every slot is filled"))
        .collect()
}
