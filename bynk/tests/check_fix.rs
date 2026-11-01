//! #2642: `bynk check --fix` / `bynkc check --fix` over the real binaries.
//!
//! Pinned here:
//!  - a fix that exposes a further fixable diagnostic converges in more than
//!    one pass, ends clean, and reports each written file with its edit count;
//!  - a fix that would introduce a diagnostic needing a human writes nothing
//!    anywhere and exits non-zero;
//!  - untouched files are not rewritten (mtime preserved);
//!  - `bynkc check --fix` and the `BYNK_BYNKC` delegation do the same;
//!  - a workspace root fixes every member.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

/// The sibling `bynkc`, present only when the whole workspace was built.
fn bynkc_sibling() -> Option<PathBuf> {
    let p = bynk().parent().unwrap().join("bynkc");
    p.exists().then_some(p)
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("check-fix")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap()
}

/// Set `path`'s modification time well in the past, so a later rewrite is
/// visible however coarse the filesystem's clock is.
fn age(path: &Path) {
    let file = std::fs::File::options().write(true).open(path).unwrap();
    file.set_modified(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000))
        .unwrap();
}

fn mtime(path: &Path) -> SystemTime {
    std::fs::metadata(path).unwrap().modified().unwrap()
}

/// Run `bin <args>` in `cwd`, capturing (code, stderr).
fn run(bin: &Path, cwd: &Path, args: &[&str], env: &[(&str, &Path)]) -> (i32, String) {
    let mut cmd = Command::new(bin);
    cmd.args(args)
        .current_dir(cwd)
        .env_remove("BYNK_BYNKC")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The relay context. The agent handler uses `Beta` and, until fixed, does
/// not list it; the provider that calls the handler lists only `Alpha`.
/// Fixing the handler makes the provider short of `Beta`, which is a fixable
/// diagnostic of its own, so the run needs two passes.
fn relay(agent_given: &str, provider_given: &str) -> String {
    format!(
        "\
context app.relay

capability Alpha {{
  fn ping() -> Effect[Int]
}}

capability Beta {{
  fn ping() -> Effect[Int]
}}

capability Relay {{
  fn ping() -> Effect[Int]
}}

provides Alpha = AlphaImpl {{
  fn ping() -> Effect[Int] {{
    1
  }}
}}

provides Beta = BetaImpl {{
  fn ping() -> Effect[Int] {{
    2
  }}
}}

agent Counter {{
  key name: String

  store hits: Cell[Int]

  on call ping() -> Effect[Int]{agent_given} {{
    Beta.ping()
  }}
}}

provides Relay = AgentRelay{provider_given} {{
  fn ping() -> Effect[Int] {{
    let a <- Alpha.ping()
    let c <- Counter(\"x\").ping()
    Effect.pure(a + c)
  }}
}}
"
    )
}

const TIME: &str = "\
context platform.time

exports capability { Clock }

capability Clock {
  fn now() -> Effect[Int]
}

provides Clock = SystemClock {
  fn now() -> Effect[Int] {
    0
  }
}
";

/// Like [`relay`], but the capability the handler is short of lives in another
/// context, so fixing the handler leaves the provider short of something no
/// machine-applicable fix adds. The run must be refused.
fn cross(agent_given: &str) -> String {
    format!(
        "\
context app.cross

consumes platform.time as Time

capability Alpha {{
  fn ping() -> Effect[Int]
}}

provides Alpha = AlphaImpl {{
  fn ping() -> Effect[Int] {{
    1
  }}
}}

capability Relay {{
  fn ping() -> Effect[Int]
}}

agent Stamp {{
  key name: String

  store hits: Cell[Int]

  on call ping() -> Effect[Int]{agent_given} {{
    let t <- Time.Clock.now()
    t
  }}
}}

provides Relay = StampRelay given Alpha {{
  fn ping() -> Effect[Int] {{
    let a <- Alpha.ping()
    let s <- Stamp(\"x\").ping()
    Effect.pure(a + s)
  }}
}}
"
    )
}

/// A file with a lone fixable problem and nothing else: `Alpha` is listed
/// but never used.
const UNUSED: &str = "\
context app.unused

capability Alpha {
  fn ping() -> Effect[Int]
}

provides Alpha = AlphaImpl {
  fn ping() -> Effect[Int] {
    1
  }
}

service one {
  on call(x: Int) -> Effect[Int] given Alpha {
    42
  }
}
";

const UNUSED_FIXED: &str = "\
context app.unused

capability Alpha {
  fn ping() -> Effect[Int]
}

provides Alpha = AlphaImpl {
  fn ping() -> Effect[Int] {
    1
  }
}

service one {
  on call(x: Int) -> Effect[Int] {
    42
  }
}
";

const CLEAN: &str = "\
context app.clean

capability Alpha {
  fn ping() -> Effect[Int]
}
";

fn cascade_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("src/app/relay.bynk"), &relay("", " given Alpha"));
    write(&root.join("src/app/clean.bynk"), CLEAN);
    root
}

#[test]
fn a_cascade_converges_and_ends_clean() {
    let root = cascade_project("cascade");
    age(&root.join("src/app/clean.bynk"));
    let clean_before = mtime(&root.join("src/app/clean.bynk"));
    // The chain takes two passes: the provider is only found short of `Beta`
    // once the handler lists it, so the editor's fix all, which takes one pass
    // at a time, would need pressing twice.
    let plan = bynk_driver::fix::plan_project(&root)
        .expect("set up")
        .expect("accepted");
    assert_eq!(plan.passes, 2);
    assert_eq!(plan.files.len(), 1);
    assert_eq!(plan.files[0].first_pass.len(), 1);
    assert_eq!(plan.files[0].edits, 2);
    let (code, err) = run(&bynk(), &root, &["check", "--fix"], &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        read(&root.join("src/app/relay.bynk")),
        relay(" given Beta", " given Alpha, Beta")
    );
    // Two edits, one per pass, in the one file; the untouched file is not
    // listed and not rewritten.
    assert!(err.contains("fixed: src/app/relay.bynk (2 edits)"), "{err}");
    assert!(!err.contains("clean.bynk"), "{err}");
    assert_eq!(mtime(&root.join("src/app/clean.bynk")), clean_before);
    // The tree is clean afterwards, with nothing more to fix.
    let (code, err) = run(&bynk(), &root, &["check", "--fix"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("fixed:"), "{err}");
    assert!(!err.contains("bynk."), "no diagnostic remains: {err}");
}

#[test]
fn bynkc_applies_the_same_fixes() {
    let Some(bynkc) = bynkc_sibling() else {
        eprintln!("skipping: sibling bynkc not built");
        return;
    };
    let root = cascade_project("bynkc");
    let (code, err) = run(&bynkc, &root, &["check", "--fix", "."], &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        read(&root.join("src/app/relay.bynk")),
        relay(" given Beta", " given Alpha, Beta")
    );
    assert!(err.contains("fixed: src/app/relay.bynk (2 edits)"), "{err}");
}

#[test]
fn the_bynk_bynkc_override_forwards_fix() {
    let Some(bynkc) = bynkc_sibling() else {
        eprintln!("skipping: sibling bynkc not built");
        return;
    };
    let root = cascade_project("delegated");
    let env = [("BYNK_BYNKC", bynkc.as_path())];
    let (code, err) = run(&bynk(), &root, &["check", "--fix"], &env);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        read(&root.join("src/app/relay.bynk")),
        relay(" given Beta", " given Alpha, Beta")
    );
    assert!(err.contains("fixed: src/app/relay.bynk (2 edits)"), "{err}");
}

#[test]
fn a_refused_run_leaves_every_file_byte_identical() {
    let root = scratch("refused");
    // `unused.bynk` has a fixable problem of its own: a refusal must not
    // write it either.
    write(&root.join("src/platform/time.bynk"), TIME);
    write(&root.join("src/app/cross.bynk"), &cross(""));
    write(&root.join("src/app/unused.bynk"), UNUSED);
    let files = [
        "src/platform/time.bynk",
        "src/app/cross.bynk",
        "src/app/unused.bynk",
    ];
    for f in files {
        age(&root.join(f));
    }
    let before: Vec<(String, SystemTime)> = files
        .iter()
        .map(|f| (read(&root.join(f)), mtime(&root.join(f))))
        .collect();
    let (code, err) = run(&bynk(), &root, &["check", "--fix"], &[]);
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("nothing was written"), "{err}");
    assert!(
        err.contains("bynk.provider.agent_call_undeclared_capability"),
        "{err}"
    );
    assert!(!err.contains("fixed:"), "{err}");
    for (file, (text, time)) in files.iter().zip(&before) {
        assert_eq!(&read(&root.join(file)), text, "{file}");
        assert_eq!(&mtime(&root.join(file)), time, "{file}");
    }
}

