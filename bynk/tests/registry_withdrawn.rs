//! Withdrawn versions (first-party registry R5a.1, #2330), over the real
//! binaries: `bynk registry withdraw` on a static registry, the client's
//! refusal of a locked withdrawn version, the publish API's `410`/`409`
//! refusals, and the `read.withdrawn` conformance check. No test touches the
//! network. Each test has its own `BYNK_HOME`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("registry-withdrawn")
        .join(name);
    if dir.exists() {
        writable(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The store makes its entries read-only; let a re-run clear them.
fn writable(dir: &Path) {
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let p = e.path();
        if p.is_dir() {
            writable(&p);
        } else if let Ok(m) = std::fs::metadata(&p) {
            let mut perms = m.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            perms.set_readonly(false);
            let _ = std::fs::set_permissions(&p, perms);
        }
    }
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// `bynk <args>` in `cwd` with `BYNK_HOME = home` and `env`: `(code, stdout,
/// stderr)`.
fn bynk(cwd: &Path, home: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(cwd)
        .env("BYNK_HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env_remove("BYNK_REGISTRY_ACME_INTERNAL_TOKEN")
        .env_remove("BYNK_BYNKC")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// `bynk registry <args>` from `root`, expecting success; its output.
fn registry_ok(root: &Path, args: &[&str]) -> String {
    let mut all = vec!["registry"];
    all.extend(args);
    let (code, out, err) = bynk(root, &root.join("home"), &all, &[]);
    assert_eq!(code, 0, "{out}{err}");
    format!("{out}{err}")
}

/// A registry at `root/registry` (id `acme`), its key at `root/acme.key`;
/// returns the directory, the key and the `key = "…"` pin.
fn registry(root: &Path) -> (PathBuf, PathBuf, String) {
    let dir = root.join("registry");
    let key = root.join("acme.key");
    let out = registry_ok(
        root,
        &[
            "init",
            dir.to_str().unwrap(),
            "--id",
            "acme",
            "--key-out",
            key.to_str().unwrap(),
        ],
    );
    let pin = out
        .lines()
        .find(|l| l.starts_with("key = "))
        .unwrap()
        .to_string();
    (dir, key, pin)
}

/// The package `acme/money` at `version` under `root/pkgs`, its commons
/// multiplying by `factor`; `extra` is appended to its manifest.
fn package(root: &Path, version: &str, factor: i32, extra: &str) -> PathBuf {
    let pkg = root.join("pkgs").join(format!("money-{version}"));
    write(
        &pkg.join("bynk.toml"),
        &format!("[package]\nname = \"acme/money\"\nversion = \"{version}\"\n\n{extra}"),
    );
    write(
        &pkg.join("src/currency.bynk"),
        &format!("commons currency\n\nfn double(c: Int) -> Int {{\n  c * {factor}\n}}\n"),
    );
    pkg
}

/// `bynk registry add` of `acme/money` `version` to `reg`.
fn add(root: &Path, reg: &Path, key: &Path, version: &str, factor: i32) -> PathBuf {
    let pkg = package(root, version, factor, "");
    registry_ok(
        root,
        &[
            "add",
            reg.to_str().unwrap(),
            pkg.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
        ],
    );
    pkg
}

fn withdraw(root: &Path, reg: &Path, version: &str, reason: &str) -> (i32, String) {
    let (code, out, err) = bynk(
        root,
        &root.join("home"),
        &[
            "registry",
            "withdraw",
            reg.to_str().unwrap(),
            "acme/money",
            version,
            "--reason",
            reason,
        ],
        &[],
    );
    (code, format!("{out}{err}"))
}

fn index_text(reg: &Path) -> String {
    std::fs::read_to_string(reg.join("acme/money")).unwrap_or_default()
}

/// The archive file of `version`'s line in `reg`.
fn archive_of(reg: &Path, version: &str) -> PathBuf {
    let line = index_text(reg)
        .lines()
        .find(|l| l.contains(&format!("\"version\":\"{version}\"")))
        .unwrap()
        .to_string();
    let checksum = line
        .split("\"checksum\":\"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap();
    let hex = bynk_project::lock::checksum_hex(checksum).unwrap();
    reg.join("dl/sha256").join(hex)
}

/// A consumer at `root/app` routing `acme` to `index`, depending on
/// `acme/money` at `range`.
fn app(root: &Path, index: &str, range: &str) -> PathBuf {
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        &format!(
            "[project]\nname = \"app\"\n\n[registries.acme-internal]\nindex = \"{index}\"\norganisations = [\"acme\"]\n\n[dependencies]\nmoney = {{ package = \"acme/money\", version = \"{range}\" }}\n"
        ),
    );
    write(
        &app.join("src/books.bynk"),
        "commons books\n\nuses money.currency\n\nfn total(n: Int) -> Int {\n  double(n)\n}\n",
    );
    app
}

fn lock(app: &Path) -> String {
    std::fs::read_to_string(app.join("bynk.lock")).unwrap_or_default()
}

/// `dir` as a `file://` URL TOML and every platform accept.
fn file_url(dir: &Path) -> String {
    let path = dir.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        let target = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &target);
        } else {
            std::fs::copy(&p, &target).unwrap();
        }
    }
}

#[test]
fn withdraw_yanks_records_the_reason_and_deletes_the_archive() {
    let root = scratch("withdraw");
    let (reg, key, _) = registry(&root);
    add(&root, &reg, &key, "1.0.0", 2);
    add(&root, &reg, &key, "1.1.0", 3);
    let archive = archive_of(&reg, "1.0.0");
    assert!(archive.is_file());
    let before = index_text(&reg);

    // The reason must be given, on one line, within the limit.
    let (code, out) = withdraw(&root, &reg, "1.0.0", "  ");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("--reason"), "{out}");
    let (code, out) = withdraw(&root, &reg, "1.0.0", &"x".repeat(501));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("500 characters"), "{out}");
    let (code, out) = withdraw(&root, &reg, "1.0.0", "two\nlines");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("control characters"), "{out}");
    let (code, out) = withdraw(&root, &reg, "9.9.9", "gone");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("no `acme/money` 9.9.9"), "{out}");
    assert_eq!(index_text(&reg), before, "a refusal changes nothing");

    let (code, out) = withdraw(&root, &reg, "1.0.0", "a takedown (case 7)");
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("withdrew acme/money 1.0.0"), "{out}");
    assert!(out.contains("deleted its archive"), "{out}");
    assert!(!archive.exists(), "the archive is gone");
    let text = index_text(&reg);
    let line = text.lines().find(|l| l.contains("1.0.0")).unwrap();
    assert!(line.contains("\"yanked\":true"), "{line}");
    assert!(
        line.contains("\"withdrawn\":\"a takedown (case 7)\""),
        "{line}"
    );
    // Nothing is re-signed: the signature is the one `add` wrote.
    let sig = |t: &str| {
        t.lines()
            .find(|l| l.contains("1.0.0"))
            .and_then(|l| l.split("\"sig\":\"").nth(1))
            .map(|s| s.split('"').next().unwrap().to_string())
    };
    assert_eq!(sig(&text), sig(&before));
    // The other version is untouched.
    assert!(archive_of(&reg, "1.1.0").is_file());
    assert!(
        text.lines()
            .find(|l| l.contains("1.1.0"))
            .unwrap()
            .contains("\"yanked\":false")
    );

    // Final: a second withdrawal is refused.
    let (code, out) = withdraw(&root, &reg, "1.0.0", "again");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("already withdrawn"), "{out}");
}

