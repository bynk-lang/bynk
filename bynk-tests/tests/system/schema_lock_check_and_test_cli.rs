//! #2078: `bynk check` and `bynk test` reconcile events against the
//! committed `bynk.schema.lock`, read-only. A non-additive event change fails
//! both with `bynk.event.non_additive_schema_change`, the same diagnostic
//! `bynk compile` refuses it with, so a CI gate of format, check and test
//! catches it before deploy. An additive change passes and leaves the lock
//! byte-identical (only a build records it), and a project with no lock is
//! a fresh project: no error, and no lock is written.
//!
//! No `tsc`/`node` needed: a failing `test` stops at the compile, and the
//! passing runs use `--no-run --format json`, which stops after it.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const CODE: &str = "bynk.event.non_additive_schema_change";

const SOURCE_V1: &str = r#"context commerce.order

consumes bynk { Events }

event PaymentConfirmed = {
  orderId: String,
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId })
  }
}
"#;

const SOURCE_V2_ADDITIVE: &str = r#"context commerce.order

consumes bynk { Events }

event PaymentConfirmed = {
  orderId: String,
  region: String = "US",
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId, region: "US" })
  }
}
"#;

const SOURCE_V3_NON_ADDITIVE: &str = r#"context commerce.order

consumes bynk { Events }

event PaymentConfirmed = {
  orderId: String,
  region: String,
}

service markPaid {
  on call(orderId: String) -> Effect[()] given Events {
    Events.emit[PaymentConfirmed](PaymentConfirmed { orderId: orderId, region: "US" })
  }
}
"#;

/// A throwaway single-tree project, removed on drop (including on panic).
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

impl Scratch {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "bynk-2078-schema-lock-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("commerce")).unwrap();
        Scratch(dir)
    }

    fn root(&self) -> &Path {
        &self.0
    }

    fn write_source(&self, source: &str) {
        fs::write(self.0.join("commerce/order.bynk"), source).unwrap();
    }

    fn lock(&self) -> PathBuf {
        self.0.join("bynk.schema.lock")
    }

    /// Commit a lock for `SOURCE_V1` the way a project gets one: a real
    /// `bynk compile`. Returns its bytes.
    fn commit_v1_lock(&self) -> Vec<u8> {
        self.write_source(SOURCE_V1);
        let out = self.0.join("out");
        let (ok, text) = bynk(&[
            "compile".as_ref(),
            self.0.as_os_str(),
            "-o".as_ref(),
            out.as_os_str(),
        ]);
        assert!(ok, "compiling V1 must succeed:\n{text}");
        fs::read(self.lock()).expect("compile wrote bynk.schema.lock")
    }
}

