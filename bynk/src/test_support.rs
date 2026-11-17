//! Helpers shared by this crate's unit tests.

use std::path::Path;

/// Write an executable stand-in (a `#!/bin/sh` script, such as a fake
/// `wrangler`) at `path`, mode 755.
///
/// By a child `sh`, never by this process. A test binary runs its tests on
/// many threads; if this process held the script open for writing, a thread
/// that forked in that window would carry the descriptor into its child until
/// that child's exec (`O_CLOEXEC` acts only at exec), and running the script
/// meanwhile fails with ETXTBSY ("Text file busy", os error 26). A child `sh`
/// opens, writes and closes the file in its own process, so no descriptor for
/// it ever exists here to inherit. First found in the provisioning tests
/// (#2055), then seen in the Vectorize ones; the back-half paydown's recording
/// `Wrangler` fake (1D.3) removes the on-disk stand-ins altogether.
#[cfg(unix)]
pub(crate) fn write_executable(path: &Path, script: &str) {
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