#[test]
fn an_interrupted_withdrawal_is_finished_and_a_refusal_writes_nothing() {
    let root = scratch("interrupted");
    let (reg, key, _) = registry(&root);
    add(&root, &reg, &key, "1.0.0", 2);
    let archive = archive_of(&reg, "1.0.0");

    // An unreadable index elsewhere refuses the withdrawal before any write.
    write(&reg.join("acme/broken"), "not json\n");
    let before = index_text(&reg);
    let (code, out) = withdraw(&root, &reg, "1.0.0", "a takedown");
    assert_eq!(code, 1, "{out}");
    assert_eq!(index_text(&reg), before, "nothing written: {out}");
    assert!(archive.is_file());
    std::fs::remove_file(reg.join("acme/broken")).unwrap();

    // The index says withdrawn but the archive is still there, as a run that
    // failed after its index write leaves it: running again finishes it.
    std::fs::write(
        reg.join("acme/money"),
        before.replace(
            "\"yanked\":false",
            "\"yanked\":true,\"withdrawn\":\"a takedown\"",
        ),
    )
    .unwrap();
    let (code, out) = withdraw(&root, &reg, "1.0.0", "a takedown");
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("it was already withdrawn; deleted the archive left behind"),
        "{out}"
    );
    assert!(!archive.exists());
    let (code, out) = withdraw(&root, &reg, "1.0.0", "a takedown");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("already withdrawn"), "{out}");
}

