//! State-migrations S0 (#2204; `design/tracks/state-migrations.md` §3.1,
//! §3.4): `bynk.schema.lock` records every agent class's stored state shape,
//! and a build refuses a change deployed data cannot be read through.
//!
//! Each test drives a scratch project through several builds, reading and
//! writing the lock the way `bynk-driver`'s production callers do, exactly as
//! `events_schema_registry_behaviour.rs` does for events.

use bynkc::{BuildTarget, SchemaLock};
use std::fs;
use std::path::{Path, PathBuf};

/// A scratch project with one `.bynk` source and its lock.
struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bynk-agent-state-lock-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("shop")).unwrap();
        Project { root }
    }

    fn lock_path(&self) -> PathBuf {
        self.root.join("bynk.schema.lock")
    }

    fn lock(&self) -> String {
        fs::read_to_string(self.lock_path()).unwrap()
    }

    /// Build `source`. On success, write the reconciled lock as a real caller
    /// would and return the rendered warnings; on failure, return the
    /// rendered errors and leave the lock alone.
    fn build(&self, source: &str) -> Result<String, String> {
        fs::write(self.root.join("shop/cart.bynk"), source).unwrap();
        let existing = bynk_driver::schema_lock::read(&self.root).unwrap();
        let options = bynk_testkit::compile_options_single(&self.root)
            .target(BuildTarget::Bundle)
            .schema_registry(SchemaLock::On { existing });
        match bynkc::compile_project(&options) {
            Ok(out) => {
                if let Some(content) = &out.schema_lock {
                    bynk_driver::schema_lock::write(&self.root, content).unwrap();
                }
                let warnings: Vec<bynkc::CompileError> =
                    out.warnings.iter().map(|w| w.error.clone()).collect();
                Ok(bynkc::render_project_errors(&warnings))
            }
            Err(failure) => Err(bynkc::render_project_errors(&failure.flatten())),
        }
    }

    fn build_ok(&self, source: &str) -> String {
        self.build(source)
            .unwrap_or_else(|e| panic!("expected a clean build:\n{e}"))
    }

    fn build_err(&self, source: &str) -> String {
        match self.build(source) {
            Ok(_) => panic!("expected the build to be refused"),
            Err(e) => e,
        }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn cart(fields: &str) -> String {
    format!(
        "context shop\n\nagent Cart {{\n  key id: String\n\n{fields}\n\n  on call ping() -> Effect[Int] {{\n    1\n  }}\n}}\n"
    )
}

/// `source` with `decl` inserted after its `context` line.
fn with_decl(decl: &str, source: &str) -> String {
    source.replacen("context shop\n", &format!("context shop\n\n{decl}\n"), 1)
}

fn agent_version(lock: &str) -> &str {
    let at = lock.find("[agents.\"shop.Cart\"]").expect("a Cart entry");
    let rest = &lock[at..];
    let line = rest
        .lines()
        .find(|l| l.starts_with("version = "))
        .expect("a version line");
    line.trim_start_matches("version = ")
}

/// `bynkc check <root>`, which reconciles against the lock read-only
/// (ADR 0493). Returns whether it passed and its combined output.
fn check(root: &Path) -> (bool, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(["check".as_ref(), root.as_os_str()])
        .output()
        .expect("run bynkc");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

/// The track's first fixture (#539): renaming `store owner` to
/// `store ownerName` compiled with exit 0 and orphaned the stored value.
#[test]
fn renaming_a_store_field_is_refused_and_names_the_pairing() {
    let p = Project::new("rename");
    p.build_ok(&cart("  store owner: Cell[String]"));
    let before = p.lock();
    assert!(before.starts_with("version = 2"), "{before}");
    assert_eq!(agent_version(&before), "1", "{before}");

    let err = p.build_err(&cart("  store ownerName: Cell[String]"));
    assert!(
        err.contains("bynk.agent.non_additive_state_change"),
        "{err}"
    );
    assert!(err.contains("field(s) removed: owner"), "{err}");
    assert!(
        err.contains("`owner` was removed and `ownerName` added with the same stored shape"),
        "{err}"
    );
    assert!(err.contains("[agents.\"shop.Cart\"]"), "{err}");
    assert_eq!(p.lock(), before, "a refused build must not touch the lock");
}

#[test]
fn two_same_shaped_renames_suggest_no_pairing() {
    let p = Project::new("ambiguous");
    p.build_ok(&cart(
        "  store first: Cell[String]\n  store second: Cell[String]",
    ));
    let err = p.build_err(&cart(
        "  store one: Cell[String]\n  store two: Cell[String]",
    ));
    assert!(
        err.contains("bynk.agent.non_additive_state_change"),
        "{err}"
    );
    assert!(err.contains("cannot be inferred"), "{err}");
    assert!(!err.contains("was removed and"), "{err}");
}

#[test]
fn adding_a_field_bumps_the_version_and_reordering_does_not() {
    let p = Project::new("additive");
    p.build_ok(&cart("  store a: Cell[Int]\n  store b: Cell[String]"));
    p.build_ok(&cart("  store b: Cell[String]\n  store a: Cell[Int]"));
    assert_eq!(agent_version(&p.lock()), "1", "reordering is not a change");

    p.build_ok(&cart(
        "  store a: Cell[Int]\n  store b: Cell[String]\n  store c: Map[String, Int]",
    ));
    let lock = p.lock();
    assert_eq!(agent_version(&lock), "2", "{lock}");
    assert!(lock.contains("additive = true"), "{lock}");
}

#[test]
fn retyping_retyping_inside_a_record_and_changing_kind_are_refused() {
    let p = Project::new("retype");
    let with_item = |item: &str, fields: &str| with_decl(item, &cart(fields));
    let base = with_item(
        "type Item = { title: String, qty: Int }",
        "  store items: Map[String, Item]\n  store n: Cell[Int]",
    );
    p.build_ok(&base);

    // Renaming the record *type* changes nothing on disk.
    p.build_ok(&with_item(
        "type Entry = { title: String, qty: Int }",
        "  store items: Map[String, Entry]\n  store n: Cell[Int]",
    ));
    assert_eq!(
        agent_version(&p.lock()),
        "1",
        "a type rename is not a change"
    );

    let err = p.build_err(&with_item(
        "type Entry = { name: String, qty: Int }",
        "  store items: Map[String, Entry]\n  store n: Cell[Int]",
    ));
    assert!(err.contains("field(s) retyped: items"), "{err}");

    let err = p.build_err(&with_item(
        "type Entry = { title: String, qty: Int }",
        "  store items: Map[String, Entry]\n  store n: Cell[String]",
    ));
    assert!(err.contains("field(s) retyped: n"), "{err}");

    let err = p.build_err(&with_item(
        "type Entry = { title: String, qty: Int }",
        "  store items: Map[String, Entry]\n  store n: Set[Int]",
    ));
    assert!(err.contains("field(s) changed storage kind: n"), "{err}");
}

/// The #539 failure one level down (#2204 review): a stored sum's payload is
/// keyed by field name, so renaming a payload field orphans data just as
/// renaming a `store` field does, while reordering payload fields does not.
#[test]
fn renaming_a_sum_payload_field_is_refused_and_reordering_is_not() {
    let p = Project::new("payload");
    let with_sum = |sum: &str| with_decl(sum, &cart("  store last: Cell[Option[Event]]"));
    p.build_ok(&with_sum(
        "type Event = | Added(sku: String, qty: Int) | Cleared",
    ));
    p.build_ok(&with_sum(
        "type Event = | Added(qty: Int, sku: String) | Cleared",
    ));
    assert_eq!(agent_version(&p.lock()), "1", "reordering is not a change");
    let err = p.build_err(&with_sum(
        "type Event = | Added(item: String, qty: Int) | Cleared",
    ));
    assert!(err.contains("field(s) retyped: last"), "{err}");
}

#[test]
fn a_refinement_change_warns_and_bumps_the_version() {
    let p = Project::new("refine");
    let with_qty = |pred: &str| {
        with_decl(
            &format!("type Qty = Int where {pred}"),
            &cart("  store q: Cell[Qty] = 0"),
        )
    };
    p.build_ok(&with_qty("InRange(0, 10)"));
    let warnings = p.build_ok(&with_qty("InRange(0, 5)"));
    assert!(
        warnings.contains("bynk.agent.refinement_changed"),
        "{warnings}"
    );
    let lock = p.lock();
    assert_eq!(agent_version(&lock), "2", "{lock}");
}

#[test]
fn removing_an_agent_class_is_refused() {
    let p = Project::new("removed");
    p.build_ok(&cart("  store owner: Cell[String]"));
    let err = p.build_err("context shop\n\nfn noop() -> Int {\n  1\n}\n");
    assert!(err.contains("bynk.agent.class_removed"), "{err}");
    assert!(err.contains("`shop.Cart`"), "{err}");
}

#[test]
fn a_v1_lock_upgrades_on_build_and_is_left_alone_by_check() {
    let p = Project::new("upgrade");
    fs::write(p.lock_path(), "version = 1\n\n[events]\n").unwrap();
    fs::write(
        p.root.join("shop/cart.bynk"),
        cart("  store owner: Cell[String]"),
    )
    .unwrap();

    let (ok, out) = check(&p.root);
    assert!(ok, "a v1 lock must be readable:\n{out}");
    assert_eq!(p.lock(), "version = 1\n\n[events]\n", "check never writes");

    p.build_ok(&cart("  store owner: Cell[String]"));
    let lock = p.lock();
    assert!(lock.starts_with("version = 2"), "{lock}");
    assert!(lock.contains("[agents.\"shop.Cart\"]"), "{lock}");
}

#[test]
fn check_reports_a_refused_change_without_writing() {
    let p = Project::new("check");
    p.build_ok(&cart("  store owner: Cell[String]"));
    let before = p.lock();
    fs::write(
        p.root.join("shop/cart.bynk"),
        cart("  store ownerName: Cell[String]"),
    )
    .unwrap();
    let (ok, out) = check(&p.root);
    assert!(!ok, "check must fail:\n{out}");
    assert!(
        out.contains("bynk.agent.non_additive_state_change"),
        "{out}"
    );
    assert_eq!(p.lock(), before);
}

/// The S1 `__v` marker lives inside the stored record (track §3.2), so no
/// `store` field may ever be named `__v`. The grammar already guarantees it:
/// an identifier is `[A-Za-z][A-Za-z0-9_]*`, so `__v` lexes as `_`, `_`, `v`
/// and never parses as a field name. This pins that guarantee; if identifiers
/// ever admit a leading `_`, a reservation check must land with the change.
#[test]
fn no_store_field_can_start_with_an_underscore() {
    let p = Project::new("reserved");
    let err = p.build_err(&cart("  store __v: Cell[Int]"));
    assert!(err.contains("bynk.parse.expected_token"), "{err}");
}
