//! Registry key delegation and revocation (first-party registry track R5b,
//! #2316), over the real binaries: a root chain key delegates signing, a
//! leaked delegate is revoked and replaced without re-pinning, a delegate can
//! grant or revoke nothing, and `rotate` extends the chain. Registries are
//! read as `file://` indexes, or through `bynk registry serve` on loopback for
//! conformance. No test touches the network.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use bynk::registry::protocol::{Config, KeyEntry, SigningKey, delegation_payload};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("registry-keys")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// `bynk <args>` in `cwd` with `BYNK_HOME = home`: `(code, stdout+stderr)`.
fn bynk(cwd: &Path, home: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(cwd)
        .env("BYNK_HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("spawn");
    (
        out.status.code().unwrap_or(-1),
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
    )
}

fn file_url(dir: &Path) -> String {
    let path = dir.display().to_string().replace('\\', "/");
    if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    }
}

/// `bynk registry <args>` from `root`, asserting success.
fn registry_ok(root: &Path, args: &[&str]) -> String {
    let mut all = vec!["registry"];
    all.extend(args);
    let (code, out) = bynk(root, &root.join("home"), &all, &[]);
    assert_eq!(code, 0, "{args:?}: {out}");
    out
}

/// A registry at `root/reg` (id `acme`) with its root key `root/root.key`;
/// returns the `key = "…"` pin line.
fn init(root: &Path) -> String {
    let out = registry_ok(
        root,
        &["init", "reg", "--id", "acme", "--key-out", "root.key"],
    );
    out.lines()
        .find(|l| l.starts_with("key = "))
        .unwrap()
        .to_string()
}

/// Publish `acme/money` at `version`, signing with `key`.
fn add(root: &Path, key: &str, version: &str, factor: i32) -> (i32, String) {
    let pkg = root.join("pkgs").join(format!("money-{version}"));
    write(
        &pkg.join("bynk.toml"),
        &format!("[package]\nname = \"acme/money\"\nversion = \"{version}\"\n"),
    );
    write(
        &pkg.join("src/currency.bynk"),
        &format!("commons currency\n\nfn double(c: Int) -> Int {{\n  c * {factor}\n}}\n"),
    );
    bynk(
        root,
        &root.join("home"),
        &[
            "registry",
            "add",
            "reg",
            pkg.to_str().unwrap(),
            "--key",
            key,
        ],
        &[],
    )
}

/// A consumer of `acme/money ^1` at `root/app`, routing `acme` to the
/// registry with its root key pinned.
fn app(root: &Path, pin: &str) -> PathBuf {
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        &format!(
            "[project]\nname = \"app\"\n\n[registries.acme-internal]\nindex = \"{}\"\norganisations = [\"acme\"]\n{pin}\n\n[dependencies]\nmoney = {{ package = \"acme/money\", version = \"^1\" }}\n",
            file_url(&root.join("reg"))
        ),
    );
    write(
        &app.join("src/books.bynk"),
        "commons books\n\nuses money.currency\n\nfn total(n: Int) -> Int {\n  double(n)\n}\n",
    );
    app
}

/// `bynk check` of the consumer with a fresh store, as a fresh clone would.
fn fresh_check(root: &Path, app: &Path, home: &str) -> (i32, String) {
    let _ = std::fs::remove_file(app.join("bynk.lock"));
    bynk(app, &root.join(home), &["check"], &[])
}

fn config(root: &Path) -> String {
    std::fs::read_to_string(root.join("reg/config.json")).unwrap()
}

fn public_key(out: &str) -> String {
    out.split_whitespace()
        .find(|w| w.starts_with("ed25519:"))
        .unwrap()
        .trim_end_matches(';')
        .to_string()
}

#[test]
fn a_leaked_delegate_is_revoked_and_replaced_without_repinning() {
    let root = scratch("recover");
    let pin = init(&root);
    assert!(config(&root).contains("\"protocol\": 1"));

    // The root delegates; the delegate signs. Protocol 2 from here on.
    let out = registry_ok(
        &root,
        &[
            "delegate",
            "reg",
            "--key",
            "root.key",
            "--delegate-out",
            "d1.key",
        ],
    );
    let d1 = public_key(&out);
    assert!(
        config(&root).contains("\"protocol\": 2"),
        "{}",
        config(&root)
    );
    let (code, out) = add(&root, "d1.key", "1.0.0", 2);
    assert_eq!(code, 0, "{out}");
    let app = app(&root, &pin);
    let (code, out) = fresh_check(&root, &app, "home-a");
    assert_eq!(code, 0, "{out}");
    let leaked_index = std::fs::read_to_string(root.join("reg/acme/money")).unwrap();

    // Revoking before re-signing would strand the line: refused.
    let (code, out) = bynk(
        &root,
        &root.join("home"),
        &["registry", "revoke", "reg", &d1, "--key", "root.key"],
        &[],
    );
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("no valid signer") && out.contains("resign"),
        "{out}"
    );

    // The recovery: delegate a new key, re-sign, revoke the leaked one.
    registry_ok(
        &root,
        &[
            "delegate",
            "reg",
            "--key",
            "root.key",
            "--delegate-out",
            "d2.key",
        ],
    );
    let out = registry_ok(&root, &["resign", "reg", "--key", "d2.key"]);
    assert!(out.contains("re-signed 1 index line"), "{out}");
    registry_ok(&root, &["revoke", "reg", &d1, "--key", "root.key"]);
    let (code, out) = fresh_check(&root, &app, "home-b");
    assert_eq!(code, 0, "same pin, no toolchain release: {out}");

    // A line still signed by the revoked delegate is refused, saying why.
    std::fs::write(root.join("reg/acme/money"), &leaked_index).unwrap();
    let (code, out) = fresh_check(&root, &app, "home-c");
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("bynk.registry.bad_signature") && out.contains("revoked") && out.contains(&d1),
        "{out}"
    );

    // The revoked delegate can no longer publish.
    let (code, out) = add(&root, "d1.key", "1.0.1", 3);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("revoked"), "{out}");
}

