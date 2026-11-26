//! #3793: `field: T replaces old` against `bynk.schema.lock`.
//!
//! Each test drives a scratch project through several builds, reading and writing the lock the way
//! `bynk-driver`'s production callers do, as `agent_state_schema_lock.rs` and
//! `events_schema_registry_behaviour.rs` do. The runtime half (the old key read at load) is pinned
//! in `field_rename_behaviour.rs`.

use bynk_driver::project::BuildTarget;
use bynk_driver::project::SchemaLock;
use std::fs;
use std::path::PathBuf;

/// A scratch project with one `.bynk` source and its lock.
struct Project {
    root: PathBuf,
    file: &'static str,
}

impl Project {
    fn new(name: &str, file: &'static str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bynk-field-rename-lock-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("shop")).unwrap();
        Project { root, file }
    }

    fn lock(&self) -> String {
        fs::read_to_string(self.root.join("bynk.schema.lock")).unwrap()
    }

    /// Build `source`. On success, write the reconciled lock and return the rendered warnings;
    /// on failure, return the rendered errors and leave the lock alone.
    fn build(&self, source: &str) -> Result<String, String> {
        fs::write(self.root.join(self.file), source).unwrap();
        let existing = bynk_driver::schema_lock::read(&self.root).unwrap();
        let options = bynk_testkit::compile_options_single(&self.root)
            .target(BuildTarget::Bundle)
            .schema_registry(SchemaLock::On { existing });
        match bynk_driver::project::compile_project(&options) {
            Ok(out) => {
                if let Some(content) = &out.schema_lock {
                    bynk_driver::schema_lock::write(&self.root, content).unwrap();
                }
                let warnings: Vec<bynk_syntax::CompileError> =
                    out.warnings.iter().map(|w| w.error.clone()).collect();
                Ok(bynk_driver::render::render_project_errors(&warnings))
            }
            Err(failure) => Err(bynk_driver::render::render_project_errors(
                &failure.flatten(),
            )),
        }
    }

    fn ok(&self, source: &str) -> String {
        self.build(source)
            .unwrap_or_else(|e| panic!("expected a clean build:\n{e}"))
    }

