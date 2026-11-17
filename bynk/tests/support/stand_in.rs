//! Writing stand-in executables (fake `wrangler`, `npm`, git hooks) that a test
//! then runs, without ETXTBSY.

// Each test binary includes this whole module and uses part of it.
#![allow(dead_code)]

use std::path::Path;

/// Write an executable `#!/bin/sh` stand-in at `path`, mode 755.
///
/// By a child `sh`, never by this process. A test binary runs its tests on
/// many threads; if this process held the script open for writing, a thread
/// that forked in that window would carry the descriptor into its child until
/// that child's exec (`O_CLOEXEC` acts only at exec), and running the script
/// meanwhile fails with ETXTBSY ("Text file busy", os error 26). A child `sh`
/// opens, writes and closes the file in its own process, so no descriptor for
/// it ever exists here to inherit. The crate's unit tests share the same helper
/// (`bynk/src/test_support.rs`).
#[cfg(unix)]
pub fn write_executable(path: &Path, script: &str) {
    let wrote = std::process::Command::new("sh")
        .arg("-c")
        .arg("printf '%s' \"$1\" > \"$2\" && chmod 755 \"$2\"")
        .arg("sh")
        .arg(script)
        .arg(path)
        .status()
        .expect("spawn sh to write the stand-in");
    assert!(
        wrote.success(),
        "the stand-in {} is written",
        path.display()
    );
}
