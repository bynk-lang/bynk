//! #1772: `bynkc check` names a file by the path you'd type from the working
//! directory, the same path `bynkc fmt` reports. It used to print the file's
//! identity path, relative to the input it was given, so neither the GitHub
//! Actions matcher nor VS Code's (both resolving against the directory the
//! command ran in) could open it.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `<root>/test/fixtures/type-error`, a project whose `src/greeting.bynk` has
/// a type error on line 4, column 3. Returns `<root>`.
fn layout(tag: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("check-paths-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    let project = root.join("test/fixtures/type-error");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
    std::fs::write(
        project.join("src/greeting.bynk"),
        "commons greeting\n\nfn greet(name: String) -> Int {\n  name\n}\n",
    )
    .unwrap();
    root
}

/// Run `bynkc <args>` in `cwd`; the combined stdout and stderr, colour off.
fn bynkc(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc-under-test"))
        .args(args)
        .current_dir(cwd)
        .env("NO_COLOR", "1")
        .output()
        .expect("bynkc runs");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    // Strip any ANSI colour the renderer emits regardless.
    let mut plain = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c in chars.by_ref() {
                if c == 'm' {
                    break;
                }
            }
        } else {
            plain.push(c);
        }
    }
    plain
}

const FROM_PARENT: &str = "test/fixtures/type-error/src/greeting.bynk:4:3";

#[test]
fn from_a_parent_directory_the_path_includes_the_input() {
    let root = layout("parent");
    for input in ["test/fixtures/type-error", "test/fixtures/type-error/src"] {
        let short = bynkc(&root, &["check", "--format", "short", input]);
        assert!(
            short.starts_with(&format!("{FROM_PARENT}: error[")),
            "`check --format short {input}` from the parent:\n{short}"
        );
    }
    let rich = bynkc(&root, &["check", "test/fixtures/type-error"]);
    assert!(
        rich.contains(&format!("─[ {FROM_PARENT} ]")),
        "the rich header names the same path:\n{rich}"
    );
}

#[test]
fn from_inside_the_project_the_path_is_relative_to_it() {
    let root = layout("inside");
    let project = root.join("test/fixtures/type-error");
    let short = bynkc(&project, &["check", "--format", "short", "src"]);
    assert!(
        short.starts_with("src/greeting.bynk:4:3: error["),
        "`check --format short src` from inside:\n{short}"
    );
    let dot = bynkc(&project, &["check", "--format", "short", "."]);
    assert!(
        dot.starts_with("src/greeting.bynk:4:3: error["),
        "`check --format short .` from inside:\n{dot}"
    );
}

/// `fmt` and `check` name the same file identically, from the same directory.
#[test]
fn fmt_and_check_agree_on_the_path() {
    let root = layout("agree");
    let project = root.join("test/fixtures/type-error");
    std::fs::write(
        project.join("src/greeting.bynk"),
        "commons greeting\n\nfn  greet(name: String) -> Int {\n  name\n}\n",
    )
    .unwrap();
    let fmt = bynkc(&root, &["fmt", "--check", "test/fixtures/type-error"]);
    let check = bynkc(
        &root,
        &["check", "--format", "short", "test/fixtures/type-error"],
    );
    let path = "test/fixtures/type-error/src/greeting.bynk";
    assert!(
        fmt.contains(&format!("{path} is not canonically formatted")),
        "{fmt}"
    );
    assert!(check.starts_with(&format!("{path}:")), "{check}");
}

/// A failing build under `bynkc test` reports the same path.
#[test]
fn a_failing_test_build_names_the_path_as_typed() {
    let root = layout("test");
    let out = bynkc(&root, &["test", "test/fixtures/type-error"]);
    assert!(out.contains(FROM_PARENT), "{out}");
}
