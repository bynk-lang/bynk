//! Approving what a dependency's adapters bring into the build (packaging
//! slice 5, #2296), over the real binaries, with registry packages read from a
//! `file://` index. No test touches the network. Each test has its own
//! `BYNK_HOME`, and stdin is never a terminal, so nothing prompts.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn bin(name: &str) -> PathBuf {
    let bynk = PathBuf::from(env!("CARGO_BIN_EXE_bynk"));
    if name == "bynk" {
        bynk
    } else {
        bynk.parent().unwrap().join(name)
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("npm-approval")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// `bin <args>` in `cwd`, with `BYNK_HOME` under `root`: `(code, stdout, stderr)`.
fn run(name: &str, cwd: &Path, root: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new(bin(name))
        .args(args)
        .current_dir(cwd)
        .env("BYNK_HOME", root.join("home"))
        .env_remove("BYNK_BYNKC")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

fn bynk(cwd: &Path, root: &Path, args: &[&str]) -> (i32, String) {
    let (code, out, err) = run("bynk", cwd, root, args);
    (code, format!("{out}{err}"))
}

fn file_url(dir: &Path) -> String {
    let path = dir.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

/// A registry at `root/registry` (id `acme`), its key at `root/acme.key`.
fn registry(root: &Path) -> (PathBuf, PathBuf) {
    let dir = root.join("registry");
    let key = root.join("acme.key");
    let (code, out) = bynk(
        root,
        root,
        &[
            "registry",
            "init",
            dir.to_str().unwrap(),
            "--id",
            "acme",
            "--key-out",
            key.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{out}");
    (dir, key)
}

/// Publish `acme/<name>` `version` with an adapter `tokens` requiring `jose`
/// at `range`, and a commons `currency`.
fn publish(root: &Path, reg: &Path, key: &Path, name: &str, version: &str, range: &str) {
    let pkg = root.join("pkgs").join(format!("{name}-{version}"));
    write(
        &pkg.join("bynk.toml"),
        &format!("[package]\nname = \"acme/{name}\"\nversion = \"{version}\"\n"),
    );
    write(
        &pkg.join("src/currency.bynk"),
        "commons currency\n\nfn double(c: Int) -> Int {\n  c * 2\n}\n",
    );
    write(
        &pkg.join("src/tokens.bynk"),
        &format!(
            "adapter tokens {{\n  binding \"./tokens.binding.ts\" requires {{ \"jose\": \"{range}\" }}\n\n  exports capability {{ Jwt }}\n\n  capability Jwt {{\n    fn sign(sub: String, secret: String) -> Effect[String]\n  }}\n\n  provides Jwt = JoseJwt\n}}\n"
        ),
    );
    write(
        &pkg.join("src/tokens.binding.ts"),
        "export class JoseJwt {\n  async sign(sub: string, secret: string): Promise<string> {\n    return sub + secret;\n  }\n}\n",
    );
    let (code, out) = bynk(
        root,
        root,
        &[
            "registry",
            "add",
            reg.to_str().unwrap(),
            pkg.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
        ],
    );
    assert_eq!(code, 0, "{out}");
}

/// A context consuming `<alias>.tokens`.
fn sessions(alias: &str) -> String {
    format!(
        "context sessions\n\nconsumes {alias}.tokens\n\nservice login {{\n  on call(secret: String) -> Effect[String] given {alias}.tokens.Jwt {{\n    let token <- {alias}.tokens.Jwt.sign(\"u1\", secret)\n    token\n  }}\n}}\n"
    )
}

/// A consumer at `root/app` routing `acme` to the registry, with `deps` as
/// its `[dependencies]` lines and `src` as `src/sessions.bynk`.
fn app(root: &Path, reg: &Path, deps: &str, src: &str) -> PathBuf {
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        &format!(
            "[project]\nname = \"app\"\n\n[registries.acme-internal]\nindex = \"{}\"\norganisations = [\"acme\"]\n\n[dependencies]\n{deps}",
            file_url(reg)
        ),
    );
    write(&app.join("src/sessions.bynk"), src);
    app
}

fn lock(app: &Path) -> String {
    std::fs::read_to_string(app.join("bynk.lock")).unwrap_or_default()
}

fn compiled_package_json(app: &Path, root: &Path) -> Option<String> {
    if !bin("bynkc").exists() {
        return None;
    }
    let out = root.join("out");
    let (code, o, e) = run(
        "bynkc",
        app,
        root,
        &["compile", ".", "-o", out.to_str().unwrap()],
    );
    assert_eq!(code, 0, "{o}{e}");
    Some(std::fs::read_to_string(out.join("package.json")).unwrap())
}

const MONEY: &str = "money = { package = \"acme/money\", version = \"^1\" }\n";

#[test]
fn a_registry_adapters_npm_package_enters_the_build_only_once_approved() {
    let root = scratch("gate");
    let (reg, key) = registry(&root);
    publish(&root, &reg, &key, "money", "1.0.0", "^5.2.0");
    let app = app(&root, &reg, MONEY, &sessions("money"));

    let (code, out) = bynk(&app, &root, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("[bynk.requires.unapproved]")
            && out.contains(
                "adapter `acme/money:tokens` requires the npm package `jose` at `^5.2.0`"
            )
            && out.contains("this project → acme/money")
            && out.contains("bynk approve jose"),
        "{out}"
    );
    let text = lock(&app);
    assert!(
        text.contains("[[npm]]\npackage = \"jose\"\nrange = \"^5.2.0\"\nrequired-by = [\"acme/money:tokens\"]\n")
            && !text.contains("approved"),
        "{text}"
    );
    if bin("bynkc").exists() {
        let (code, o, e) = run("bynkc", &app, &root, &["check", "."]);
        assert_eq!(code, 1, "{o}{e}");
        assert!(format!("{o}{e}").contains("bynk.requires.unapproved"));
    }

    let (code, out, err) = run("bynk", &app, &root, &["approve", "--list"]);
    assert_eq!(code, 0, "{err}");
    assert!(
        out.contains("pending") && out.contains("jose ^5.2.0"),
        "{out}"
    );
    assert_eq!(lock(&app), text, "`--list` changes nothing");

    // Off a terminal, a bare `bynk approve` asks for a package or `--all`.
    let (code, out) = bynk(&app, &root, &["approve"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("--all"), "{out}");
    let (code, out) = bynk(&app, &root, &["approve", "left-pad"]);
    assert_eq!(code, 1, "{out}");

    let (code, out) = bynk(&app, &root, &["approve", "jose"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("approved npm package `jose` ^5.2.0")
            && out.contains("this project → acme/money → acme/money:tokens")
            && out.contains("https://www.npmjs.com/package/jose"),
        "{out}"
    );
    assert!(lock(&app).contains("required-by = [\"acme/money:tokens\"]\napproved = true\n"));
    let (code, out) = bynk(&app, &root, &["check", "--locked", "--offline"]);
    assert_eq!(code, 0, "{out}");
    if let Some(pkg) = compiled_package_json(&app, &root) {
        assert!(pkg.contains("\"jose\": \"^5.2.0\""), "{pkg}");
    }
}

#[test]
fn a_changed_range_asks_again_and_a_new_requirer_of_an_approved_row_does_not() {
    let root = scratch("scope");
    let (reg, key) = registry(&root);
    publish(&root, &reg, &key, "money", "1.0.0", "^5.2.0");
    let app = app(&root, &reg, MONEY, &sessions("money"));
    let _ = bynk(&app, &root, &["fetch"]);
    let (code, out) = bynk(&app, &root, &["approve", "--all"]);
    assert_eq!(code, 0, "{out}");

    // Another package requiring the approved `(jose, ^5.2.0)` keeps it approved.
    publish(&root, &reg, &key, "auth", "1.0.0", "^5.2.0");
    write(
        &app.join("bynk.toml"),
        &std::fs::read_to_string(app.join("bynk.toml"))
            .unwrap()
            .replace(
                MONEY,
                &format!("{MONEY}auth = {{ package = \"acme/auth\", version = \"^1\" }}\n"),
            ),
    );
    let (code, out) = bynk(&app, &root, &["fetch"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains(
            "the approved npm package `jose` ^5.2.0 is now also required by `acme/auth:tokens`"
        ),
        "{out}"
    );
    assert!(lock(&app).contains(
        "required-by = [\"acme/auth:tokens\", \"acme/money:tokens\"]\napproved = true\n"
    ));

    // A new version moving the range is a new row: the fetch says so, and the
    // build refuses it until approved.
    publish(&root, &reg, &key, "money", "1.1.0", "^6.0.0");
    let (code, out) = bynk(&app, &root, &["update", "money"]);
    assert_eq!(code, 0, "{out}");
    let text = lock(&app);
    assert!(text.contains("version = \"1.1.0\""), "{text}");
    assert!(
        out.contains("npm packages awaiting approval")
            && out.contains("jose ^6.0.0")
            && out.contains("bynk approve"),
        "{out}"
    );
    assert!(
        text.contains("range = \"^6.0.0\"\nrequired-by = [\"acme/money:tokens\"]\n\n")
            || text.ends_with("range = \"^6.0.0\"\nrequired-by = [\"acme/money:tokens\"]\n"),
        "{text}"
    );
    let (code, out) = bynk(&app, &root, &["check", "--format", "short"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("`jose` at `^6.0.0`"), "{out}");
}

#[test]
fn an_unreached_adapters_pending_row_is_not_an_error() {
    let root = scratch("unreached");
    let (reg, key) = registry(&root);
    publish(&root, &reg, &key, "money", "1.0.0", "^5.2.0");
    let app = app(
        &root,
        &reg,
        MONEY,
        "commons sessions\n\nuses money.currency\n\nfn total(n: Int) -> Int {\n  double(n)\n}\n",
    );
    let (code, out) = bynk(&app, &root, &["check"]);
    assert_eq!(code, 0, "{out}");
    let (_, out, _) = run("bynk", &app, &root, &["approve", "--list"]);
    assert!(out.contains("pending") && out.contains("jose"), "{out}");
    if let Some(pkg) = compiled_package_json(&app, &root) {
        assert!(!pkg.contains("jose"), "{pkg}");
    }
}

#[test]
fn a_fetch_off_a_terminal_writes_the_lock_and_says_what_awaits_approval() {
    let root = scratch("offer");
    let (reg, key) = registry(&root);
    publish(&root, &reg, &key, "money", "1.0.0", "^5.2.0");
    let app = app(&root, &reg, MONEY, &sessions("money"));
    let (code, out) = bynk(&app, &root, &["fetch"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("wrote bynk.lock")
            && out.contains("npm packages awaiting approval")
            && out.contains("jose ^5.2.0")
            && out.contains("bynk approve"),
        "{out}"
    );
    // `check` never offers; it fails with the command to run.
    let (code, out) = bynk(&app, &root, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(!out.contains("awaiting approval"), "{out}");
}
