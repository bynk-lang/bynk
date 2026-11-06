//! Regression: `dev::compile_once` is the shared in-process compile step
//! behind both `bynk dev` and `bynk deploy` (`deploy.rs` calls it directly).
//! On a successful build it wrote the output and returned, without ever
//! calling `print_project_warnings` — so a non-failing warning (e.g. an
//! unused `given` capability) was silently swallowed.
//! `bynk/src/diagnostics.rs` re-exported
//! `print_project_warnings` under `#[allow(unused_imports)]` because nothing
//! in the driver called it.
//!
//! `compile_once` reports via `eprintln!`, which the default test harness
//! does not capture — so this redirects the process's real fd 2 to a temp
//! file around the call and reads it back. The redirect is a real syscall on
//! a process-global resource, so the swap is guarded by a mutex in case this
//! file ever grows a second test.

use std::io::Read;
use std::path::PathBuf;
use std::sync::Mutex;

use bynk::dev::compile_once;

static STDERR_REDIRECT: Mutex<()> = Mutex::new(());

#[cfg(unix)]
fn with_captured_stderr<F: FnOnce()>(f: F) -> String {
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn dup(fd: i32) -> i32;
        fn dup2(oldfd: i32, newfd: i32) -> i32;
        fn close(fd: i32) -> i32;
    }

    let _guard = STDERR_REDIRECT.lock().unwrap();
    let tmp = std::env::temp_dir().join(format!(
        "bynk-compile-once-stderr-{}-{:?}.txt",
        std::process::id(),
        std::thread::current().id()
    ));
    let file = std::fs::File::create(&tmp).expect("create capture file");

    // SAFETY: `dup`/`dup2`/`close` are standard POSIX calls; the fds involved
    // are either freshly opened by this function or fd 2, which every process
    // has. The mutex above serialises every use of this helper, so no other
    // thread observes fd 2 mid-swap.
    let saved_stderr = unsafe { dup(2) };
    assert!(saved_stderr >= 0, "dup(2) failed");
    // dup2 returns the new fd number (2) on success, -1 on error.
    let rc = unsafe { dup2(file.as_raw_fd(), 2) };
    assert_eq!(rc, 2, "dup2 onto fd 2 failed");

    f();

    use std::io::Write;
    let _ = std::io::stderr().flush();
    let rc = unsafe { dup2(saved_stderr, 2) };
    assert_eq!(rc, 2, "dup2 restoring fd 2 failed");
    unsafe { close(saved_stderr) };

    let mut s = String::new();
    std::fs::File::open(&tmp)
        .expect("reopen capture file")
        .read_to_string(&mut s)
        .expect("read capture file");
    let _ = std::fs::remove_file(&tmp);
    s
}

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/compile_once_warning")
}

#[test]
#[cfg(unix)]
fn compile_once_surfaces_non_failing_warnings() {
    let build_dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("compile-once-warning-out");
    let project_root = fixture();

    let mut ok = false;
    let stderr = with_captured_stderr(|| {
        // #980: this fixture is a committed repo directory, not a scratch
        // one — the same hazard `bynk-tests/tests/e2e.rs` has for its in-place
        // fixtures. `schema_registry: false` keeps this test from writing a
        // real `bynk.schema.lock` into the tree on every run.
        ok = compile_once(&project_root, &build_dir, false);
    });

    assert!(
        ok,
        "compile_once should succeed on a warning-only project, stderr:\n{stderr}"
    );
    assert!(
        stderr.contains("bynk.given.unused_capability"),
        "expected the unused-`given`-capability warning to be printed, got:\n{stderr}"
    );
}
