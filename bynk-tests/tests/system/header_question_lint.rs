//! #2839 (Decision B): `headers.get("X")?` in an `HttpResult` handler lifts a
//! missing header to `404`; the warning says so and offers the `let … else`
//! rewrite that answers `400`. The behaviour of the rewrite on the wire is
//! pinned by the `2839_http_header_question_behaviour` positive fixture.
//!
//! The lint is a checker warning, so it is driven through the real
//! `bynk check` on a scratch project (the `--fix` flag applies its rewrite).

use std::path::PathBuf;
use std::process::Command;

const CODE: &str = "bynk.http.header_question_not_found";

fn handler(body: &str) -> String {
    format!(
        "context demo.hooks\n\nfn find(k: String) -> Option[String] {{ None }}\n\nservice hooks from http {{\n  on GET(\"/e\") (headers: RequestHeaders) -> Effect[HttpResult[String]] by v: Visitor {{\n{body}\n  }}\n}}\n"
    )
}

fn project(tag: &str, source: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bynk-2839-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src/demo")).unwrap();
    std::fs::write(
        dir.join("bynk.toml"),
        "[project]\nname = \"p\"\nversion = \"0.1.0\"\n\n[paths]\ninclude = [\"src\"]\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/demo/hooks.bynk"), source).unwrap();
    dir
}

fn check(dir: &PathBuf, fix: bool) -> (bool, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"));
    cmd.arg("check").arg(dir).args(["--format", "short"]);
    if fix {
        cmd.arg("--fix");
    }
    let out = cmd.output().expect("bynk check runs");
    let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    (out.status.success(), s)
}

const FLAGGED: &str = "    let event = headers.get(\"X-Event\")?\n    Ok(event)";

#[test]
fn a_header_question_in_an_http_handler_is_a_warning() {
    let dir = project("warn", &handler(FLAGGED));
    let (ok, out) = check(&dir, false);
    assert!(ok, "a warning never fails the check:\n{out}");
    assert!(out.contains(&format!("warning[{CODE}]")), "{out}");
}

#[test]
fn the_fix_rewrites_to_a_let_else_that_answers_400() {
    let dir = project("fix", &handler(FLAGGED));
    let (ok, out) = check(&dir, true);
    assert!(ok, "{out}");
    let fixed = std::fs::read_to_string(dir.join("src/demo/hooks.bynk")).unwrap();
    assert!(
        fixed.contains(
            "let Some(event) = headers.get(\"X-Event\") else { BadRequest(\"missing X-Event\") }"
        ),
        "{fixed}"
    );
    let (ok, again) = check(&dir, false);
    assert!(
        ok && !again.contains(CODE),
        "the fixed handler is clean:\n{again}"
    );
}

#[test]
fn a_header_question_that_is_not_a_plain_let_is_flagged_without_a_fix() {
    let dir = project("nested", &handler("    Ok(headers.get(\"X-Event\")?)"));
    let (_, out) = check(&dir, true);
    assert!(out.contains(CODE), "still reported after --fix:\n{out}");
    let src = std::fs::read_to_string(dir.join("src/demo/hooks.bynk")).unwrap();
    assert!(
        src.contains("headers.get(\"X-Event\")?"),
        "left untouched: {src}"
    );
}

#[test]
fn question_on_a_non_header_option_is_not_flagged() {
    let dir = project(
        "other",
        &handler("    let stored = find(\"k\")\n    let x = stored?\n    Ok(x)"),
    );
    let (ok, out) = check(&dir, false);
    assert!(ok, "{out}");
    assert!(!out.contains(CODE), "{out}");
}
