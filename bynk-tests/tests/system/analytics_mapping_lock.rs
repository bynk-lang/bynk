//! #4105: `bynk.schema.lock` records the column mapping of each `Analytics` instance written with
//! `writeValue`, and a build refuses a mapping that changed until it is accepted.
//!
//! Each test drives a scratch project through several builds, reading and writing the lock the
//! way `bynk-driver`'s production callers do, as `ord_order_lock.rs` does for `derives Ord`.

use bynk_driver::project::BuildTarget;
use bynk_driver::project::SchemaLock;
use std::fs;
use std::path::{Path, PathBuf};

struct Project {
    root: PathBuf,
}

impl Project {
    fn new(name: &str) -> Self {
        let root = std::env::temp_dir().join(format!(
            "bynk-analytics-mapping-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(
            root.join("bynk.toml"),
            "[project]\nname = \"analytics-mapping\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\".\"]\n\n[unstable]\nallow = [\"bynk.cloudflare.Analytics\"]\n",
        )
        .unwrap();
        Project { root }
    }

    fn lock(&self) -> String {
        fs::read_to_string(self.root.join("bynk.schema.lock")).unwrap_or_default()
    }

    /// Build `source` as `shop.cart`. On success write the reconciled lock as a real caller would;
    /// on failure return the rendered errors and leave the lock.
    fn build(&self, source: &str) -> Result<(), String> {
        fs::create_dir_all(self.root.join("shop")).unwrap();
        fs::write(self.root.join("shop/cart.bynk"), source).unwrap();
        let existing = bynk_driver::schema_lock::read(&self.root).unwrap();
        let options = bynk_testkit::compile_options_single(&self.root)
            .target(BuildTarget::Workers)
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

/// A context writing `Signup { <fields> }` to `Signups`; `value` is the literal it writes.
fn cart(fields: &str, value: &str) -> String {
    format!(
        "context shop.cart\n\nconsumes bynk\nconsumes bynk.cloudflare {{ Analytics }}\n\n\
         type Signup = {{ {fields} }}\n\ncapability Signups = Analytics\n\n\
         service api from http {{\n\
         \ton POST(\"/signup\") () -> Effect[HttpResult[String]] by Visitor given Signups {{\n\
         \t\tlet _ <- Signups.writeValue(Signup {{ {value} }})\n\
         \t\tCreated(\"counted\")\n\
         \t}}\n}}\n"
    )
}

const FIELDS: &str =
    "@index plan: String, country: String, channel: String, seats: Int, amount: Float";
const VALUE: &str = "plan: \"a\", country: \"GB\", channel: \"web\", seats: 1, amount: 1.5";

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
fn the_first_build_records_the_mapping_silently() {
    let p = Project::new("first");
    p.ok(&cart(FIELDS, VALUE));
    let lock = p.lock();
    assert!(lock.starts_with("version = 4"), "{lock}");
    assert!(lock.contains("[analytics.\"shop.cart.Signups\"]"), "{lock}");
    assert!(lock.contains("index = \"plan\""), "{lock}");
    assert!(
        lock.contains("\"country:text\"") && lock.contains("\"seats:number\""),
        "{lock}"
    );
    // Rebuilding the same source changes nothing.
    p.ok(&cart(FIELDS, VALUE));
    assert_eq!(p.lock(), lock);
}

#[test]
fn a_project_that_writes_no_typed_point_keeps_its_lock_as_it_was() {
    let p = Project::new("none");
    p.ok("context shop.cart\n\ntype Plain = { a: Int }\n");
    assert!(!p.lock().contains("analytics"), "{}", p.lock());
    assert!(!p.lock().contains("version = 4"), "{}", p.lock());
}

#[test]
fn reordering_fields_is_refused_and_names_both_mappings() {
    let p = Project::new("swap");
    p.ok(&cart(FIELDS, VALUE));
    let err = p.refused(&cart(
        "@index plan: String, channel: String, country: String, seats: Int, amount: Float",
        VALUE,
    ));
    assert!(err.contains("bynk.analytics.mapping_changed"), "{err}");
    assert!(err.contains("blob1 = country, blob2 = channel"), "{err}");
    assert!(err.contains("blob1 = channel, blob2 = country"), "{err}");
    assert!(
        err.contains("bynk lock accept-analytics shop.cart.Signups"),
        "{err}"
    );
}

#[test]
fn retyping_or_removing_a_field_is_refused() {
    let p = Project::new("retype");
    p.ok(&cart(FIELDS, VALUE));
    // `seats` from a number to text moves it from `double1` to a blob.
    let retyped = p.refused(&cart(
        "@index plan: String, country: String, channel: String, seats: String, amount: Float",
        "plan: \"a\", country: \"GB\", channel: \"web\", seats: \"1\", amount: 1.5",
    ));
    assert!(
        retyped.contains("bynk.analytics.mapping_changed"),
        "{retyped}"
    );
    let removed = p.refused(&cart(
        "@index plan: String, channel: String, seats: Int, amount: Float",
        "plan: \"a\", channel: \"web\", seats: 1, amount: 1.5",
    ));
    assert!(
        removed.contains("bynk.analytics.mapping_changed"),
        "{removed}"
    );
}

#[test]
fn appending_and_renaming_in_place_are_allowed_and_the_lock_follows() {
    let p = Project::new("allowed");
    p.ok(&cart(FIELDS, VALUE));
    p.ok(&cart(
        &format!("{FIELDS}, referrer: String"),
        &format!("{VALUE}, referrer: \"x\""),
    ));
    assert!(p.lock().contains("referrer:text"), "{}", p.lock());
    p.ok(&cart(
        "@index plan: String, nation: String, channel: String, seats: Int, amount: Float, referrer: String",
        "plan: \"a\", nation: \"GB\", channel: \"web\", seats: 1, amount: 1.5, referrer: \"x\"",
    ));
    assert!(p.lock().contains("nation:text"), "{}", p.lock());
}

#[test]
fn an_instance_no_longer_written_drops_its_entry() {
    let p = Project::new("drop");
    p.ok(&cart(FIELDS, VALUE));
    p.ok("context shop.cart\n\ntype Plain = { a: Int }\n");
    assert!(!p.lock().contains("analytics"), "{}", p.lock());
}

#[test]
fn accept_analytics_rewrites_the_entry_and_clears_the_error() {
    let p = Project::new("accept");
    p.ok(&cart(FIELDS, VALUE));
    let swapped =
        "@index plan: String, channel: String, country: String, seats: Int, amount: Float";
    let _ = p.refused(&cart(swapped, VALUE));
    // `check` is read-only and refuses too.
    let (passed, _) = bynk(&["check", "."], &p.root);
    assert!(!passed);
    let (done, out) = bynk(&["lock", "accept-analytics", "Signups"], &p.root);
    assert!(done, "{out}");
    assert!(
        out.contains("accepted the mapping of shop.cart.Signups"),
        "{out}"
    );
    let lock = p.lock();
    let channel = lock.find("\"channel:text\"").expect("channel");
    let country = lock.find("\"country:text\"").expect("country");
    assert!(channel < country, "{lock}");
    p.ok(&cart(swapped, VALUE));
    // Accepting again says so, and `--all` lists nothing.
    let (done, out) = bynk(&["lock", "accept-analytics", "--all"], &p.root);
    assert!(done && out.contains("already the declared one"), "{out}");
}

#[test]
fn accept_analytics_names_an_instance_it_cannot_find() {
    let p = Project::new("unknown");
    p.ok(&cart(FIELDS, VALUE));
    let (done, out) = bynk(&["lock", "accept-analytics", "Nope"], &p.root);
    assert!(!done);
    assert!(out.contains("no `Analytics` instance `Nope`"), "{out}");
}