#[test]
fn a_shared_archive_is_kept_with_a_warning() {
    let root = scratch("shared");
    let (reg, key, _) = registry(&root);
    add(&root, &reg, &key, "1.0.0", 2);
    let archive = archive_of(&reg, "1.0.0");
    // Another package's line naming the very same tree (as a registry built
    // some other way could hold): its archive must survive the withdrawal.
    let other = index_text(&reg)
        .trim()
        .replace("\"acme/money\"", "\"acme/twin\"");
    write(&reg.join("acme/twin"), &format!("{other}\n"));
    let (code, out) = withdraw(&root, &reg, "1.0.0", "a takedown");
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("warning: kept its archive, which acme/twin 1.0.0 names too"),
        "{out}"
    );
    assert!(archive.is_file());
    assert!(index_text(&reg).contains("\"withdrawn\":\"a takedown\""));

    // Conformance accepts a withdrawn version's archive while a live version
    // of the package names the same tree.
    let same = index_text(&reg)
        .lines()
        .next()
        .unwrap()
        .replace("\"version\":\"1.0.0\"", "\"version\":\"1.0.1\"")
        .replace("\"yanked\":true", "\"yanked\":false")
        .replace(",\"withdrawn\":\"a takedown\"", "");
    std::fs::write(
        reg.join("acme/money"),
        format!("{}{same}\n", index_text(&reg)),
    )
    .unwrap();
    token(&root);
    let server = Server::start(&root, &reg, &key);
    // The fabricated 1.0.1 line is unsigned, so the run as a whole fails
    // `read.lines`; `read.withdrawn` is judged on its own.
    let (_, out, err) = bynk(
        &root,
        &root.join("c1"),
        &[
            "registry",
            "conformance",
            &server.index(),
            "--read-only",
            "--package",
            "acme/money",
            "--format",
            "json",
        ],
        &[],
    );
    let json = format!("{out}{err}");
    assert_eq!(outcome(&json, "read.withdrawn"), "pass", "{json}");
}

