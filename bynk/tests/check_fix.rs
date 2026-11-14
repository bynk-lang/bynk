//! #2642: `bynk check --fix` over the real binary.
//!
//! Pinned here:
//!  - a fix that exposes a further fixable diagnostic converges in more than
//!    one pass, ends clean, and reports each written file with its edit count;
//!  - a fix that would introduce a diagnostic needing a human writes nothing
//!    anywhere and exits non-zero;
//!  - untouched files are not rewritten (mtime preserved);
//!  - a workspace root fixes every member.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

fn bynk() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
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
    assert!(err.contains("fixed: src/app/relay.bynk: bynk.given.undeclared_capability ×1, bynk.provider.agent_call_undeclared_capability ×1"), "{err}");
    assert!(!err.contains("clean.bynk"), "{err}");
    assert_eq!(mtime(&root.join("src/app/clean.bynk")), clean_before);
    // The tree is clean afterwards, with nothing more to fix.
    let (code, err) = run(&bynk(), &root, &["check", "--fix"], &[]);
    assert_eq!(code, 0, "{err}");
    assert!(!err.contains("fixed:"), "{err}");
    assert!(!err.contains("bynk."), "no diagnostic remains: {err}");
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
    assert!(
        err.contains("fixed: age.bynk: bynk.types.inverted_range ×1"),
        "{err}"
    );
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
                "fixed: packages/{name}/src/app/unused.bynk: bynk.given.unused_capability ×1"
            )),
            "{err}"
        );
    }
}

