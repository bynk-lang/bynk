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

// ---------------------------------------------------------------------------
// State-migrations S2 (#2224): `migrate from N` steps.
// ---------------------------------------------------------------------------

/// `source`'s `cart(...)` with `decls` inserted after the context line.
fn cart_with(decls: &str, fields: &str) -> String {
    with_decl(decls, &cart(fields))
}

/// The track's first fixture turns positive: the rename carried by a step
/// builds, and the lock records version 2 as a non-additive change.
#[test]
fn a_rename_carried_by_a_migrate_step_builds() {
    let p = Project::new("s2-rename");
    p.build_ok(&cart("  store owner: Cell[String]"));
    p.build_ok(&cart_with(
        "type CartV1 = { owner: String }",
        "  store ownerName: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    ownerName := old.owner\n  }",
    ));
    let lock = p.lock();
    assert_eq!(agent_version(&lock), "2", "{lock}");
    assert!(lock.contains("version = 2\nadditive = false"), "{lock}");
}

/// S0's rename diagnostic now names the step that would carry it.
#[test]
fn the_rename_diagnostic_suggests_a_concrete_step() {
    let p = Project::new("s2-hint");
    p.build_ok(&cart("  store owner: Cell[String]"));
    let err = p.build_err(&cart("  store ownerName: Cell[String]"));
    assert!(err.contains("type CartV1 = { owner: String }"), "{err}");
    assert!(
        err.contains("migrate from 1 (old: CartV1) { ownerName := old.owner }"),
        "{err}"
    );
}

#[test]
fn a_step_whose_old_shape_does_not_match_the_lock_is_refused() {
    let p = Project::new("s2-shape");
    p.build_ok(&cart("  store owner: Cell[String]"));
    let err = p.build_err(&cart_with(
        "type CartV1 = { owner: Int }",
        "  store ownerName: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    ownerName := \"x\"\n  }",
    ));
    assert!(err.contains("bynk.agent.migrate_shape_mismatch"), "{err}");
    assert!(err.contains("recorded it as `Cell` of `String`"), "{err}");
}

#[test]
fn a_step_that_leaves_a_removed_field_unread_is_incomplete() {
    let p = Project::new("s2-incomplete");
    p.build_ok(&cart(
        "  store owner: Cell[String]\n  store total: Cell[Int]",
    ));
    let err = p.build_err(&cart_with(
        "type CartV1 = { owner: String }",
        "  store ownerName: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    ownerName := old.owner\n  }",
    ));
    assert!(err.contains("bynk.agent.migrate_incomplete"), "{err}");
    assert!(
        err.contains("neither read (`old.f`) nor dropped: total"),
        "{err}"
    );
    // Dropping it, on purpose, completes the step.
    p.build_ok(&cart_with(
        "type CartV1 = { owner: String }",
        "  store ownerName: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    ownerName := old.owner\n    drop total\n  }",
    ));
}

#[test]
fn a_retyped_field_must_be_assigned_and_a_rekinded_set_may_be_dropped() {
    let p = Project::new("s2-retype");
    p.build_ok(&cart("  store total: Cell[Int]\n  store tags: Set[String]"));
    let err = p.build_err(&cart_with(
        "type CartV1 = { total: Int }",
        "  store total: Cell[String]\n  store tags: Cell[Int]\n\n  migrate from 1 (old: CartV1) {\n    drop tags\n  }",
    ));
    assert!(
        err.contains("changed field(s) not assigned: total"),
        "{err}"
    );
    p.build_ok(&cart_with(
        "type CartV1 = { total: Int }",
        "  store total: Cell[String]\n  store tags: Cell[Int]\n\n  migrate from 1 (old: CartV1) {\n    total := \"n\"\n    drop tags\n  }",
    ));
}

#[test]
fn a_step_from_an_unrecorded_version_is_refused() {
    let p = Project::new("s2-unknown");
    // A fresh lock baselines at version 1, so nothing older exists (F).
    let err = p.build_err(&cart_with(
        "type CartV1 = { owner: String }",
        "  store owner: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    owner := old.owner\n  }",
    ));
    assert!(err.contains("bynk.agent.migrate_unknown_version"), "{err}");
}