#[test]
fn a_locked_withdrawn_version_fails_unless_its_tree_is_in_the_store() {
    let root = scratch("client");
    let (reg, key, pin) = registry(&root);
    add(&root, &reg, &key, "1.0.0", 2);
    let home = root.join("home");
    let app = app(&root, &file_url(&reg), "^1");
    let (code, out, err) = bynk(&app, &home, &["check"], &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(lock(&app).contains("version = \"1.0.0\""), "{}", lock(&app));
    add(&root, &reg, &key, "1.1.0", 3);
    let (code, out) = withdraw(
        &root,
        &reg,
        "1.0.0",
        "removed at the copyright holder's request",
    );
    assert_eq!(code, 0, "{out}");

    // Its tree is in this store: the no-network path keeps building.
    let (code, out, err) = bynk(&app, &home, &["check", "--locked"], &[]);
    assert_eq!(code, 0, "{out}{err}");

    // A fresh store has to fetch it, and can't.
    let fresh = root.join("fresh-home");
    let (code, out, err) = bynk(&app, &fresh, &["check"], &[]);
    let all = format!("{out}{err}");
    assert_eq!(code, 1, "{all}");
    assert!(
        all.contains(
            "error[bynk.registry.withdrawn]: `acme/money` 1.0.0 was withdrawn by registry `acme-internal`: \"removed at the copyright holder's request\""
        ),
        "{all}"
    );
    assert!(
        all.contains(
            "= note: `bynk.lock` locks this version; choose another with `bynk update money`"
        ),
        "{all}"
    );
    assert!(!all.contains("mirror"), "read from the origin: {all}");
    assert!(
        lock(&app).contains("version = \"1.0.0\""),
        "the lock is kept"
    );

    // `bynk update` moves off it; a new resolution never chooses it.
    let (code, out, err) = bynk(&app, &fresh, &["update", "money"], &[]);
    assert_eq!(code, 0, "{out}{err}");
    assert!(lock(&app).contains("version = \"1.1.0\""), "{}", lock(&app));
    let (code, out) = withdraw(&root, &reg, "1.1.0", "malware");
    assert_eq!(code, 0, "{out}");
    std::fs::remove_file(app.join("bynk.lock")).unwrap();
    let (code, out, err) = bynk(&app, &root.join("third-home"), &["check"], &[]);
    let all = format!("{out}{err}");
    assert_eq!(code, 1, "{all}");
    assert!(
        all.contains("error[bynk.package.version_conflict]"),
        "nothing left to choose: {all}"
    );

    // An untrusted reason is printed sanitised: no control characters.
    add(&root, &reg, &key, "1.2.0", 4);
    let (code, out, err) = bynk(&app, &root.join("fourth-home"), &["check"], &[]);
    assert_eq!(code, 0, "{out}{err}");
    let hostile = index_text(&reg).replace(
        "\"version\":\"1.2.0\",",
        "\"version\":\"1.2.0\",\"withdrawn\":\"evil\\u001b[2K\\rerror[bynk.fake]: pwned\",",
    );
    std::fs::write(reg.join("acme/money"), hostile).unwrap();
    let (code, out, err) = bynk(&app, &root.join("fifth-home"), &["check"], &[]);
    let all = format!("{out}{err}");
    assert_eq!(code, 1, "{all}");
    assert!(!all.contains('\u{1b}') && !all.contains('\r'), "{all:?}");
    assert!(
        all.contains(
            "was withdrawn by registry `acme-internal`: \"evil[2Kerror[bynk.fake]: pwned\""
        ),
        "{all}"
    );

    // Read through a mirror, the message says so.
    let mirror = root.join("mirror");
    copy_dir(&reg, &mirror);
    write(
        &app.join("bynk.toml"),
        &std::fs::read_to_string(app.join("bynk.toml"))
            .unwrap()
            .replace(&file_url(&reg), &file_url(&root.join("unreachable")))
            .replace(
                "organisations = [\"acme\"]\n",
                &format!(
                    "organisations = [\"acme\"]\n{pin}\n\n[registries.corp]\nindex = \"{}\"\nmirror-of = \"acme-internal\"\n",
                    file_url(&mirror)
                ),
            ),
    );
    let (code, out, err) = bynk(&app, &root.join("sixth-home"), &["check"], &[]);
    let all = format!("{out}{err}");
    assert_eq!(code, 1, "{all}");
    assert!(all.contains("error[bynk.registry.withdrawn]"), "{all}");
    assert!(
        all.contains("= note: read through the mirror `corp` — the origin wasn't consulted"),
        "{all}"
    );
}

#[test]
fn a_withdrawn_version_is_not_added_again_or_unyanked() {
    let root = scratch("refusals");
    let (reg, key, _) = registry(&root);
    let pkg = add(&root, &reg, &key, "1.0.0", 2);
    let (code, out) = withdraw(&root, &reg, "1.0.0", "a takedown");
    assert_eq!(code, 0, "{out}");
    let before = index_text(&reg);

    // The very same tree again: a retry must not restore a takedown.
    let (code, out, err) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "add",
            reg.to_str().unwrap(),
            pkg.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code, 1, "{out}{err}");
    assert!(
        err.contains("was withdrawn from the registry and can't be published again"),
        "{err}"
    );
    let (code, out, err) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "yank",
            reg.to_str().unwrap(),
            "acme/money",
            "1.0.0",
            "--undo",
        ],
        &[],
    );
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("can't be un-yanked"), "{err}");
    assert_eq!(index_text(&reg), before);
    // Yanking it again is harmless.
    registry_ok(
        &root,
        &["yank", reg.to_str().unwrap(), "acme/money", "1.0.0"],
    );
    assert_eq!(index_text(&reg), before);
}

