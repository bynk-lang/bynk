//! Constrained generics slice 2: the fixes `Key` brings, through the editor-less
//! `bynk check --fix` (#2642) path, which shares its selection rule with
//! `source.fixAll.bynk`. A project written before `Key` keyed a `Map` or `Set` by a
//! bare type parameter; `missing_key_bound` is `MachineApplicable`, so a fix run
//! migrates it, and the migrated source is clean.

use std::path::PathBuf;

use bynk_driver::fix::plan_file;

/// A scratch file holding `text`, named for this test file so parallel runs
/// never share one.
fn scratch(name: &str, text: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bynk-key-bound-fixes-{}", std::process::id()));
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
fn key_fixes_apply_to_a_fixpoint_and_the_result_is_clean() {
    let src = "commons m\n\n\
        fn one[K](k: K) -> Map[K, Int] {\n  let m: Map[K, Int] = Map.empty()\n  m.insert(k, 1)\n}\n\n\
        fn keep[T: Eq](xs: Set[T]) -> Set[T] { xs }\n\n\
        fn both[T: Eq + Key](xs: Set[T]) -> Set[T] { xs }\n\n\
        fn dedup[U](xs: List[U]) -> List[U] { xs.distinct() }\n\n\
        fn outer[V](xs: Set[V]) -> Int { keep(xs).length() }\n\n\
        type Index[K, V] = { m: Map[K, V] }\n\n\
        fn Index.size(self) -> Int { self.m.length() }\n";
    let path = scratch("fixpoint", src);
    let plan = plan_file(&path).unwrap().expect("fixes settle");
    let [file] = plan.files.as_slice() else {
        panic!("one file changed: {plan:?}");
    };
    let fixed = &file.fixed;
    assert!(fixed.contains("fn one[K: Key]"), "{fixed}");
    assert!(fixed.contains("fn keep[T: Key]"), "Eq becomes Key: {fixed}");
    assert!(
        fixed.contains("fn both[T: Key]"),
        "redundant Eq removed: {fixed}"
    );
    assert!(fixed.contains("fn dedup[U: Key]"), "a body use: {fixed}");
    assert!(
        fixed.contains("fn outer[V: Key]"),
        "a chain is migrated: {fixed}"
    );
    assert!(
        fixed.contains("type Index[K: Key, V]"),
        "a generic type's own parameter: {fixed}"
    );
    assert!(categories(fixed).is_empty(), "{:?}", categories(fixed));
    // Stable: a second run has nothing to do.
    let again = scratch("fixpoint_again", fixed);
    assert!(plan_file(&again).unwrap().unwrap().files.is_empty());
}

#[test]
fn a_receiver_parameter_is_bounded_by_a_where_clause() {
    let src = "commons m\n\ntype Box[A] = { value: A }\n\nfn Box.keyed(self, s: Set[A]) -> Int { s.length() }\n";
    assert!(categories(src).contains(&"bynk.generics.missing_key_bound"));
    let path = scratch("receiver", src);
    let plan = plan_file(&path).unwrap().expect("fixes settle");
    let [file] = plan.files.as_slice() else {
        panic!("one file changed: {plan:?}");
    };
    assert!(
        file.fixed.contains("where A: Key"),
        "the method declares it: {}",
        file.fixed
    );
    assert!(
        categories(&file.fixed).is_empty(),
        "{:?}",
        categories(&file.fixed)
    );
}

#[test]
fn not_keyable_is_not_a_fix() {
    let src = "commons m\n\nfn count(m: Map[Float, Int]) -> Int { m.length() }\n";
    assert!(categories(src).contains(&"bynk.types.not_keyable"));
    let path = scratch("not_keyable", src);
    let plan = plan_file(&path).unwrap().expect("nothing to refuse");
    assert!(plan.files.is_empty(), "{plan:?}");
}

#[test]
fn the_migrated_stdlib_shape_is_what_the_fix_produces() {
    // The pre-slice-2 `bynk.set` / `bynk.map` signatures, as shipped.
    let src = "commons m\n\n\
        fn union[T](a: Set[T], b: Set[T]) -> Set[T] {\n  b.toList().fold(a, (acc, x) => acc.insert(x))\n}\n\n\
        fn mapSet[A, B](s: Set[A], f: A -> B) -> Set[B] {\n  Set.of(s.toList().map(f))\n}\n\n\
        fn values[K, V](m: Map[K, V]) -> List[V] {\n  m.keys().map((k) => m.get(k).unwrapOr(0))\n}\n";
    let path = scratch("stdlib_shape", src);
    let plan = plan_file(&path).unwrap().expect("fixes settle");
    let [file] = plan.files.as_slice() else {
        panic!("one file changed: {plan:?}");
    };
    assert!(file.fixed.contains("fn union[T: Key]("), "{}", file.fixed);
    assert!(
        file.fixed.contains("fn mapSet[A: Key, B: Key]("),
        "{}",
        file.fixed
    );
    assert!(
        file.fixed.contains("fn values[K: Key, V]("),
        "{}",
        file.fixed
    );
}
