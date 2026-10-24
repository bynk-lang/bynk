//! #2387: `embed` declarations through the real CLI.
//!
//! - A single file and a project check clean (`bynkc check`), with the embed's
//!   name typed as a `String` in a function body.
//! - A single file's build is refused with `bynk.embed.needs_project`: it has
//!   no project root to find `embeds/` under.
//! - A project builds: the file's text is an exported `string` constant of the
//!   declaring module, with CRLF line endings and a byte-order mark
//!   normalised away, so the same source builds to the same constant.
//! - The file's own rules are checked, not found at runtime: a missing file, a
//!   file that is not UTF-8, one with a NUL byte, and one over the size cap.
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
    assert!(out.contains("bynk.embed.needs_project"), "{out}");
}

#[test]
fn a_project_with_an_embed_builds_the_files_text_as_a_constant() {
    let dir = project("embed-build-project");
    // CRLF endings and a byte-order mark, as a Windows checkout might have.
    std::fs::write(
        dir.join("embeds/page.html"),
        b"\xef\xbb\xbf<p>{{content}}</p>\r\n<!-- \"q\" \\ -->\r\n",
    )
    .unwrap();
    let (ok, out) = bynkc(&["check"], &dir);
    assert!(ok, "`bynkc check` on a project must pass, got:\n{out}");

    let out_dir = dir.join("out");
    let (ok, out) = bynkc(&["compile", "--output", out_dir.to_str().unwrap()], &dir);
    assert!(ok, "`bynkc compile` must build the embed, got:\n{out}");
    let ts = std::fs::read_to_string(out_dir.join("app/page.ts")).unwrap();
    assert!(
        ts.contains(r#"export const shell: string = "<p>{{content}}</p>\n<!-- \"q\" \\ -->\n";"#),
        "{ts}"
    );
}

/// Each case writes one bad file and the project must say why, at check time.
#[test]
fn a_bad_embedded_file_is_refused_with_its_reason() {
    let cases: [(&str, &[u8], &str); 4] = [
        (
            "embed-bad-utf8",
            b"ok \xff\xfe",
            "bynk.embed.invalid_encoding",
        ),
        ("embed-bad-nul", b"a\0b", "bynk.embed.binary_content"),
        (
            "embed-bad-size",
            &[b'a'; 256 * 1024 + 1],
            "bynk.embed.too_large",
        ),
        ("embed-bad-missing", b"", "bynk.embed.missing_file"),
    ];
    for (name, bytes, code) in cases {
        let dir = project(name);
        if code == "bynk.embed.missing_file" {
            std::fs::remove_file(dir.join("embeds/page.html")).unwrap();
        } else {
            std::fs::write(dir.join("embeds/page.html"), bytes).unwrap();
        }
        let (ok, out) = bynkc(&["check"], &dir);
        assert!(!ok, "{name} must fail the check, got:\n{out}");
        assert!(out.contains(code), "{name} must report {code}, got:\n{out}");
    }
}

/// The project-wide cap counts each declared file once.
#[test]
fn the_declared_embeds_together_have_a_size_cap() {
    let dir = project("embed-total-size");
    let mut decls = String::from("commons app.more\n\n");
    for n in 0..5 {
        // 5 x 250 KiB is over the 1 MiB total, though each is under the 256 KiB cap.
        std::fs::write(dir.join(format!("embeds/f{n}.txt")), vec![b'a'; 250 * 1024]).unwrap();
        decls.push_str(&format!("embed f{n} = \"embeds/f{n}.txt\"\n"));
    }
    std::fs::write(dir.join("src/app/more.bynk"), decls).unwrap();
    let (ok, out) = bynkc(&["check"], &dir);
    assert!(
        !ok,
        "five 250 KiB embeds must pass the project cap, got:\n{out}"
    );
    assert!(out.contains("bynk.embed.too_large"), "{out}");
    assert!(
        out.contains("embed `f4`"),
        "only the file past the cap: {out}"
    );
    assert!(!out.contains("embed `f3`"), "{out}");
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

/// A context sees the embeds of a commons it `uses`, typed `String`, and a
/// local declaration of the name wins over the imported embed.
#[test]
fn a_used_commons_embed_is_a_string_and_a_local_declaration_wins() {
    let dir = project("embed-check-uses");
    std::fs::write(
        dir.join("src/app/board.bynk"),
        "context app.board\n\nuses app.page\n\nfn shell_text() -> String {\n  shell\n}\n",
    )
    .unwrap();
    let (ok, out) = bynkc(&["check"], &dir);
    assert!(ok, "a used commons' embed must check, got:\n{out}");

    // A local function of the same name wins; the embed is not the value.
    std::fs::write(
        dir.join("src/app/board.bynk"),
        "context app.board\n\nuses app.page\n\nfn shell() -> Int {\n  1\n}\n\nfn n() -> Int {\n  shell()\n}\n",
    )
    .unwrap();
    let (ok, out) = bynkc(&["check"], &dir);
    assert!(
        ok,
        "a local declaration must win over the import, got:\n{out}"
    );
}
