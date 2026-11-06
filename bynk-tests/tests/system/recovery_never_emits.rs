//! #1663: checking now continues past a resolve error and past a syntax error
//! (a recovering parse), so partial programs flow further than they did. None
//! of that may reach emission: a program with any error-severity diagnostic
//! still produces no output, from `compile`, `compile_project`, or
//! `bynkc test`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const RESOLVE_ERROR: &str = "commons demo\n\nfn a() -> Int { 1 }\n\nfn b() -> Int { nope }\n";
const SYNTAX_ERROR: &str = "commons demo\n\nfn a() -> Int { 1 +  }\n\nfn b() -> Int { 2 }\n";
const CLEAN: &str = "commons demo\n\nfn a() -> Int { 1 }\n";
const SUITE: &str = "suite demo\n\ncase \"a is one\" {\n  expect a() == 1\n}\n";

fn project(name: &str, source: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("recovery-never-emits")
        .join(name);
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    fs::write(
        root.join("bynk.toml"),
        "[project]\nname = \"recovery\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\", \"tests\"]\n",
    )
    .unwrap();
    fs::write(root.join("src/demo.bynk"), source).unwrap();
    fs::write(root.join("tests/demo.bynk"), SUITE).unwrap();
    root
}

fn compiles(root: &Path) -> bool {
    let paths = bynk_driver::project::try_read_project_paths(root).expect("well-formed manifest");
    bynk_driver::project::compile_project(&bynk_testkit::compile_options_split(
        root.to_path_buf(),
        paths,
    ))
    .is_ok()
}

#[test]
fn single_file_compile_refuses_a_partial_program() {
    assert!(bynk_emit::compile(RESOLVE_ERROR, "demo.bynk").is_err());
    assert!(bynk_emit::compile(SYNTAX_ERROR, "demo.bynk").is_err());
}

#[test]
fn project_compile_refuses_a_partial_program() {
    // The control: the same project without the error compiles, so a refusal
    // below is the error's doing, not the harness's.
    assert!(compiles(&project("clean", CLEAN)));
    assert!(!compiles(&project("resolve", RESOLVE_ERROR)));
    assert!(!compiles(&project("syntax", SYNTAX_ERROR)));
}

#[test]
fn bynkc_test_runs_nothing_for_a_partial_program() {
    for (name, source) in [
        ("test-resolve", RESOLVE_ERROR),
        ("test-syntax", SYNTAX_ERROR),
    ] {
        let root = project(name, source);
        let out_dir = root.join("out");
        let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
            .arg("test")
            .arg(&root)
            .arg("--output")
            .arg(&out_dir)
            .output()
            .expect("run bynkc test");
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(!out.status.success(), "{name}: `bynkc test` must fail");
        assert!(
            !stdout.contains("passed"),
            "{name}: no case may run:\n{stdout}"
        );
        assert!(!out_dir.exists(), "{name}: nothing may be emitted");
    }
}

/// #1710: a file the strict parse rejects now has its surviving declarations
/// checked on the project path. That must not let any of the project reach
/// emission: a clean sibling unit next to a broken file is still refused, by
/// `compile_project` and by `bynkc test` (which runs no case and writes
/// nothing). The clean sibling alone compiles, so a refusal is the broken
/// file's doing.
#[test]
fn a_clean_sibling_of_a_partially_parsed_file_is_not_emitted() {
    let build = |name: &str, with_broken: bool| -> PathBuf {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join("recovery-never-emits")
            .join(name);
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(root.join("src")).unwrap();
        fs::create_dir_all(root.join("tests")).unwrap();
        fs::write(
            root.join("bynk.toml"),
            "[project]\nname = \"recovery\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\", \"tests\"]\n",
        )
        .unwrap();
        fs::write(
            root.join("src/ok.bynk"),
            "commons ok\n\nfn a() -> Int { 1 }\n",
        )
        .unwrap();
        if with_broken {
            fs::write(
                root.join("src/bad.bynk"),
                "commons bad\n\ntype Pair = { first: Int second: Int }\n\nfn b() -> Int { 2 }\n",
            )
            .unwrap();
        }
        fs::write(
            root.join("tests/ok.bynk"),
            "suite ok\n\ncase \"a is one\" {\n  expect a() == 1\n}\n",
        )
        .unwrap();
        root
    };
    assert!(compiles(&build("sibling-clean", false)));
    let root = build("sibling-broken", true);
    assert!(
        !compiles(&root),
        "a project with a syntax error must not compile"
    );

    let out_dir = root.join("out");
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .arg("test")
        .arg(&root)
        .arg("--output")
        .arg(&out_dir)
        .output()
        .expect("run bynkc test");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!out.status.success(), "`bynkc test` must fail");
    assert!(!stdout.contains("passed"), "no case may run:\n{stdout}");
    assert!(!out_dir.exists(), "nothing may be emitted");
}