/// #3274: `--each` over a manifest-less directory of projects fixes each one,
/// as a workspace root does.
#[test]
fn each_fixes_every_project_of_a_directory() {
    let root = scratch("each");
    for name in ["a", "b"] {
        write(
            &root.join(format!("{name}/bynk.toml")),
            &format!("[project]\nname = \"{name}\"\n"),
        );
        write(&root.join(format!("{name}/src/app/unused.bynk")), UNUSED);
    }
    let (code, err) = run(&bynk(), &root, &["check", "--each", "--fix"], &[]);
    assert_eq!(code, 0, "{err}");
    for name in ["a", "b"] {
        assert_eq!(
            read(&root.join(format!("{name}/src/app/unused.bynk"))),
            UNUSED_FIXED
        );
        assert!(
            err.contains(&format!(
                "fixed: {name}/src/app/unused.bynk: bynk.given.unused_capability ×1"
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

// ---- #3537: `--fix` applies only what `check` shows ----

/// Two `~>` sends (the deprecated spelling, `bynk.send.deprecated`), and a nested
/// `match` staircase that `check` only offers as an editor hint.
const SENDS_AND_STAIRCASE: &str = "\
commons app.sends

fn keep(e: Effect[()]) -> Effect[()] { e }

fn twice(ping: Effect[()]) -> Effect[Int] {
  ~> ping
  ~> keep(ping)
  Effect.pure(2)
}

fn read(a: Option[Int], b: Option[Int]) -> Result[Int, String] {
  match a {
    None => Err(\"a\"),
    Some(x) => match b {
      None => Err(\"b\"),
      Some(y) => Ok(x + y),
    },
  }
}
";

/// A one-armed `if` as a bare statement: rejected, and not for `--fix` to bind.
const ONE_ARMED_IF: &str = "\
commons app.guard

fn greet(name: String) -> Effect[()] {
  if name == \"world\" { Effect.pure(()) }
  Effect.pure(())
}
";

#[test]
fn fix_touches_only_what_check_reports() {
    let root = scratch("scope");
    let sends = root.join("src/app/sends.bynk");
    let guard = root.join("src/app/guard.bynk");
    write(&sends, SENDS_AND_STAIRCASE);
    write(&guard, ONE_ARMED_IF);
    let (_, shown) = run(&bynk(), &root, &["check", "."], &[]);
    assert!(shown.contains("bynk.send.deprecated"), "{shown}");
    assert!(
        !shown.contains("let_else_staircase"),
        "a hint is not shown: {shown}"
    );
    let (code, err) = run(&bynk(), &root, &["check", "--fix"], &[]);
    assert_ne!(code, 0, "guard.bynk is still an error: {err}");
    assert!(
        err.contains("fixed: src/app/sends.bynk: bynk.send.deprecated ×2"),
        "{err}"
    );
    assert!(
        !err.contains("guard.bynk (") && !err.contains("fixed: src/app/guard"),
        "{err}"
    );
    // Only the `~>` lines changed.
    let expected = SENDS_AND_STAIRCASE
        .replace("~> ping", "do ping.detach()")
        .replace("~> keep(ping)", "do keep(ping).detach()");
    assert_eq!(read(&sends), expected);
    assert!(read(&sends).contains("match a {"));
    assert_eq!(read(&guard), ONE_ARMED_IF);
}

// ---- #3537: `--fix=<code>` ----

const AGE: &str = "commons app.age\n\ntype Age = Int where InRange(120, 0)\n";
const AGE_FIXED: &str = "commons app.age\n\ntype Age = Int where InRange(0, 120)\n";

const STAIRCASE: &str = "\
commons app.stairs

fn read(a: Option[Int], b: Option[Int]) -> Result[Int, String] {
  match a {
    None => Err(\"a\"),
    Some(x) => match b {
      None => Err(\"b\"),
      Some(y) => Ok(x + y),
    },
  }
}
";

/// Three files, each with a different fix: an inverted range and an unused
/// capability (both shown by `check`), and a staircase (a hint it hides).
fn scoped_project(name: &str) -> PathBuf {
    let root = scratch(name);
    write(&root.join("src/app/age.bynk"), AGE);
    write(&root.join("src/app/unused.bynk"), UNUSED);
    write(&root.join("src/app/stairs.bynk"), STAIRCASE);
    root
}

#[test]
fn a_named_code_takes_only_its_fixes() {
    let root = scoped_project("named");
    let (code, err) = run(
        &bynk(),
        &root,
        &["check", "--fix=bynk.types.inverted_range"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("fixed: src/app/age.bynk: bynk.types.inverted_range ×1"),
        "{err}"
    );
    assert_eq!(read(&root.join("src/app/age.bynk")), AGE_FIXED);
    // The other shown fix and the hint are left alone.
    assert_eq!(read(&root.join("src/app/unused.bynk")), UNUSED);
    assert_eq!(read(&root.join("src/app/stairs.bynk")), STAIRCASE);
}

#[test]
fn several_codes_may_be_listed_or_repeated() {
    for args in [
        &[
            "check",
            "--fix=bynk.types.inverted_range,bynk.given.unused_capability",
        ][..],
        &[
            "check",
            "--fix=bynk.types.inverted_range",
            "--fix=bynk.given.unused_capability",
        ][..],
    ] {
        let root = scoped_project("several");
        let (code, err) = run(&bynk(), &root, args, &[]);
        assert_eq!(code, 0, "{err}");
        assert_eq!(read(&root.join("src/app/age.bynk")), AGE_FIXED);
        assert_eq!(read(&root.join("src/app/unused.bynk")), UNUSED_FIXED);
        assert_eq!(read(&root.join("src/app/stairs.bynk")), STAIRCASE);
    }
}

#[test]
fn a_hint_is_fixed_only_when_its_code_is_named() {
    let root = scoped_project("hint");
    let (_, err) = run(&bynk(), &root, &["check", "--fix"], &[]);
    assert!(
        !err.contains("stairs.bynk"),
        "plain --fix leaves a hint: {err}"
    );
    assert_eq!(read(&root.join("src/app/stairs.bynk")), STAIRCASE);
    let (code, err) = run(
        &bynk(),
        &root,
        &["check", "--fix=bynk.parse.let_else_staircase"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("fixed: src/app/stairs.bynk: bynk.parse.let_else_staircase ×1"),
        "{err}"
    );
    assert!(read(&root.join("src/app/stairs.bynk")).contains("let Some(x) = a else"));
}

#[test]
fn an_unknown_code_is_refused_with_the_nearest_real_one() {
    let root = scoped_project("unknown");
    let (code, err) = run(
        &bynk(),
        &root,
        &["check", "--fix=bynk.types.inverted_rang"],
        &[],
    );
    assert_ne!(code, 0, "{err}");
    assert!(
        err.contains("unknown diagnostic code `bynk.types.inverted_rang`")
            && err.contains("did you mean `bynk.types.inverted_range`?"),
        "{err}"
    );
    // Refused before any work: nothing written.
    assert_eq!(read(&root.join("src/app/age.bynk")), AGE);
    assert_eq!(read(&root.join("src/app/unused.bynk")), UNUSED);
}

#[test]
fn a_code_with_nothing_to_fix_writes_nothing() {
    let root = scoped_project("nothing");
    let (code, err) = run(&bynk(), &root, &["check", "--fix=bynk.fault.todo"], &[]);
    // The inverted range is still an error, so the check that follows fails.
    assert_ne!(code, 0, "{err}");
    assert!(!err.contains("fixed:"), "{err}");
    assert_eq!(read(&root.join("src/app/age.bynk")), AGE);
}
