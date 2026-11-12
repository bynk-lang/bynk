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
    ~> ping\n  ~> keep(ping)\n  ~> (ping)\n  Effect.pure(n * 2)\n}\n";

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
    assert!(!fixed.contains("~>"), "{fixed}");
    assert!(categories(fixed).is_empty(), "{:?}", categories(fixed));
    let again = scratch("sends_again", fixed);
    assert!(plan_file(&again).unwrap().unwrap().files.is_empty());
}

#[test]
fn a_refused_send_is_not_also_deprecated() {
    let src = "commons m\n\nfn label(n: Int) -> String {\n  ~> Effect.pure(())\n  \"n\"\n}\n";
    let found = categories(src);
    assert!(found.contains(&"bynk.send.in_pure_context"), "{found:?}");
    assert!(!found.contains(&"bynk.send.deprecated"), "{found:?}");
}
