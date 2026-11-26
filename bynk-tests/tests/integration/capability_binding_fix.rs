//! #3959, capability parameters, track §11: a capability parameter is bound
//! explicitly at every call and never inferred or defaulted, by the compiler
//! or by a machine-applicable fix with more than one candidate. The fix for
//! `bynk.given.binding_missing` offers one binding per instance of the kind
//! the caller holds, and `bynk check --fix` (which shares its selection rule
//! with the editor's fix-all) takes it only when there is exactly one.

use std::path::{Path, PathBuf};

use bynk_driver::fix::plan_project;

const COMMONS: &str = "commons lib.cache

consumes bynk.cloudflare { Kv }

fn forget(key: String) -> Effect[()] given Store: Kv {
\tStore.delete(key)
}
";

const CONTEXT: &str = "context shop

consumes bynk.cloudflare { Kv }
uses lib.cache

capability Sessions = Kv
capability Archive = Kv

fn one(key: String) -> Effect[()] given Sessions {
\tforget(key)
}

fn two(key: String) -> Effect[()] given Sessions, Archive {
\tforget(key)
}
";

/// A scratch project holding the commons and `context`, named for this test
/// so parallel runs never share one.
fn project(name: &str, context: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bynk-capability-binding-fix-{name}-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/lib")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"binding-fix\"\nversion = \"0.1.0\"\n\n[unstable]\nallow = [\"bynk.given.parameters\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/lib/cache.bynk"), COMMONS).unwrap();
    std::fs::write(dir.join("src/shop.bynk"), context).unwrap();
    dir
}

fn fixed_context(dir: &Path) -> Option<String> {
    let plan = plan_project(dir)
        .expect("the project sets up")
        .expect("the fixes settle");
    plan.files
        .iter()
        .find(|f| f.path.ends_with("shop.bynk"))
        .map(|f| f.fixed.clone())
}

/// §11: with one candidate the binding fix is machine-applicable, and it
/// writes the instance at the call; with two, the call is left alone. In
/// both, `bynk.given.unused_capability`'s machine-applicable removal would
/// apply to the unbound call's candidates too; it is suppressed, so every
/// instance survives the fix run in each `given` clause.
#[test]
fn binding_missing_fix_applies_only_with_exactly_one_candidate() {
    let dir = project("one-and-two", CONTEXT);
    let fixed = fixed_context(&dir).expect("the one-candidate call is fixed");
    assert!(
        fixed.contains("fn one(key: String) -> Effect[()] given Sessions {\n\tforget(key, given Store = Sessions)\n}"),
        "{fixed}"
    );
    assert!(
        fixed.contains(
            "fn two(key: String) -> Effect[()] given Sessions, Archive {\n\tforget(key)\n}"
        ),
        "a call with two candidates is never bound by a fix: {fixed}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// §11: a call with two candidates and nothing else to fix produces no edit
/// at all. The unbound call does not leave its candidates reported unused
/// either, whose machine-applicable removal would take an instance away.
#[test]
fn binding_missing_fix_leaves_a_two_candidate_call_alone() {
    let only_two = CONTEXT.replace(
        "fn one(key: String) -> Effect[()] given Sessions {\n\tforget(key)\n}\n\n",
        "",
    );
    let dir = project("two", &only_two);
    assert_eq!(fixed_context(&dir), None, "no file is changed");
    let _ = std::fs::remove_dir_all(&dir);
}
