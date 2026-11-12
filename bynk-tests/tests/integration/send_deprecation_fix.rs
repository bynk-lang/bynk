//! #3437: `~> e` is deprecated for `do e.detach()`. The warning is
//! `MachineApplicable`, so `bynk check --fix` (#2642) rewrites each send, and the
//! rewritten source is clean.

use std::path::PathBuf;

use bynk_driver::fix::plan_file;

fn scratch(name: &str, text: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("bynk-send-deprecation-fix-{}", std::process::id()));
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

const SRC: &str = "commons m\n\n\
    fn keep(e: Effect[()]) -> Effect[()] { e }\n\n\
    fn twice(ping: Effect[()], n: Int) -> Effect[Int] {\n  \
    ~> ping\n  ~> keep(ping)\n  ~> (ping)\n  Effect.pure(n * 2)\n}\n\n\
    fn branch(c: Bool, ping: Effect[()]) -> Effect[()] {\n  \
    ~> if c { ping } else { keep(ping) }\n  \
    ~> match c {\n    true => ping\n    false => keep(ping)\n  }\n  Effect.pure(())\n}\n";

#[test]
fn each_send_is_flagged_and_fixed_to_do_detach() {
    let path = scratch("sends", SRC);
    let plan = plan_file(&path).unwrap().expect("fixes settle");
    let [file] = plan.files.as_slice() else {
        panic!("one file changed: {plan:?}");
    };
    let fixed = &file.fixed;
    assert!(fixed.contains("do ping.detach()"), "{fixed}");
    assert!(fixed.contains("do keep(ping).detach()"), "{fixed}");
    assert!(fixed.contains("do (ping).detach()"), "{fixed}");
    // A non-postfix operand is parenthesised, and the result still parses.
    assert!(
        fixed.contains("do (if c { ping } else { keep(ping) }).detach()"),
        "{fixed}"
    );
    assert!(fixed.contains("do (match c {"), "{fixed}");
    assert!(fixed.contains("}).detach()"), "{fixed}");
    assert!(!fixed.contains("~>"), "{fixed}");
    assert!(categories(fixed).is_empty(), "{:?}", categories(fixed));
    let again = scratch("sends_again", fixed);
    assert!(plan_file(&again).unwrap().unwrap().files.is_empty());
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

/// A warning raised in the operand does not hide the deprecation: `todo()` warns
/// (`bynk.fault.todo`), and the send is still rewritten.
#[test]
fn a_warning_in_the_operand_does_not_hide_the_deprecation() {
    let src = "commons m\n\nfn keep(e: Effect[()]) -> Effect[()] { e }\n\nfn later() -> Effect[Int] {\n  ~> keep(todo(\"later\"))\n  Effect.pure(1)\n}\n";
    let path = scratch("todo_operand", src);
    let plan = plan_file(&path).unwrap().expect("fixes settle");
    let [file] = plan.files.as_slice() else {
        panic!("one file changed: {plan:?}");
    };
    assert!(
        file.fixed.contains("do keep(todo(\"later\")).detach()"),
        "{}",
        file.fixed
    );
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn a_refused_send_is_not_also_deprecated() {
    let src = "commons m\n\nfn label(n: Int) -> String {\n  ~> Effect.pure(())\n  \"n\"\n}\n";
    let found = categories(src);
    assert!(found.contains(&"bynk.send.in_pure_context"), "{found:?}");
    assert!(!found.contains(&"bynk.send.deprecated"), "{found:?}");
}
