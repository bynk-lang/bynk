//! Opt-in per-level timing of one project analysis (#1523, step 2).
//!
//! #1523's trigger asks for keystroke-to-diagnostic latency "attributed by
//! level": R3.13's four query levels. This attributes the wall time of
//! [`crate::analysis::analyse_project_with_policy`], the pass the language
//! server runs on every edit, to those levels:
//!
//! | Level | What it covers here |
//! |---|---|
//! | `file` | discovery, and reading and parsing every source file (through the shared parse cache) |
//! | `unit` | grouping, the per-unit symbol tables and signatures, `uses`/`consumes` resolution, and each unit's symbol composition and prelude |
//! | `definition` | resolving and checking every declaration body, plus test bodies and the platform lock |
//! | `project` | the whole-project validations (consumes cycles, name conflicts, exports, providers, event wiring), and assembling the result |
//!
//! The buckets partition the call: every instant belongs to exactly one, so
//! they sum to the round's time. The boundaries are marked where the
//! pipeline's own phase comments already put them, so the attribution is as
//! good as that decomposition; it does not claim to be a profile.
//!
//! **Off by default and free when off.** Nothing is recorded unless the
//! `BYNK_PHASE_TIMING` environment variable is set when the process first
//! asks; each mark is then one cached boolean test. When on, [`finish`] writes
//! one line to stderr, which `cargo xtask bench` reads from the language
//! server.

use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// One of R3.13's query levels.
#[derive(Clone, Copy, Debug)]
pub enum Level {
    File,
    Unit,
    Definition,
    Project,
}

const NAMES: [&str; 4] = ["file", "unit", "definition", "project"];

struct Stopwatch {
    at: Instant,
    level: Level,
    spent: [Duration; 4],
}

thread_local! {
    static STOPWATCH: RefCell<Option<Stopwatch>> = const { RefCell::new(None) };
}

fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os("BYNK_PHASE_TIMING").is_some())
}

/// Start timing the current analysis, charging time to `level` until the next
/// [`mark`]. A no-op unless `BYNK_PHASE_TIMING` is set.
pub fn begin(level: Level) {
    if !enabled() {
        return;
    }
    STOPWATCH.with(|s| {
        *s.borrow_mut() = Some(Stopwatch {
            at: Instant::now(),
            level,
            spent: [Duration::ZERO; 4],
        });
    });
}

/// From here, charge time to `level`; the time since the last mark goes to the
/// level that was current. A no-op unless timing is on and [`begin`] ran.
pub fn mark(level: Level) {
    if !enabled() {
        return;
    }
    STOPWATCH.with(|s| {
        if let Some(w) = s.borrow_mut().as_mut() {
            let now = Instant::now();
            w.spent[w.level as usize] += now - w.at;
            w.at = now;
            w.level = level;
        }
    });
}

/// Close the current analysis and write one line to stderr:
/// `bynk-phase-timing file=<ms> unit=<ms> definition=<ms> project=<ms>`.
pub fn finish() {
    if !enabled() {
        return;
    }
    mark(Level::Project);
    let spent = STOPWATCH.with(|s| s.borrow_mut().take().map(|w| w.spent));
    if let Some(spent) = spent {
        let mut line = String::from("bynk-phase-timing");
        for (name, d) in NAMES.iter().zip(spent) {
            line.push_str(&format!(" {name}={:.1}", d.as_secs_f64() * 1000.0));
        }
        eprintln!("{line}");
    }
}