#[test]
fn a_delegate_can_grant_revoke_and_extend_nothing() {
    let root = scratch("limits");
    let pin = init(&root);
    registry_ok(
        &root,
        &[
            "delegate",
            "reg",
            "--key",
            "root.key",
            "--delegate-out",
            "d1.key",
        ],
    );
    let d2 = public_key(&registry_ok(
        &root,
        &[
            "delegate",
            "reg",
            "--key",
            "root.key",
            "--delegate-out",
            "d2.key",
        ],
    ));

    // Through the tooling: refused.
    for args in [
        vec![
            "registry",
            "delegate",
            "reg",
            "--key",
            "d1.key",
            "--delegate-out",
            "x.key",
        ],
        vec!["registry", "revoke", "reg", d2.as_str(), "--key", "d1.key"],
        vec![
            "registry",
            "rotate",
            "reg",
            "--key",
            "d1.key",
            "--key-out",
            "y.key",
        ],
    ] {
        let (code, out) = bynk(&root, &root.join("home"), &args, &[]);
        assert_eq!(code, 1, "{args:?}: {out}");
    }

    // By hand: a delegation signed by a delegate grants nothing. Its key's
    // lines are refused, and the client says it ignored the delegation.
    let d1 = SigningKey::from_pkcs8(&std::fs::read(root.join("d1.key")).unwrap()).unwrap();
    let (forged, forged_doc) = SigningKey::generate().unwrap();
    std::fs::write(root.join("forged.key"), forged_doc).unwrap();
    let mut cfg = Config::parse(config(&root).as_bytes()).unwrap();
    cfg.delegations.push(KeyEntry {
        key: forged.public(),
        sig: Some(d1.sign(&delegation_payload("acme", &forged.public()))),
    });
    std::fs::write(root.join("reg/config.json"), cfg.render()).unwrap();
    let (code, out) = add(&root, "forged.key", "1.0.0", 2);
    assert_eq!(code, 1, "the tooling refuses an ungranted key too: {out}");
    // Publish with a real delegate, then swap in the ungranted key's
    // signature, as a holder of it would.
    let (code, out) = add(&root, "d2.key", "1.0.0", 2);
    assert_eq!(code, 0, "{out}");
    // The client says it ignored the forged delegation, and resolves the
    // validly signed line.
    let app = app(&root, &pin);
    let (code, out) = fresh_check(&root, &app, "home-e");
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("no trusted key signed") && out.contains("ignoring it"),
        "{out}"
    );
    // Signed by the ungranted key instead, the line is refused.
    let text = std::fs::read_to_string(root.join("reg/acme/money")).unwrap();
    let mut line = bynk::registry::protocol::IndexLine::parse(text.trim()).unwrap();
    line.sig = forged.sign(&line.payload("acme"));
    std::fs::write(root.join("reg/acme/money"), line.render() + "\n").unwrap();
    let (code, out) = fresh_check(&root, &app, "home-f");
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("bynk.registry.bad_signature"), "{out}");
    // And the operator's recovery tooling won't launder it into a valid line.
    let (code, out) = bynk(
        &root,
        &root.join("home"),
        &["registry", "resign", "reg", "--key", "d2.key"],
        &[],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("refusing to re-sign"), "{out}");
}

#[test]
fn rotation_extends_the_chain_and_old_pins_follow_it() {
    let root = scratch("rotate");
    let pin = init(&root);
    let (code, out) = add(&root, "root.key", "1.0.0", 2);
    assert_eq!(code, 0, "{out}");
    registry_ok(
        &root,
        &[
            "rotate",
            "reg",
            "--key",
            "root.key",
            "--key-out",
            "root2.key",
        ],
    );
    assert!(
        config(&root).contains("\"protocol\": 1"),
        "rotation alone stays protocol 1"
    );
    // The old chain key is no longer current.
    let (code, out) = add(&root, "root.key", "1.1.0", 3);
    assert_eq!(code, 1, "{out}");
    let (code, out) = add(&root, "root2.key", "1.1.0", 3);
    assert_eq!(code, 0, "{out}");
    // A consumer pinned to the first key follows the chain to both lines.
    let app = app(&root, &pin);
    let (code, out) = fresh_check(&root, &app, "home-r");
    assert_eq!(code, 0, "{out}");
    let lock = std::fs::read_to_string(app.join("bynk.lock")).unwrap();
    assert!(lock.contains("version = \"1.1.0\""), "{lock}");
    // A key file inside the directory is refused.
    let (code, out) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "rotate",
            "reg",
            "--key",
            "root2.key",
            "--key-out",
            "reg/k.key",
        ],
        &[],
    );
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("inside the registry directory"), "{out}");
}

