//! #3956: `bynk.schema.lock` records the order of each `derives Ord` type, and a
//! build refuses a reorder until it is accepted.
//!
//! Each test drives a scratch project through several builds, reading and writing
//! the lock the way `bynk-driver`'s production callers do, as
//! `agent_state_schema_lock.rs` does for agent state.

use bynk_driver::project::BuildTarget;
use bynk_driver::project::SchemaLock;
use std::fs;
use std::path::{Path, PathBuf};

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let root =
            std::env::temp_dir().join(format!("bynk-ord-order-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("bynk.toml"),
            "[project]\nname = \"ord-order\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\".\"]\n",
        )
        .unwrap();
        Project { root }
    }

    fn lock(&self) -> String {
        fs::read_to_string(self.root.join("bynk.schema.lock")).unwrap_or_default()
    }

    /// Build `source` as `tasks`. On success write the reconciled lock as a real
    /// caller would; on failure return the rendered errors and leave the lock.
    fn build(&self, source: &str) -> Result<(), String> {
        fs::write(self.root.join("tasks.bynk"), source).unwrap();
        let existing = bynk_driver::schema_lock::read(&self.root).unwrap();
        let options = bynk_testkit::compile_options_single(&self.root)
            .target(BuildTarget::Bundle)
            .schema_registry(SchemaLock::On { existing });
        match bynk_driver::project::compile_project(&options) {
            Ok(out) => {
                if let Some(content) = &out.schema_lock {
                    bynk_driver::schema_lock::write(&self.root, content).unwrap();
                }
                Ok(())
            }
            Err(failure) => Err(bynk_driver::render::render_project_errors(
                &failure.flatten(),
            )),
        }
    }

    fn ok(&self, source: &str) {
        self.build(source)
            .unwrap_or_else(|e| panic!("expected a clean build:\n{e}"));
    }

    fn refused(&self, source: &str) -> String {
        match self.build(source) {
            Ok(()) => panic!("expected the build to be refused"),
            Err(e) => e,
        }
    }
}

impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn priority(variants: &str) -> String {
    format!("commons tasks\n\ntype Priority = enum {{ {variants} }} derives Ord\n")
}

fn bynk(args: &[&str], root: &Path) -> (bool, String) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(args)
        .current_dir(root)
        .output()
        .expect("run bynk");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

#[test]
fn the_first_build_records_the_order_silently() {
    let p = Project::new("first");
    p.ok(&priority("Low, Medium, High"));
    let lock = p.lock();
    assert!(lock.starts_with("version = 3"), "{lock}");
    assert!(lock.contains("[orders.\"tasks.Priority\"]"), "{lock}");
    // Rebuilding the same source changes nothing.
    p.ok(&priority("Low, Medium, High"));
    assert_eq!(p.lock(), lock);
}

#[test]
fn a_project_with_no_ordered_type_keeps_its_lock_as_it_was() {
    let p = Project::new("none");
    p.ok("commons tasks\n\ntype Plain = enum { A, B }\n");
    assert!(!p.lock().contains("orders"), "{}", p.lock());
    assert!(!p.lock().contains("version = 3"), "{}", p.lock());
}

#[test]
fn reordering_variants_is_refused_and_names_both_orders() {
    let p = Project::new("swap");
    p.ok(&priority("Low, Medium, High"));
    let err = p.refused(&priority("High, Low, Medium"));
    assert!(err.contains("bynk.types.ord_order_changed"), "{err}");
    assert!(err.contains("recorded order: Low < Medium < High"), "{err}");
    assert!(err.contains("new order:      High < Low < Medium"), "{err}");
    assert!(err.contains("bynk lock accept-order Priority"), "{err}");
}

#[test]
fn reordering_or_inserting_a_field_is_refused() {
    let p = Project::new("fields");
    let record =
        |fields: &str| format!("commons tasks\n\ntype Task = {{ {fields} }} derives Ord\n");
    p.ok(&record("due: Int, title: String"));
    assert!(
        p.refused(&record("title: String, due: Int"))
            .contains("bynk.types.ord_order_changed")
    );
    assert!(
        p.refused(&record("due: Int, owner: String, title: String"))
            .contains("bynk.types.ord_order_changed")
    );
}

#[test]
fn appending_removing_and_renaming_in_place_are_allowed_and_the_lock_follows() {
    let p = Project::new("allowed");
    p.ok(&priority("Low, Medium, High"));
    p.ok(&priority("Low, Medium, High, Urgent"));
    assert!(p.lock().contains("Urgent"), "{}", p.lock());
    p.ok(&priority("Low, High, Urgent"));
    assert!(!p.lock().contains("Medium"), "{}", p.lock());
    p.ok(&priority("Low, Top, Urgent"));
    assert!(p.lock().contains("Top") && !p.lock().contains("\"High\""));
}

#[test]
fn dropping_derives_ord_removes_the_entry() {
    let p = Project::new("drop");
    p.ok(&priority("Low, High"));
    p.ok("commons tasks\n\ntype Priority = enum { Low, High }\n");
    assert!(!p.lock().contains("orders"), "{}", p.lock());
}

#[test]
fn accept_order_rewrites_the_entry_and_clears_the_error() {
    let p = Project::new("accept");
    p.ok(&priority("Low, Medium, High"));
    let _ = p.refused(&priority("High, Low, Medium"));
    // `check` is read-only and refuses too.
    let (passed, _) = bynk(&["check", "."], &p.root);
    assert!(!passed);
    let (done, out) = bynk(&["lock", "accept-order", "Priority"], &p.root);
    assert!(done, "{out}");
    assert!(
        out.contains("accepted the order of tasks.Priority"),
        "{out}"
    );
    let lock = p.lock();
    let high = lock.find("\"High\"").expect("High");
    let low = lock.find("\"Low\"").expect("Low");
    assert!(high < low, "{lock}");
    p.ok(&priority("High, Low, Medium"));
    // Accepting again says so, and `--all` lists nothing.
    let (done, out) = bynk(&["lock", "accept-order", "--all"], &p.root);
    assert!(done && out.contains("already the declared one"), "{out}");
}

#[test]
fn accept_order_names_a_type_it_cannot_find() {
    let p = Project::new("unknown");
    p.ok(&priority("Low, High"));
    let (done, out) = bynk(&["lock", "accept-order", "Nope"], &p.root);
    assert!(!done);
    assert!(out.contains("no `derives Ord` type `Nope`"), "{out}");
}