    fn err(&self, source: &str) -> String {
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

// ---- events ---------------------------------------------------------------

/// An event `Paid` with `fields`, and a service that emits it with `values` (one per
/// non-defaulted field).
fn event(fields: &str, values: &str) -> String {
    format!(
        "context shop\n\nconsumes bynk {{ Events }}\n\nevent Paid = {{\n{fields}\n}}\n\nservice pay {{\n  on call(id: String) -> Effect[()] by Caller given Events {{\n    Events.emit[Paid](Paid {{ {values} }})\n  }}\n}}\n"
    )
}

fn event_project(name: &str) -> Project {
    Project::new(name, "shop/pay.bynk")
}

/// The `schema = N` of the lock's only event.
fn schema(lock: &str) -> &str {
    lock.lines()
        .find(|l| l.starts_with("schema = "))
        .expect("an event entry")
        .trim_start_matches("schema = ")
}

#[test]
fn a_renamed_event_field_is_additive_and_recorded() {
    let p = event_project("event-additive");
    p.ok(&event("  id: String,\n  amount: Int,", "id: id, amount: 1"));
    assert_eq!(schema(&p.lock()), "1");

    // Emitting needs the new name; the rename is `amount` to `total`.
    p.ok(&event(
        "  id: String,\n  total: Int replaces amount,",
        "id: id, total: 1",
    ));
    let lock = p.lock();
    assert_eq!(schema(&lock), "2", "a rename is a step: {lock}");
    assert!(lock.contains("renamed_from = [\"amount\"]"), "{lock}");
    assert!(
        !lock.contains("name = \"amount\""),
        "the old name is no longer a field: {lock}"
    );

    // The clause stays in source: nothing changes.
    p.ok(&event(
        "  id: String,\n  total: Int replaces amount,",
        "id: id, total: 1",
    ));
    assert_eq!(schema(&p.lock()), "2");

    // The clause is deleted: the recorded history stays, and nothing is a change.
    p.ok(&event("  id: String,\n  total: Int,", "id: id, total: 1"));
    let lock = p.lock();
    assert_eq!(schema(&lock), "2", "{lock}");
    assert!(lock.contains("renamed_from = [\"amount\"]"), "{lock}");
}

#[test]
fn a_rename_beside_a_retype_is_still_refused() {
    let p = event_project("event-retype");
    p.ok(&event("  id: String,\n  amount: Int,", "id: id, amount: 1"));
    let before = p.lock();
    let err = p.err(&event(
        "  id: String,\n  total: String replaces amount,",
        "id: id, total: \"x\"",
    ));
    assert!(
        err.contains("bynk.event.non_additive_schema_change"),
        "{err}"
    );
    assert!(err.contains("retyped"), "{err}");
    assert_eq!(p.lock(), before, "a refused build must not touch the lock");
}

#[test]
fn an_event_rename_beside_a_defaulted_addition_is_additive() {
    let p = event_project("event-and-added");
    p.ok(&event("  id: String,\n  amount: Int,", "id: id, amount: 1"));
    p.ok(&event(
        "  id: String,\n  total: Int replaces amount,\n  region: String = \"EU\",",
        "id: id, total: 1",
    ));
    assert_eq!(schema(&p.lock()), "2");
}

#[test]
fn an_event_field_replacing_an_unrecorded_name_is_refused() {
    let p = event_project("event-unknown");
    p.ok(&event("  id: String,\n  amount: Int,", "id: id, amount: 1"));
    let err = p.err(&event(
        "  id: String,\n  total: Int replaces amout,",
        "id: id, total: 1",
    ));
    assert!(err.contains("bynk.field.replaces_unknown"), "{err}");
    assert!(err.contains("records no field `amout`"), "{err}");
}

#[test]
fn an_event_rename_onto_a_recorded_field_is_refused() {
    let p = event_project("event-both");
    p.ok(&event(
        "  id: String,\n  amount: Int,\n  total: Int,",
        "id: id, amount: 1, total: 1",
    ));
    let err = p.err(&event(
        "  id: String,\n  total: Int replaces amount,",
        "id: id, total: 1",
    ));
    assert!(err.contains("bynk.field.replaces_conflict"), "{err}");
    assert!(err.contains("records both"), "{err}");
}

// ---- agent state ------------------------------------------------------------

fn cart(fields: &str) -> String {
    format!(
        "context shop\n\nagent Cart {{\n  key id: String\n\n{fields}\n\n  on call ping() -> Effect[Int] {{\n    1\n  }}\n}}\n"
    )
}

fn agent_project(name: &str) -> Project {
    Project::new(name, "shop/cart.bynk")
}

fn agent_version(lock: &str) -> &str {
    let at = lock.find("[agents.\"shop.Cart\"]").expect("a Cart entry");
    lock[at..]
        .lines()
        .find(|l| l.starts_with("version = "))
        .expect("a version line")
        .trim_start_matches("version = ")
}

#[test]
fn a_renamed_store_field_is_additive_and_recorded() {
    let p = agent_project("store-additive");
    p.ok(&cart("  store customer: Cell[String]"));
    assert_eq!(agent_version(&p.lock()), "1");

    p.ok(&cart("  store owner: Cell[String] replaces customer"));
    let lock = p.lock();
    assert_eq!(agent_version(&lock), "2", "a rename is a step: {lock}");
    assert!(lock.contains("renamed_from = [\"customer\"]"), "{lock}");
    // Version 1 still records the old name: a record at it is read through the clause.
    assert!(lock.contains("name = \"customer\""), "{lock}");

    // The clause stays: no change.
    p.ok(&cart("  store owner: Cell[String] replaces customer"));
    assert_eq!(agent_version(&p.lock()), "2");
}

#[test]
fn a_store_rename_beside_a_retype_is_still_refused() {
    let p = agent_project("store-retype");
    p.ok(&cart("  store customer: Cell[String]"));
    let before = p.lock();
    let err = p.err(&cart("  store owner: Cell[Int] replaces customer"));
    assert!(
        err.contains("bynk.agent.non_additive_state_change"),
        "{err}"
    );
    assert_eq!(p.lock(), before, "a refused build must not touch the lock");
}

#[test]
fn deleting_the_clause_while_an_older_version_is_held_is_refused() {
    let p = agent_project("store-needed");
    p.ok(&cart("  store customer: Cell[String]"));
    p.ok(&cart("  store owner: Cell[String] replaces customer"));
    let before = p.lock();
    let err = p.err(&cart("  store owner: Cell[String]"));
    assert!(err.contains("bynk.field.replaces_needed"), "{err}");
    assert!(err.contains("still holds"), "{err}");
    assert_eq!(p.lock(), before, "a refused build must not touch the lock");
}

#[test]
fn a_clause_no_held_version_needs_warns_that_it_can_go() {
    let p = agent_project("store-settled");
    p.ok(&cart("  store customer: Cell[String]"));
    p.ok(&cart("  store owner: Cell[String] replaces customer"));

    // `bynk migrate` raises the floor past version 1, the only version that held `customer`.
    let mut lock =
        bynk_project::schema_registry::parse(Some(&p.lock()), &p.root).expect("the lock parses");
    lock.raise_floor("shop.Cart", 2).unwrap();
    bynk_driver::schema_lock::write(&p.root, &bynk_project::schema_registry::serialize(&lock))
        .unwrap();

    let warnings = p.ok(&cart("  store owner: Cell[String] replaces customer"));
    assert!(
        warnings.contains("bynk.field.replaces_settled"),
        "{warnings}"
    );
    assert!(warnings.contains("delete `replaces`"), "{warnings}");

    // And deleting it now is accepted.
    p.ok(&cart("  store owner: Cell[String]"));
}

#[test]
fn a_store_field_replacing_an_unrecorded_name_is_refused() {
    let p = agent_project("store-unknown");
    p.ok(&cart("  store customer: Cell[String]"));
    let err = p.err(&cart("  store owner: Cell[String] replaces custmer"));
    assert!(err.contains("bynk.field.replaces_unknown"), "{err}");
}

#[test]
fn a_store_rename_onto_a_recorded_field_is_refused() {
    let p = agent_project("store-both");
    p.ok(&cart(
        "  store customer: Cell[String]\n  store owner: Cell[String]",
    ));
    let err = p.err(&cart("  store owner: Cell[String] replaces customer"));
    assert!(err.contains("bynk.field.replaces_conflict"), "{err}");
}

#[test]
fn a_collection_cannot_be_renamed_with_replaces() {
    let p = agent_project("store-collection");
    p.ok(&cart("  store seen: Map[String, Int]"));
    let err = p.err(&cart("  store visited: Map[String, Int] replaces seen"));
    assert!(err.contains("bynk.store.replaces_collection"), "{err}");
}

#[test]
fn the_non_additive_refusal_names_replaces_for_a_cell() {
    let p = agent_project("store-hint");
    p.ok(&cart("  store owner: Cell[String]"));
    let err = p.err(&cart("  store ownerName: Cell[String]"));
    assert!(err.contains("replaces owner"), "{err}");
}

// ---- a record field inside a stored value (#4483) ---------------------------

/// A `Prefs` record with `width_field`, stored in a `Cell` and in a `Map` value.
fn prefs_cart(width_field: &str) -> String {
    format!(
        "context shop\n\ntype Prefs = {{ {width_field} }}\n\nagent Cart {{\n  key id: String\n\n  store prefs: Cell[Prefs]\n  store by_name: Map[String, Prefs]\n\n  on call ping() -> Effect[Int] {{\n    1\n  }}\n}}\n"
    )
}

#[test]
fn a_record_field_renamed_inside_a_stored_value_needs_no_migration() {
    let p = agent_project("nested-rename");
    p.ok(&prefs_cart("width: Int"));
    assert_eq!(agent_version(&p.lock()), "1");

    // No `migrate from`: the stored `{ width }` decodes through the clause.
    p.ok(&prefs_cart("size: Int replaces width"));
    let lock = p.lock();
    assert_eq!(agent_version(&lock), "2", "a rename is a step: {lock}");
    assert!(lock.contains("renamed_from = \"width\""), "{lock}");

    // The clause stays in source: nothing changes.
    p.ok(&prefs_cart("size: Int replaces width"));
    assert_eq!(agent_version(&p.lock()), "2");
}

#[test]
fn a_nested_rename_beside_a_retype_or_without_a_clause_is_still_refused() {
    let p = agent_project("nested-refused");
    p.ok(&prefs_cart("width: Int"));
    let before = p.lock();
    // A rename that also retypes is a retype.
    let err = p.err(&prefs_cart("size: String replaces width"));
    assert!(
        err.contains("bynk.agent.non_additive_state_change"),
        "{err}"
    );
    // No clause: still a removal and an addition.
    let err = p.err(&prefs_cart("size: Int"));
    assert!(
        err.contains("bynk.agent.non_additive_state_change"),
        "{err}"
    );
    assert_eq!(p.lock(), before, "a refused build must not touch the lock");
}

#[test]
fn deleting_a_nested_clause_while_an_older_version_is_held_is_refused() {
    // #4483: the guard `replaces_needed` gives a `Cell`, for a record field inside a stored value.
    let p = agent_project("nested-needed");
    p.ok(&prefs_cart("width: Int"));
    p.ok(&prefs_cart("size: Int replaces width"));
    let before = p.lock();
    let err = p.err(&prefs_cart("size: Int"));
    assert!(err.contains("bynk.field.replaces_needed"), "{err}");
    assert!(err.contains("renamed from `width`"), "{err}");
    assert_eq!(p.lock(), before, "a refused build must not touch the lock");
}

#[test]
fn a_nested_clause_no_held_version_needs_warns_that_it_can_go() {
    let p = agent_project("nested-settled");
    p.ok(&prefs_cart("width: Int"));
    // While version 1 is held, the clause is needed and says nothing.
    let held = p.ok(&prefs_cart("size: Int replaces width"));
    assert!(!held.contains("replaces_settled"), "{held}");

    // `bynk migrate` raises the floor past version 1, the only version that held `width`.
    let mut lock =
        bynk_project::schema_registry::parse(Some(&p.lock()), &p.root).expect("the lock parses");
    lock.raise_floor("shop.Cart", 2).unwrap();
    bynk_driver::schema_lock::write(&p.root, &bynk_project::schema_registry::serialize(&lock))
        .unwrap();

    let warnings = p.ok(&prefs_cart("size: Int replaces width"));
    assert!(
        warnings.contains("bynk.field.replaces_settled"),
        "{warnings}"
    );
    // And deleting it now is accepted.
    p.ok(&prefs_cart("size: Int"));
}
