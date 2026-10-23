//! #2387: `embed` declarations through the real CLI (slice 1: checked, not
//! yet built).
//!
//! - A single file and a project check clean (`bynkc check`), with the embed's
//!   name typed as a `String` in a function body.
//! - Their builds are refused with `bynk.embed.not_yet_emitted`: nothing emits
//!   an embed yet, so a build must not produce a module whose uses of the name
//!   are undefined. The editor and `check` stay clean.
//!
//! Each case works in its own directory under `CARGO_TARGET_TMPDIR`: the CLI
//! may write a schema lock beside its input.

use std::path::{Path, PathBuf};
use std::process::Command;

const PAGE: &str = "commons app.page\n\nembed shell = \"embeds/page.html\"\n\nfn page(content: String) -> String {\n  shell.replace(\"{{content}}\", content)\n}\n";
const HTML: &[u8] = b"<p>{{content}}</p>\n";

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn project(name: &str) -> PathBuf {
    let dir = scratch(name);
    std::fs::create_dir_all(dir.join("src/app")).unwrap();
    std::fs::write(dir.join("src/app/page.bynk"), PAGE).unwrap();
    std::fs::create_dir_all(dir.join("embeds")).unwrap();
    std::fs::write(dir.join("embeds/page.html"), HTML).unwrap();
    dir
}

fn bynkc(args: &[&str], path: &Path) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .args(args)
        .arg(path)
        .output()
        .expect("run bynkc");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

#[test]
fn a_single_file_with_an_embed_checks_clean_and_its_build_is_refused() {
    let dir = scratch("embed-check-single");
    let file = dir.join("page.bynk");
    std::fs::write(&file, PAGE).unwrap();

    let (ok, out) = bynkc(&["check"], &file);
    assert!(ok, "`bynkc check` must pass, got:\n{out}");

    let ts = dir.join("page.ts");
    let (ok, out) = bynkc(&["compile", "--output", ts.to_str().unwrap()], &file);
    assert!(!ok, "`bynkc compile` must refuse the embed, got:\n{out}");
    assert!(out.contains("bynk.embed.not_yet_emitted"), "{out}");
}

#[test]
fn a_project_with_an_embed_checks_clean_and_its_build_is_refused() {
    let dir = project("embed-check-project");
    let (ok, out) = bynkc(&["check"], &dir);
    assert!(ok, "`bynkc check` on a project must pass, got:\n{out}");

    let (ok, out) = bynkc(
        &["compile", "--output", dir.join("out").to_str().unwrap()],
        &dir,
    );
    assert!(!ok, "`bynkc compile` must refuse the embed, got:\n{out}");
    assert!(out.contains("bynk.embed.not_yet_emitted"), "{out}");
}

#[test]
fn an_embed_is_a_string_not_an_int() {
    let dir = project("embed-check-type");
    std::fs::write(
        dir.join("src/app/page.bynk"),
        "commons app.page\n\nembed shell = \"embeds/page.html\"\n\nfn n() -> Int {\n  shell\n}\n",
    )
    .unwrap();
    let (ok, out) = bynkc(&["check"], &dir);
    assert!(!ok, "an embed returned as an Int must fail, got:\n{out}");
    assert!(out.contains("bynk.types.return_mismatch"), "{out}");
}