#[test]
fn two_steps_from_one_version_are_refused() {
    let p = Project::new("s2-dup");
    p.build_ok(&cart("  store owner: Cell[String]"));
    let err = p.build_err(&cart_with(
        "type CartV1 = { owner: String }",
        "  store ownerName: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    ownerName := old.owner\n  }\n  migrate from 1 (old: CartV1) {\n    ownerName := old.owner\n  }",
    ));
    assert!(err.contains("bynk.agent.migrate_duplicate"), "{err}");
}

/// Decision C: a step from 2 also covers version 1 when 1 -> 2 was additive,
/// so only a version below an uncovered non-additive change warns.
#[test]
fn a_step_covers_its_additive_predecessors_and_an_uncovered_version_warns() {
    let p = Project::new("s2-additive");
    p.build_ok(&cart("  store owner: Cell[String]"));
    p.build_ok(&cart(
        "  store owner: Cell[String]\n  store note: Cell[String]",
    ));
    assert_eq!(agent_version(&p.lock()), "2");
    let warnings = p.build_ok(&cart_with(
        "type CartV2 = { owner: String, note: String = \"n/a\" }",
        "  store ownerName: Cell[String]\n  store note: Cell[String]\n\n  migrate from 2 (old: CartV2) {\n    ownerName := old.owner\n  }",
    ));
    assert_eq!(agent_version(&p.lock()), "3");
    assert!(!warnings.contains("version_without_path"), "{warnings}");

    // Version 3 -> 4 is non-additive and covered only from 3: versions 1 and 2
    // still have their step from 2, so nothing warns. Dropping that step
    // leaves them without a path.
    let warnings = p.build_ok(&cart_with(
        "type CartV2 = { owner: String, note: String = \"n/a\" }\ntype CartV3 = { note: String }",
        "  store ownerName: Cell[String]\n  store memo: Cell[String]\n\n  migrate from 2 (old: CartV2) {\n    ownerName := old.owner\n    memo := old.note\n  }\n  migrate from 3 (old: CartV3) {\n    memo := old.note\n  }",
    ));
    assert_eq!(agent_version(&p.lock()), "4");
    assert!(!warnings.contains("version_without_path"), "{warnings}");
    let warnings = p.build_ok(&cart_with(
        "type CartV3 = { note: String }",
        "  store ownerName: Cell[String]\n  store memo: Cell[String]\n\n  migrate from 3 (old: CartV3) {\n    memo := old.note\n  }",
    ));
    assert!(
        warnings.contains("bynk.agent.version_without_path"),
        "{warnings}"
    );
    assert!(
        warnings.contains("stored version 1 has no path"),
        "{warnings}"
    );
}

/// Decision A: an old field matches on structure; a refinement-only
/// difference warns rather than refusing.
#[test]
fn an_old_field_differing_only_in_refinement_warns() {
    let p = Project::new("s2-refine");
    p.build_ok(&with_decl(
        "type Name = String where NonEmpty",
        &cart("  store owner: Cell[Name] = \"a\""),
    ));
    let warnings = p.build_ok(&cart_with(
        "type Name = String where NonEmpty\ntype CartV1 = { owner: String }",
        "  store ownerName: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    ownerName := old.owner\n  }",
    ));
    assert!(
        warnings.contains("bynk.agent.refinement_changed"),
        "{warnings}"
    );
    assert!(
        warnings.contains("`CartV1.owner` differs from version 1"),
        "{warnings}"
    );
}

#[test]
fn a_step_body_must_be_pure_key_free_and_assign_each_field_once() {
    let p = Project::new("s2-body");
    p.build_ok(&cart("  store owner: Cell[String]"));
    let err = p.build_err(&cart_with(
        "type CartV1 = { owner: String }",
        "  store ownerName: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    ownerName := old.owner\n    ownerName := id\n  }",
    ));
    assert!(err.contains("bynk.agent.migrate_impure"), "{err}");
    let err = p.build_err(&cart_with(
        "type CartV1 = { owner: String }",
        "  store ownerName: Cell[String]\n\n  migrate from 1 (old: CartV1) {\n    ownerName := old.owner\n    ownerName := \"b\"\n  }",
    ));
    assert!(
        err.contains("bynk.agent.migrate_duplicate_assignment"),
        "{err}"
    );
}