struct Server(Child, u16);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn serve(root: &Path, key: &str) -> Result<Server, String> {
    let mut child = Command::new(bin())
        .args([
            "registry",
            "serve",
            "reg",
            "--key",
            key,
            "--tokens",
            "tokens.toml",
            "--listen",
            "127.0.0.1:0",
        ])
        .current_dir(root)
        .env("BYNK_HOME", root.join("home"))
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn");
    let mut lines = BufReader::new(child.stderr.take().unwrap()).lines();
    let mut seen = String::new();
    for line in lines.by_ref() {
        let line = line.unwrap();
        seen.push_str(&line);
        seen.push('\n');
        if let Some(rest) = line.split(" on http://127.0.0.1:").nth(1) {
            let port = rest
                .split_whitespace()
                .next()
                .and_then(|p| p.parse().ok())
                .unwrap();
            std::thread::spawn(move || for _ in lines {});
            return Ok(Server(child, port));
        }
    }
    let _ = child.wait();
    Err(seen)
}

/// The outcome the JSON report gives check `id`.
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
fn a_server_signing_with_a_delegate_conforms_and_a_revoked_one_is_refused() {
    let root = scratch("serve");
    init(&root);
    let out = registry_ok(
        &root,
        &[
            "delegate",
            "reg",
            "--key",
            "root.key",
            "--delegate-out",
            "d1.key",
        ],
    );
    let d1 = public_key(&out);
    let token = registry_ok(&root, &["token", "tokens.toml", "--org", "acme"]);
    let token = token
        .lines()
        .find(|l| l.starts_with("bynk_"))
        .unwrap()
        .trim()
        .to_string();
    let server = serve(&root, "d1.key").expect("serves with a delegate");
    let index = format!("http://localhost:{}", server.1);
    let (code, json) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "conformance",
            &index,
            "--org",
            "acme",
            "--format",
            "json",
        ],
        &[("BYNK_REGISTRY_CONFORMANCE_TOKEN", &token)],
    );
    assert_eq!(code, 0, "{json}");
    assert_eq!(outcome(&json, "config.delegations"), "pass", "{json}");
    assert_eq!(outcome(&json, "publish.signer"), "pass", "{json}");
    assert!(json.contains("signed by the delegate"), "{json}");
    let published = json
        .split("\"published\": \"")
        .nth(1)
        .and_then(|r| r.split('"').next())
        .unwrap()
        .to_string();
    drop(server);

    // Revoked (forced, stranding its lines): the server won't start with it,
    // and a read-only check names the revoked signer.
    registry_ok(
        &root,
        &["revoke", "reg", &d1, "--key", "root.key", "--force"],
    );
    let refused = serve(&root, "d1.key")
        .err()
        .expect("a revoked key is refused");
    assert!(refused.contains("revoked"), "{refused}");
    registry_ok(
        &root,
        &[
            "delegate",
            "reg",
            "--key",
            "root.key",
            "--delegate-out",
            "d2.key",
        ],
    );
    let server = serve(&root, "d2.key").expect("serves with the new delegate");
    let index = format!("http://localhost:{}", server.1);
    let (code, json) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "conformance",
            &index,
            "--read-only",
            "--package",
            &published,
            "--format",
            "json",
        ],
        &[],
    );
    assert_eq!(code, 1, "{json}");
    assert_eq!(outcome(&json, "read.lines"), "fail", "{json}");
    assert_eq!(outcome(&json, "read.revoked"), "fail", "{json}");
    assert!(json.contains("signed by the revoked key"), "{json}");

    // A delegation no chain key signed fails `config.delegations`.
    drop(server);
    let d2 = SigningKey::from_pkcs8(&std::fs::read(root.join("d2.key")).unwrap()).unwrap();
    let (stray, _) = SigningKey::generate().unwrap();
    let mut cfg = Config::parse(config(&root).as_bytes()).unwrap();
    cfg.delegations.push(KeyEntry {
        key: stray.public(),
        sig: Some(d2.sign(&delegation_payload("acme", &stray.public()))),
    });
    std::fs::write(root.join("reg/config.json"), cfg.render()).unwrap();
    let server = serve(&root, "d2.key").expect("serves");
    let index = format!("http://localhost:{}", server.1);
    let (code, json) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "conformance",
            &index,
            "--read-only",
            "--format",
            "json",
        ],
        &[],
    );
    assert_eq!(code, 1, "{json}");
    assert_eq!(outcome(&json, "config.delegations"), "fail", "{json}");
}