#[test]
fn fixing_a_single_file() {
    let root = scratch("file");
    write(
        &root.join("age.bynk"),
        "commons app.age\n\ntype Age = Int where InRange(120, 0)\n",
    );
    let (code, err) = run(&bynk(), &root, &["check", "--fix", "age.bynk"], &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        read(&root.join("age.bynk")),
        "commons app.age\n\ntype Age = Int where InRange(0, 120)\n"
    );
    assert!(err.contains("fixed: age.bynk (2 edits)"), "{err}");
}

#[test]
fn a_workspace_root_fixes_every_member() {
    let root = scratch("workspace");
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    );
    for name in ["a", "b"] {
        write(
            &root.join(format!("packages/{name}/bynk.toml")),
            &format!("[package]\nname = \"acme/{name}\"\n"),
        );
        write(
            &root.join(format!("packages/{name}/src/app/unused.bynk")),
            UNUSED,
        );
    }
    let (code, err) = run(&bynk(), &root, &["check", "--fix"], &[]);
    assert_eq!(code, 0, "{err}");
    for name in ["a", "b"] {
        assert_eq!(
            read(&root.join(format!("packages/{name}/src/app/unused.bynk"))),
            UNUSED_FIXED
        );
        assert!(
            err.contains(&format!(
                "fixed: packages/{name}/src/app/unused.bynk (1 edit)"
            )),
            "{err}"
        );
    }
}