/// A running `bynk registry serve`, stopped on drop.
struct Server {
    child: Child,
    port: u16,
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Server {
    fn start(root: &Path, reg: &Path, key: &Path) -> Server {
        let mut child = Command::new(bin())
            .args([
                "registry",
                "serve",
                reg.to_str().unwrap(),
                "--key",
                key.to_str().unwrap(),
                "--tokens",
                root.join("tokens.toml").to_str().unwrap(),
                "--listen",
                "127.0.0.1:0",
            ])
            .env("BYNK_HOME", root.join("home"))
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
        let mut port = None;
        for line in lines.by_ref() {
            let line = line.unwrap();
            if let Some(rest) = line.split(" on http://127.0.0.1:").nth(1) {
                port = rest.split_whitespace().next().and_then(|p| p.parse().ok());
                break;
            }
        }
        let port = port.expect("the server reported its address");
        std::thread::spawn(move || for _ in lines {});
        Server { child, port }
    }

    fn index(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// A raw request: `(status, body)`.
    fn request(&self, method: &str, path: &str, token: &str, body: &[u8]) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\nAuthorization: Bearer {token}\r\n\r\n",
            body.len()
        );
        s.write_all(head.as_bytes()).unwrap();
        s.write_all(body).unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        let status = text
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let body = text
            .split("\r\n\r\n")
            .nth(1)
            .unwrap_or_default()
            .to_string();
        (status, body)
    }
}

