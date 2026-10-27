//! #2471: `bynkc compile --sources-stdin` compiles unsaved buffers: the JSON
//! overlay on stdin wins over disk, its output and source maps are written to
//! `--output`, a bad overlay builds nothing, and the preview build leaves
//! `bynk.schema.lock` alone.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

const SAVED: &str = "context p.greet\n\nservice api from http {\n  on GET(\"/hello\") () -> Effect[HttpResult[String]] by v: Visitor {\n    Ok(\"saved\")\n  }\n}\n";

fn project(tag: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("sources-stdin-{tag}"));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src/p")).unwrap();
    std::fs::write(root.join("bynk.toml"), "[project]\nname = \"p\"\n").unwrap();
    std::fs::write(root.join("src/p/greet.bynk"), SAVED).unwrap();
    root.canonicalize().unwrap()
}

/// Run `bynkc compile <root> -o <out> --sources-stdin` feeding `stdin`.
fn compile(root: &Path, out: &Path, stdin: &str) -> (bool, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_bynkc"))
        .arg("compile")
        .arg(root)
        .arg("-o")
        .arg(out)
        .arg("--sources-stdin")
        .env("NO_COLOR", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("bynkc runs");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let o = child.wait_with_output().unwrap();
    (
        o.status.success(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

fn overlay(root: &Path, text: &str) -> String {
    let path = root.join("src/p/greet.bynk");
    serde_json::json!({ path.to_string_lossy(): text }).to_string()
}

#[test]
fn the_overlay_wins_over_disk_and_output_and_maps_are_written() {
    let root = project("wins");
    let out = root.join("scratch");
    let unsaved = SAVED.replace("saved", "unsaved");
    let (ok, text) = compile(&root, &out, &overlay(&root, &unsaved));
    assert!(ok, "{text}");
    let ts = std::fs::read_to_string(out.join("p/greet.ts")).expect("the module is written");
    assert!(ts.contains("unsaved") && !ts.contains("\"saved\""), "{ts}");
    let map = std::fs::read_to_string(out.join("p/greet.ts.map")).expect("its map is written");
    assert!(map.contains("greet.bynk"), "{map}");
    // The file on disk is untouched.
    assert_eq!(
        std::fs::read_to_string(root.join("src/p/greet.bynk")).unwrap(),
        SAVED
    );
}

#[test]
fn a_path_outside_the_project_builds_nothing() {
    let root = project("outside");
    let out = root.join("scratch");
    let other = std::env::temp_dir().join("elsewhere.bynk");
    let doc = serde_json::json!({ other.to_string_lossy(): "context p.x\n" }).to_string();
    let (ok, text) = compile(&root, &out, &doc);
    assert!(!ok);
    assert!(text.contains("is outside the project"), "{text}");
    assert!(!out.exists(), "nothing is written");
}

#[test]
fn a_malformed_document_is_refused() {
    let root = project("malformed");
    let (ok, text) = compile(&root, &root.join("scratch"), "[1, 2]");
    assert!(!ok);
    assert!(text.contains("JSON object"), "{text}");
}

#[test]
fn a_preview_build_leaves_the_schema_lock_alone() {
    let root = project("lock");
    let (ok, text) = compile(&root, &root.join("scratch"), &overlay(&root, SAVED));
    assert!(ok, "{text}");
    assert!(!root.join("bynk.schema.lock").exists());
}