#[test]
fn without_fix_nothing_is_written() {
    let root = scratch("no-flag");
    write(&root.join("src/app/unused.bynk"), UNUSED);
    let (code, err) = run(&bynk(), &root, &["check", "."], &[]);
    assert_eq!(code, 0, "an unused capability is a warning: {err}");
    assert!(err.contains("bynk.given.unused_capability"), "{err}");
    assert_eq!(read(&root.join("src/app/unused.bynk")), UNUSED);
}

#[test]
fn a_refusal_in_one_member_writes_nothing_in_any() {
    let root = scratch("workspace-refused");
    write(
        &root.join("bynk.toml"),
        "[workspace]\nmembers = [\"packages/*\"]\n",
    );
    for name in ["a", "b"] {
        write(
            &root.join(format!("packages/{name}/bynk.toml")),
            &format!("[package]\nname = \"acme/{name}\"\n"),
        );
    }
    // `a` is fixable and is checked first; `b` is refused.
    let fixable = root.join("packages/a/src/app/unused.bynk");
    write(&fixable, UNUSED);
    write(&root.join("packages/b/src/platform/time.bynk"), TIME);
    write(&root.join("packages/b/src/app/cross.bynk"), &cross(""));
    age(&fixable);
    let before = mtime(&fixable);
    let (code, err) = run(&bynk(), &root, &["check", "--fix"], &[]);
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("nothing was written"), "{err}");
    assert!(!err.contains("fixed:"), "{err}");
    assert_eq!(read(&fixable), UNUSED);
    assert_eq!(mtime(&fixable), before);
}
