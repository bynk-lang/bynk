//! #1659 (runtime-semantics track S10; closes #696's gap): every diagnostic a
//! `suite` raises carries its source file, so it renders with a file, line and
//! span like any other. Suite and integration diagnostics used to be collected
//! with no file attribution (`extend_for(None, …)`) and rendered as a bare
//! `[code] message`; they are now attributed by each error's span
//! (`ErrorSink::extend_attributed_by_span`).

use std::fs;

const COMMONS: &str = "commons demo.x\n\nfn h() -> Option[Int] { None }\nfn n() -> Int { 1 }\n";

// Several kinds of suite error: an uninferable `None`, an unknown name, a type
// mismatch, and a wrong-kind `Ok` payload.
const SUITE: &str = r#"suite demo.x

case "uninferable" {
  expect None == None
}

case "unknown name" {
  expect nope == 1
}

case "mismatch" {
  expect n() == "one"
}

case "bad ok" {
  let r: Result[Int, String] = Ok("x")
  expect r == r
}
"#;

#[test]
fn every_suite_diagnostic_is_attributed_to_its_file() {
    let root = std::env::temp_dir().join(format!("bynk-suite-attr-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/demo")).unwrap();
    fs::create_dir_all(root.join("tests/demo")).unwrap();
    fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"attr\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\", \"tests\"]\n",
    )
    .unwrap();
    fs::write(root.join("src/demo/x.bynk"), COMMONS).unwrap();
    fs::write(root.join("tests/demo/x.bynk"), SUITE).unwrap();

    let paths = bynkc::try_read_project_paths(&root).expect("well-formed manifest");
    let failure =
        match bynkc::compile_project(&bynk_testkit::compile_options_split(root.clone(), paths)) {
            Ok(_) => panic!("the suite has errors; the build must fail"),
            Err(f) => f,
        };
    let _ = fs::remove_dir_all(&root);

    assert!(
        failure.errors.len() >= 4,
        "expected at least one diagnostic per failing case, got {}: {:#?}",
        failure.errors.len(),
        failure
            .errors
            .iter()
            .map(|a| &a.error.category)
            .collect::<Vec<_>>()
    );
    for a in &failure.errors {
        let path = a.source_path.as_ref().unwrap_or_else(|| {
            panic!(
                "`{}` ({}) is not attributed to a file",
                a.error.category, a.error.message
            )
        });
        assert!(
            path.ends_with("tests/demo/x.bynk"),
            "`{}` is attributed to {path:?}, not the suite's file",
            a.error.category
        );
        assert!(
            a.error.span.end > a.error.span.start,
            "`{}` has an empty span",
            a.error.category
        );
    }
}