/// Run `bynk` with `args`, returning success and stdout + stderr.
fn bynk(args: &[&std::ffi::OsStr]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(args)
        .output()
        .expect("run bynk");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

fn check(root: &Path) -> (bool, String) {
    bynk(&["check".as_ref(), root.as_os_str()])
}

fn check_short(root: &Path) -> (bool, String) {
    bynk(&[
        "check".as_ref(),
        "--format".as_ref(),
        "short".as_ref(),
        root.as_os_str(),
    ])
}

fn test(root: &Path) -> (bool, String) {
    bynk(&["test".as_ref(), root.as_os_str()])
}

fn test_discover(root: &Path) -> (bool, String) {
    bynk(&[
        "test".as_ref(),
        "--no-run".as_ref(),
        "--format".as_ref(),
        "json".as_ref(),
        root.as_os_str(),
    ])
}

#[test]
fn a_non_additive_change_fails_check_and_test_and_leaves_the_lock_alone() {
    let proj = Scratch::new("non-additive");
    let committed = proj.commit_v1_lock();
    proj.write_source(SOURCE_V3_NON_ADDITIVE);

    for (what, (ok, out)) in [
        ("check", check(proj.root())),
        ("check --format short", check_short(proj.root())),
        ("test", test(proj.root())),
        ("test --no-run --format json", test_discover(proj.root())),
    ] {
        assert!(!ok, "`bynk {what}` must fail a non-additive change:\n{out}");
        assert!(
            out.contains(CODE),
            "`bynk {what}` must report {CODE}:\n{out}"
        );
        assert!(
            out.contains("region"),
            "`bynk {what}` must name the added field:\n{out}"
        );
        assert_eq!(
            fs::read(proj.lock()).unwrap(),
            committed,
            "`bynk {what}` must not touch the committed lock"
        );
    }
}

#[test]
fn an_additive_change_passes_check_and_test_without_writing_the_lock() {
    let proj = Scratch::new("additive");
    let committed = proj.commit_v1_lock();
    proj.write_source(SOURCE_V2_ADDITIVE);

    for (what, (ok, out)) in [
        ("check", check(proj.root())),
        ("test --no-run --format json", test_discover(proj.root())),
    ] {
        assert!(ok, "`bynk {what}` must pass an additive change:\n{out}");
        assert!(!out.contains(CODE), "`bynk {what}`:\n{out}");
        assert_eq!(
            fs::read(proj.lock()).unwrap(),
            committed,
            "`bynk {what}` must leave the lock byte-identical; only a build records the bump"
        );
    }
}

#[test]
fn no_lock_means_no_error_and_no_lock_written() {
    let proj = Scratch::new("no-lock");
    proj.write_source(SOURCE_V3_NON_ADDITIVE);

    for (what, (ok, out)) in [
        ("check", check(proj.root())),
        ("test --no-run --format json", test_discover(proj.root())),
    ] {
        assert!(ok, "`bynk {what}` with no lock must pass:\n{out}");
        assert!(
            !proj.lock().exists(),
            "`bynk {what}` must not create bynk.schema.lock"
        );
    }
}

/// #2081 (a test gap from #2080's review): a garbage lock is corruption, not
/// a fresh project. `check` and `test` refuse it with
/// `bynk.project.schema_registry_corrupt`, as a build does, and leave it as
/// they found it.
#[test]
fn a_garbage_lock_fails_check_and_test_as_corrupt() {
    const CORRUPT: &str = "bynk.project.schema_registry_corrupt";
    let proj = Scratch::new("garbage");
    proj.write_source(SOURCE_V1);
    let garbage = b"this is [[[ not a schema lock\n".to_vec();
    fs::write(proj.lock(), &garbage).unwrap();

    for (what, (ok, out)) in [
        ("check", check(proj.root())),
        ("test --no-run --format json", test_discover(proj.root())),
    ] {
        assert!(!ok, "`bynk {what}` must fail a corrupt lock:\n{out}");
        assert!(
            out.contains(CORRUPT),
            "`bynk {what}` must report {CORRUPT}:\n{out}"
        );
        assert_eq!(
            fs::read(proj.lock()).unwrap(),
            garbage,
            "`bynk {what}` must not touch the corrupt lock"
        );
    }
}

/// #2081 (a test gap from #2080's review): a declared `@schema(N)` that
/// disagrees with the version the lock computes fails `check` and `test`
/// with `bynk.event.schema_version_mismatch`, as it fails a build.
#[test]
fn a_schema_annotation_the_lock_disagrees_with_fails_check_and_test() {
    const MISMATCH: &str = "bynk.event.schema_version_mismatch";
    let proj = Scratch::new("mismatch");
    let committed = proj.commit_v1_lock();
    // Same shape as V1, so the lock computes version 1; the annotation says 2.
    proj.write_source(&SOURCE_V1.replace(
        "event PaymentConfirmed = {",
        "event PaymentConfirmed @schema(2) = {",
    ));

    for (what, (ok, out)) in [("check", check(proj.root())), ("test", test(proj.root()))] {
        assert!(!ok, "`bynk {what}` must fail the mismatch:\n{out}");
        assert!(
            out.contains(MISMATCH),
            "`bynk {what}` must report {MISMATCH}:\n{out}"
        );
        assert_eq!(fs::read(proj.lock()).unwrap(), committed);
    }
}

/// #2081 (a test gap from #2080's review): `bynk test` emits the version the
/// registry computes, as a build does. After an additive change the lock
/// still records schema 1 (test never writes it), but the reconciled version
/// is 2, and that is what the emitted mint site carries.
#[test]
fn test_emits_the_registry_computed_schema_version() {
    let proj = Scratch::new("emitted-version");
    let committed = proj.commit_v1_lock();
    proj.write_source(SOURCE_V2_ADDITIVE);
    let out_dir = proj.root().join("test-out");

    let (ok, out) = bynk(&[
        "test".as_ref(),
        "--no-run".as_ref(),
        "-o".as_ref(),
        out_dir.as_os_str(),
        proj.root().as_os_str(),
    ]);
    assert!(
        ok,
        "`bynk test --no-run` must pass an additive change:\n{out}"
    );

    let mut emitted = String::new();
    let mut stack = vec![out_dir];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "ts") {
                emitted.push_str(&fs::read_to_string(&path).unwrap());
            }
        }
    }
    assert!(
        emitted.contains("schemaVersion: 2"),
        "the emitted mint site must carry the registry's bumped version 2"
    );
    assert!(
        !emitted.contains("schemaVersion: 1"),
        "no mint site may carry the unreconciled version 1"
    );
    assert_eq!(
        fs::read(proj.lock()).unwrap(),
        committed,
        "`bynk test` must still not record the bump"
    );
}
