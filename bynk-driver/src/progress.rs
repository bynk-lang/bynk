//! #3100: per-context build progress, printed to stderr.
//!
//! The pipeline reports each context through a [`ProgressSink`]
//! ([`crate::project::CompileOptions::progress`]); this is the one printer the
//! verbs that build share, so `build`, `dev` and `deploy` read the same.
//!
//! The lines appear only when stderr is a terminal and `--quiet` is not given,
//! so a script or CI log that captures the output sees exactly what it did
//! before.

use std::io::IsTerminal;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::project::{ProgressEvent, ProgressSink};

static QUIET: AtomicBool = AtomicBool::new(false);

/// `--quiet`: turn the per-context lines off for this process.
pub fn set_quiet(quiet: bool) {
    QUIET.store(quiet, Ordering::Relaxed);
}

/// The printer for this process, or `None` when nothing should print: stderr
/// is not a terminal, or `--quiet` was given.
pub fn terminal_sink() -> Option<ProgressSink> {
    printer_if(std::io::stderr().is_terminal() && !QUIET.load(Ordering::Relaxed))
}

fn printer_if(on: bool) -> Option<ProgressSink> {
    on.then(|| Arc::new(|event: &ProgressEvent| eprintln!("{}", line(event))) as ProgressSink)
}

/// One event's line: a right-aligned verb, then the context, as `cargo` does.
pub fn line(event: &ProgressEvent) -> String {
    match event {
        ProgressEvent::Started(name) => format!("{:>10} {name}", "Compiling"),
        ProgressEvent::Finished(name) => format!("{:>10} {name}", "Finished"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_align_their_verbs() {
        assert_eq!(
            line(&ProgressEvent::Started("shop".into())),
            " Compiling shop"
        );
        assert_eq!(
            line(&ProgressEvent::Finished("shop".into())),
            "  Finished shop"
        );
    }

    #[test]
    fn off_means_no_sink() {
        assert!(printer_if(false).is_none());
        assert!(printer_if(true).is_some());
    }
}