/// Mint a token into `root/tokens.toml` for `acme` that can publish and yank.
fn token(root: &Path) -> String {
    let tokens = root.join("tokens.toml");
    let (code, out, err) = bynk(
        root,
        &root.join("home"),
        &[
            "registry",
            "token",
            tokens.to_str().unwrap(),
            "--org",
            "acme",
            "--can",
            "publish,yank",
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    out.trim().to_string()
}

/// The `outcome` of check `id` in a conformance run's JSON report.
fn outcome(json: &str, id: &str) -> String {
    let at = json
        .find(&format!("\"id\": \"{id}\""))
        .unwrap_or_else(|| panic!("no check `{id}` in {json}"));
    let start = json[..at].rfind('{').unwrap();
    let end = at + json[at..].find('}').unwrap();
    let object = &json[start..end];
    let o = object.find("\"outcome\": \"").unwrap() + "\"outcome\": \"".len();
    object[o..o + object[o..].find('"').unwrap()].to_string()
}

#[test]
fn the_api_refuses_a_withdrawn_version_and_conformance_checks_withdrawals() {
    let root = scratch("api");
    let (reg, key, _) = registry(&root);
    let t = token(&root);
    let server = Server::start(&root, &reg, &key);
    let home = root.join("home");
    let pkg = package(
        &root,
        "1.0.0",
        2,
        &format!(
            "[registries.acme-internal]\nindex = \"{}\"\norganisations = [\"acme\"]\n",
            server.index()
        ),
    );
    let env = [("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", t.as_str())];
    let (code, out, err) = bynk(&pkg, &home, &["publish"], &env);
    assert_eq!(code, 0, "{out}{err}");
    let conformance = |home: &Path| {
        let (code, out, err) = bynk(
            &root,
            home,
            &[
                "registry",
                "conformance",
                &server.index(),
                "--read-only",
                "--package",
                "acme/money",
                "--format",
                "json",
            ],
            &[],
        );
        (code, format!("{out}{err}"))
    };
    let (code, json) = conformance(&root.join("c1"));
    assert_eq!(code, 0, "{json}");
    assert_eq!(outcome(&json, "read.withdrawn"), "pass", "{json}");
    assert!(json.contains("has no withdrawn versions"), "{json}");

    let archive = archive_of(&reg, "1.0.0");
    let kept = std::fs::read(&archive).unwrap();
    let (code, out) = withdraw(&root, &reg, "1.0.0", "a takedown");
    assert_eq!(code, 0, "{out}");

    // A `PUT` of the withdrawn version — the same tree — is `410 withdrawn`.
    let (code, out, err) = bynk(&pkg, &home, &["publish"], &env);
    assert_eq!(code, 1, "{out}{err}");
    assert!(err.contains("410") && err.contains("withdrawn"), "{err}");
    let (status, body) = server.request("PUT", "/api/acme/money/1.0.0", &t, &kept);
    assert_eq!(status, 410, "{body}");
    assert!(body.contains("\"error\":\"withdrawn\""), "{body}");
    // An un-yank is `409 withdrawn`; a yank is harmless.
    let (status, body) = server.request("POST", "/api/acme/money/1.0.0/unyank", &t, b"");
    assert_eq!(status, 409, "{body}");
    assert!(body.contains("\"error\":\"withdrawn\""), "{body}");
    let (status, body) = server.request("POST", "/api/acme/money/1.0.0/yank", &t, b"");
    assert_eq!(status, 200, "{body}");
    assert!(index_text(&reg).contains("\"withdrawn\":\"a takedown\""));

    // A consistent withdrawal passes; the archive check skips a package whose
    // every version is withdrawn.
    let (code, json) = conformance(&root.join("c2"));
    assert_eq!(code, 0, "{json}");
    assert_eq!(outcome(&json, "read.withdrawn"), "pass", "{json}");
    assert!(json.contains("1 withdrawn version(s)"), "{json}");
    assert_eq!(outcome(&json, "read.archive"), "skip", "{json}");

    // Still serving the archive fails it.
    std::fs::write(&archive, &kept).unwrap();
    let (code, json) = conformance(&root.join("c3"));
    assert_eq!(code, 1, "{json}");
    assert_eq!(outcome(&json, "read.withdrawn"), "fail", "{json}");
    assert!(json.contains("is still served"), "{json}");
    std::fs::remove_file(&archive).unwrap();

    // So does a withdrawn line that isn't yanked.
    std::fs::write(
        reg.join("acme/money"),
        index_text(&reg).replace("\"yanked\":true", "\"yanked\":false"),
    )
    .unwrap();
    let (code, json) = conformance(&root.join("c4"));
    assert_eq!(code, 1, "{json}");
    assert_eq!(outcome(&json, "read.withdrawn"), "fail", "{json}");
    assert!(json.contains("is withdrawn but not yanked"), "{json}");

    // A withdrawal written as a codec's tagged `Option` isn't read as absent
    // — which would pass `read.withdrawn` for want of a withdrawn line — but
    // fails, as does a tagged `toolchain`.
    let served = index_text(&reg);
    std::fs::write(
        reg.join("acme/money"),
        served.replace(
            "\"withdrawn\":\"a takedown\"",
            r#""withdrawn":{"kind":"Some","value":"a takedown"}"#,
        ),
    )
    .unwrap();
    let (code, json) = conformance(&root.join("c5"));
    assert_eq!(code, 1, "{json}");
    assert_eq!(outcome(&json, "read.withdrawn"), "fail", "{json}");
    assert_eq!(outcome(&json, "read.lines"), "fail", "{json}");
    assert!(
        json.contains("acme/money 1.0.0's `withdrawn` is an object, not a string"),
        "{json}"
    );
    std::fs::write(
        reg.join("acme/money"),
        served.replace("\"toolchain\":null", r#""toolchain":{"kind":"None"}"#),
    )
    .unwrap();
    let (code, json) = conformance(&root.join("c6"));
    assert_eq!(code, 1, "{json}");
    assert_eq!(outcome(&json, "read.lines"), "fail", "{json}");
    assert!(
        json.contains("acme/money 1.0.0's `toolchain` is an object, not a string or `null`"),
        "{json}"
    );
}
