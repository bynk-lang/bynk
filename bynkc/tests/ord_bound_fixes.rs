//! Constrained generics slice 1: the fixes `Ord` brings, through the editor-less
//! `bynk check --fix` (#2642) path, which shares its selection rule with
//! `source.fixAll.bynk`.
//!
//! The machine-applicable ones (`missing_ord_bound`, `redundant_eq_bound`,
//! `derives_automatic`) are applied to a fixpoint; `not_orderable`'s
//! "add `derives Ord`" is `MaybeIncorrect` — it changes what the type means — so
//! a fix run never takes it.

use std::path::PathBuf;

use bynk_driver::fix::plan_file;

/// A scratch file holding `text`, named for this test file so parallel runs
/// never share one.
fn scratch(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bynk-ord-bound-fixes-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.bynk"));
    std::fs::write(&path, text).unwrap();
    path
}

fn categories(text: &str) -> Vec<&'static str> {
    bynk_ide::diagnose(text)
        .iter()
        .map(|d| d.error.category)
        .collect()
}

#[test]
fn ord_fixes_apply_to_a_fixpoint_and_the_result_is_clean() {
    let src = "commons m\n\nfn lt[T](a: T, b: T) -> Bool { a < b }\n\nfn le[T: Eq](a: T, b: T) -> Bool { a <= b }\n\nfn both[T: Eq + Ord](a: T) -> T { a }\n\ntype P = { a: Int } derives Eq\n\nfn smaller[U](a: U, b: U) -> Bool { lt(a, b) }\n";
    let path = scratch("fixpoint", src);
    let plan = plan_file(&path).unwrap().expect("fixes settle");
    let [file] = plan.files.as_slice() else {
        panic!("one file changed: {plan:?}");
    };
    let fixed = &file.fixed;
    assert!(fixed.contains("fn lt[T: Ord]"), "{fixed}");
    assert!(fixed.contains("fn le[T: Ord]"), "Eq becomes Ord: {fixed}");
    assert!(
        fixed.contains("fn both[T: Ord]"),
        "redundant Eq removed: {fixed}"
    );
    assert!(
        fixed.contains("type P = { a: Int }\n"),
        "automatic derive removed: {fixed}"
    );
    assert!(
        fixed.contains("fn smaller[U: Ord]"),
        "a chain is migrated: {fixed}"
    );
    assert!(categories(fixed).is_empty(), "{:?}", categories(fixed));
    // Stable: a second run has nothing to do.
    let again = scratch("fixpoint_again", fixed);
    assert!(plan_file(&again).unwrap().unwrap().files.is_empty());
}

#[test]
fn not_orderable_is_offered_but_never_applied_by_a_fix_run() {
    let src = "commons m\n\ntype R = { a: Int }\n\nfn lt(x: R, y: R) -> Bool { x < y }\n";
    assert!(categories(src).contains(&"bynk.types.not_orderable"));
    let path = scratch("not_orderable", src);
    let plan = plan_file(&path).unwrap().expect("nothing to refuse");
    assert!(
        plan.files.is_empty(),
        "the `derives Ord` suggestion is MaybeIncorrect, so `--fix` leaves it: {plan:?}"
    );
    assert!(!std::fs::read_to_string(&path).unwrap().contains("derives"));
}

#[test]
fn a_maybe_incorrect_fix_does_not_ride_along_with_machine_applicable_ones() {
    // One file with an automatic fix and a `not_orderable`: only the former is
    // taken, and the latter is still reported afterwards.
    let src = "commons m\n\ntype R = { a: Int }\n\nfn lt(x: R, y: R) -> Bool { x < y }\n\nfn le[T: Eq](a: T, b: T) -> Bool { a <= b }\n";
    let path = scratch("mixed", src);
    let plan = plan_file(&path).unwrap().expect("fixes settle");
    let [file] = plan.files.as_slice() else {
        panic!("one file changed: {plan:?}");
    };
    assert!(file.fixed.contains("fn le[T: Ord]"), "{}", file.fixed);
    assert!(!file.fixed.contains("derives"), "{}", file.fixed);
    assert_eq!(categories(&file.fixed), ["bynk.types.not_orderable"]);
}
