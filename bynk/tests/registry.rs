//! Registries (packaging slice 4a, #2274), over the real binaries, against
//! static registries built with `bynk registry init`/`add` and read as
//! `file://` indexes — or over a local HTTP test server for auth. No test
//! touches the network. Each test has its own `BYNK_HOME`.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use bynk::registry::protocol::{Config, KeyEntry, SigningKey, rotation_payload};

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
        .join("registry")
        .join(name);
    if dir.exists() {
        writable(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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

/// `bin <args>` in `cwd` with `BYNK_HOME = home` and `env`.
fn run(name: &str, cwd: &Path, home: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String) {
    let mut cmd = Command::new(bin(name));
    cmd.args(args)
        .current_dir(cwd)
        .env("BYNK_HOME", home)
        .env_remove("BYNK_BYNKC")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for (k, v) in env {
        cmd.env(k, v);
    }
    if name == "bynk" && args.first() == Some(&"test") && bin("bynkc").exists() {
        cmd.env("BYNK_BYNKC", bin("bynkc"));
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

fn bynk(cwd: &Path, home: &Path, args: &[&str]) -> (i32, String) {
    run("bynk", cwd, home, args, &[])
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

/// A registry at `root/registry` (id `acme`), its key at `root/acme.key`.
fn registry(root: &Path) -> (PathBuf, PathBuf, String) {
    let dir = root.join("registry");
    let key = root.join("acme.key");
    let (code, out) = bynk(
        root,
        &root.join("home"),
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
    let pin = out
        .lines()
        .find(|l| l.starts_with("key = "))
        .unwrap()
        .to_string();
    (dir, key, pin)
}

/// Publish `acme/<name>` at `version` with `body` as its `commons <unit>`,
/// and `deps` (TOML lines for `[dependencies]`).
fn publish(
    root: &Path,
    reg: &Path,
    key: &Path,
    name: &str,
    version: &str,
    unit: &str,
    body: &str,
    deps: &str,
) {
    let pkg = root.join("pkgs").join(format!("{name}-{version}"));
    let mut manifest = format!("[package]\nname = \"acme/{name}\"\nversion = \"{version}\"\n");
    if !deps.is_empty() {
        manifest.push_str(&format!("\n[dependencies]\n{deps}"));
    }
    write(&pkg.join("bynk.toml"), &manifest);
    write(&pkg.join(format!("src/{unit}.bynk")), body);
    let (code, out) = bynk(
        root,
        &root.join("home"),
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

fn currency(factor: i32) -> String {
    format!("commons currency\n\nfn double(c: Int) -> Int {{\n  c * {factor}\n}}\n")
}

/// A consumer at `root/app` routing `acme` to `index`, depending on
/// `acme/money` at `range`, with a commons using it.
fn app(root: &Path, index: &str, range: &str, extra: &str) -> PathBuf {
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        &format!(
            "[project]\nname = \"app\"\n\n[registries.acme-internal]\nindex = \"{index}\"\norganisations = [\"acme\"]\n{extra}\n[dependencies]\nmoney = {{ package = \"acme/money\", version = \"{range}\" }}\n"
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

#[test]
fn a_registry_package_is_selected_verified_locked_and_run() {
    let root = scratch("e2e");
    let (reg, key, _) = registry(&root);
    for (v, f) in [("1.2.0", 2), ("1.4.2", 3), ("2.0.0", 4)] {
        publish(&root, &reg, &key, "money", v, "currency", &currency(f), "");
    }
    let home = root.join("home");
    let app = app(&root, &file_url(&reg), "^1", "");
    write(
        &app.join("tests/books_test.bynk"),
        "suite books\n\ncase \"through the registry\" {\n  expect total(2) == 6\n}\n",
    );
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        out.contains("trusting registry `acme-internal` (id `acme`)"),
        "{out}"
    );
    let text = lock(&app);
    assert!(
        text.contains("version = \"1.4.2\"\nsource = \"registry+acme-internal\""),
        "{text}"
    );
    assert!(
        text.contains("[[registry]]\nname = \"acme-internal\"\nid = \"acme\"\nkey = \"ed25519-fp:"),
        "{text}"
    );
    let (code, out) = bynk(&app, &home, &["check", "--locked", "--offline"]);
    assert_eq!(code, 0, "{out}");
    if bin("bynkc").exists() {
        let (code, out) = run("bynkc", &app, &home, &["check", "."], &[]);
        assert_eq!(code, 0, "{out}");
        let (code, out) = bynk(&app, &home, &["test"]);
        assert_eq!(code, 0, "{out}");
        assert!(out.contains("through the registry"), "{out}");
    }
}

#[test]
fn ranges_from_every_requirer_choose_one_version_or_conflict() {
    let root = scratch("select");
    let (reg, key, _) = registry(&root);
    for (v, f) in [("1.2.0", 2), ("1.4.2", 3), ("1.5.0", 5)] {
        publish(&root, &reg, &key, "money", v, "currency", &currency(f), "");
    }
    publish(
        &root,
        &reg,
        &key,
        "ledger",
        "0.1.0",
        "entries",
        "commons entries\n\nfn post(n: Int) -> Int {\n  n\n}\n",
        "money = { package = \"acme/money\", version = \"<1.5\" }\n",
    );
    let home = root.join("home");
    let app = app(&root, &file_url(&reg), "^1.2", "");
    let manifest = std::fs::read_to_string(app.join("bynk.toml")).unwrap();
    write(
        &app.join("bynk.toml"),
        &format!("{manifest}ledger = {{ package = \"acme/ledger\", version = \"^0.1\" }}\n"),
    );
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(
        lock(&app).contains("name = \"acme/money\"\nversion = \"1.4.2\""),
        "the highest both admit: {}",
        lock(&app)
    );

    // A range no version satisfies alongside the others.
    write(
        &app.join("bynk.toml"),
        &format!("{manifest}ledger = {{ package = \"acme/ledger\", version = \"^0.1\" }}\n")
            .replace("\"^1.2\"", "\"^1.5\""),
    );
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("error[bynk.package.version_conflict]"),
        "{out}"
    );
    assert!(
        out.contains("`^1.5` (from this project)") && out.contains("`<1.5` (from acme/ledger)"),
        "{out}"
    );
}

#[test]
fn a_lock_holds_until_update_and_a_yank_never_breaks_it() {
    let root = scratch("update");
    let (reg, key, _) = registry(&root);
    publish(
        &root,
        &reg,
        &key,
        "money",
        "1.4.2",
        "currency",
        &currency(3),
        "",
    );
    let home = root.join("home");
    let app = app(&root, &file_url(&reg), "^1", "");
    assert_eq!(bynk(&app, &home, &["check"]).0, 0);
    publish(
        &root,
        &reg,
        &key,
        "money",
        "1.5.0",
        "currency",
        &currency(5),
        "",
    );
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(lock(&app).contains("version = \"1.4.2\""));
    // Yanking the locked version doesn't break the locked build.
    let (code, out) = bynk(
        &root,
        &home,
        &[
            "registry",
            "yank",
            reg.to_str().unwrap(),
            "acme/money",
            "1.4.2",
        ],
    );
    assert_eq!(code, 0, "{out}");
    let (code, out) = bynk(&app, &home, &["check", "--locked"]);
    assert_eq!(code, 0, "{out}");
    let (code, out) = bynk(&app, &home, &["update", "money"]);
    assert_eq!(code, 0, "{out}");
    assert!(lock(&app).contains("version = \"1.5.0\""), "{}", lock(&app));
}

#[test]
fn keys_are_pinned_trusted_once_and_rotated_by_chain() {
    let root = scratch("trust");
    let (reg, key, pin) = registry(&root);
    publish(
        &root,
        &reg,
        &key,
        "money",
        "1.4.2",
        "currency",
        &currency(3),
        "",
    );
    let home = root.join("home");
    // A pinned key that isn't the registry's.
    let (stranger, _) = SigningKey::generate().unwrap();
    let app = app(
        &root,
        &file_url(&reg),
        "^1",
        &format!("key = \"{}\"\n", stranger.public()),
    );
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.registry.key_mismatch]"), "{out}");
    // The right pin works, with no first-use notice.
    let app = self::app(&root, &file_url(&reg), "^1", &format!("{pin}\n"));
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("on first use"), "{out}");

    // Rotate: a new key signed by the old one. The lock keeps its trust
    // anchor, and lines signed by either key verify.
    let app = self::app(&root, &file_url(&reg), "^1", "");
    std::fs::remove_file(app.join("bynk.lock")).ok();
    assert_eq!(bynk(&app, &home, &["check"]).0, 0);
    let old = SigningKey::from_pkcs8(&std::fs::read(&key).unwrap()).unwrap();
    let (new, new_doc) = SigningKey::generate().unwrap();
    let mut config = Config::parse(&std::fs::read(reg.join("config.json")).unwrap()).unwrap();
    config.keys.push(KeyEntry {
        key: new.public(),
        sig: Some(old.sign(&rotation_payload("acme", &new.public()))),
    });
    std::fs::write(reg.join("config.json"), config.render()).unwrap();
    std::fs::write(&key, new_doc).unwrap();
    let before = lock(&app);
    publish(
        &root,
        &reg,
        &key,
        "money",
        "1.5.0",
        "currency",
        &currency(5),
        "",
    );
    let (code, out) = bynk(&app, &home, &["update"]);
    assert_eq!(code, 0, "{out}");
    let after = lock(&app);
    assert!(after.contains("version = \"1.5.0\""), "{after}");
    let anchor = |l: &str| {
        l.lines()
            .find(|x| x.starts_with("key = "))
            .map(str::to_owned)
    };
    assert_eq!(
        anchor(&after),
        anchor(&before),
        "the trust anchor is unchanged"
    );

    // A config whose keys don't lead from the trusted key.
    let (rogue, _) = SigningKey::generate().unwrap();
    let mut bad = config.clone();
    bad.keys = vec![KeyEntry {
        key: rogue.public(),
        sig: None,
    }];
    std::fs::write(reg.join("config.json"), bad.render()).unwrap();
    let (code, out) = bynk(&app, &home, &["update"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.registry.key_mismatch]"), "{out}");
}

#[test]
fn forged_and_replayed_lines_and_archives_are_refused() {
    let root = scratch("forge");
    let (reg, key, pin) = registry(&root);
    publish(
        &root,
        &reg,
        &key,
        "money",
        "1.4.2",
        "currency",
        &currency(3),
        "",
    );
    publish(
        &root,
        &reg,
        &key,
        "payments",
        "1.0.0",
        "currency",
        &currency(9),
        "",
    );
    let home = root.join("home");
    let app = app(&root, &file_url(&reg), "^1", &format!("{pin}\n"));
    // A line edited after signing.
    let index = reg.join("acme/money");
    let original = std::fs::read_to_string(&index).unwrap();
    std::fs::write(&index, original.replace("1.4.2", "1.4.3")).unwrap();
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.registry.bad_signature]"), "{out}");
    // `acme/payments`' signed line served as `acme/money`.
    std::fs::write(
        &index,
        std::fs::read_to_string(reg.join("acme/payments")).unwrap(),
    )
    .unwrap();
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.registry.bad_signature]"), "{out}");
    // A rewritten archive.
    std::fs::write(&index, &original).unwrap();
    let archive = std::fs::read_dir(reg.join("dl/sha256"))
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .find(|p| std::fs::read(p).unwrap().windows(5).any(|w| w == b"c * 3"))
        .unwrap();
    let bytes = std::fs::read(&archive).unwrap();
    let tampered: Vec<u8> = String::from_utf8(bytes)
        .unwrap()
        .replace("c * 3", "c * 7")
        .into_bytes();
    std::fs::write(&archive, tampered).unwrap();
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("error[bynk.package.checksum_mismatch]"),
        "{out}"
    );
    // A malformed archive (here, one that climbs out of the package).
    std::fs::write(&archive, b"bynk-tree-v1\n../escape\x001\0x").unwrap();
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.registry.bad_archive]"), "{out}");
}

#[test]
fn routing_mirrors_and_source_mismatch() {
    let root = scratch("routing");
    let (reg, key, pin) = registry(&root);
    publish(
        &root,
        &reg,
        &key,
        "money",
        "1.4.2",
        "currency",
        &currency(3),
        "",
    );
    let home = root.join("home");
    // An organisation no registry lists.
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        "[project]\nname = \"app\"\n\n[dependencies]\nmoney = { package = \"acme/money\", version = \"^1\" }\n",
    );
    write(&app.join("src/books.bynk"), "commons books\n");
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.registry.unavailable]"), "{out}");
    assert!(
        out.contains("the first-party registry isn't live yet"),
        "{out}"
    );

    // A mirror: a copy of the registry, verified against the origin's key.
    let mirror = root.join("mirror");
    copy_dir(&reg, &mirror);
    let app = self::app(
        &root,
        &file_url(&root.join("unreachable")),
        "^1",
        &format!(
            "{pin}\n\n[registries.corp]\nindex = \"{}\"\nmirror-of = \"acme-internal\"\n",
            file_url(&mirror)
        ),
    );
    std::fs::remove_file(app.join("bynk.lock")).ok();
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 0, "{out}");
    // A mirror that forges a line can't pass it off.
    let index = mirror.join("acme/money");
    std::fs::write(
        &index,
        std::fs::read_to_string(&index)
            .unwrap()
            .replace("1.4.2", "1.9.9"),
    )
    .unwrap();
    std::fs::remove_file(app.join("bynk.lock")).ok();
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("error[bynk.registry.bad_signature]"), "{out}");

    // A locked package whose organisation is re-routed.
    let app = self::app(&root, &file_url(&reg), "^1", "");
    std::fs::remove_file(app.join("bynk.lock")).ok();
    assert_eq!(bynk(&app, &home, &["check"]).0, 0);
    let manifest = std::fs::read_to_string(app.join("bynk.toml")).unwrap();
    write(
        &app.join("bynk.toml"),
        &manifest.replace("[registries.acme-internal]", "[registries.elsewhere]"),
    );
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("error[bynk.registry.source_mismatch]"),
        "{out}"
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        let dest = to.join(e.file_name());
        if p.is_dir() {
            copy_dir(&p, &dest)
        } else {
            std::fs::copy(&p, &dest).unwrap();
        }
    }
}

/// A one-thread HTTP server over `dir`: `GET /<path>` serves the file. When
/// `token` is set, a request without `Authorization: Bearer <token>` is 401.
/// `redirect` maps a path to a `Location`. Records each request's path and
/// whether it carried an `Authorization` header.
fn serve(
    dir: PathBuf,
    token: Option<&'static str>,
    redirect: Option<(&'static str, String)>,
) -> (u16, Arc<Mutex<Vec<(String, bool)>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_string();
            let mut auth = None;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h
                    .strip_prefix("authorization: ")
                    .or_else(|| h.strip_prefix("Authorization: "))
                {
                    auth = Some(v.trim().to_string());
                }
            }
            log.lock().unwrap().push((path.clone(), auth.is_some()));
            let mut out = stream;
            if let Some((from, to)) = &redirect
                && path == *from
            {
                let _ = write!(
                    out,
                    "HTTP/1.1 302 Found\r\nLocation: {to}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                continue;
            }
            if let Some(t) = token
                && auth.as_deref() != Some(&format!("Bearer {t}"))
            {
                let _ = write!(
                    out,
                    "HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                continue;
            }
            match std::fs::read(dir.join(path.trim_start_matches('/'))) {
                Ok(body) => {
                    let _ = write!(
                        out,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = out.write_all(&body);
                }
                Err(_) => {
                    let _ = write!(
                        out,
                        "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                }
            }
        }
    });
    (port, seen)
}

#[test]
fn tokens_go_only_to_the_registrys_origin() {
    let root = scratch("auth");
    let (reg, key, pin) = registry(&root);
    publish(
        &root,
        &reg,
        &key,
        "money",
        "1.4.2",
        "currency",
        &currency(3),
        "",
    );
    let mut config = Config::parse(&std::fs::read(reg.join("config.json")).unwrap()).unwrap();
    config.token_auth = true;
    std::fs::write(reg.join("config.json"), config.render()).unwrap();
    let home = root.join("home");
    let (port, _) = serve(reg.clone(), Some("s3cret"), None);
    let index = format!("http://localhost:{port}");
    let app = app(&root, &index, "^1", &format!("{pin}\n"));
    // No token: refused, with how to supply one — and no secret in sight.
    let (code, out) = bynk(&app, &home, &["check"]);
    assert_eq!(code, 1, "{out}");
    assert!(
        out.contains("error[bynk.registry.http_failed]")
            && out.contains("BYNK_REGISTRY_ACME_INTERNAL_TOKEN"),
        "{out}"
    );
    let (code, out) = run(
        "bynk",
        &app,
        &home,
        &["check"],
        &[("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", "s3cret")],
    );
    assert_eq!(code, 0, "{out}");
    assert!(!out.contains("s3cret"), "{out}");

    // A redirect to another origin drops the token.
    let (other, other_seen) = serve(reg.clone(), None, None);
    let (port2, _) = serve(
        reg.clone(),
        Some("s3cret"),
        Some((
            "/acme/money",
            format!("http://127.0.0.1:{other}/acme/money"),
        )),
    );
    let app = self::app(
        &root,
        &format!("http://localhost:{port2}"),
        "^1",
        &format!("{pin}\n"),
    );
    std::fs::remove_file(app.join("bynk.lock")).ok();
    let fresh = root.join("home2");
    let _ = run(
        "bynk",
        &app,
        &fresh,
        &["check"],
        &[("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", "s3cret")],
    );
    let hits = other_seen.lock().unwrap().clone();
    assert!(
        hits.iter().any(|(p, _)| p == "/acme/money"),
        "the redirect was followed: {hits:?}"
    );
    assert!(
        hits.iter().all(|(_, auth)| !auth),
        "no token crossed origins: {hits:?}"
    );
}

#[test]
fn bynk_add_writes_the_dependency_and_keeps_comments() {
    let root = scratch("add");
    let (reg, key, pin) = registry(&root);
    publish(
        &root,
        &reg,
        &key,
        "money",
        "1.4.2",
        "currency",
        &currency(3),
        "",
    );
    let home = root.join("home");
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        &format!(
            "# the shop\n[project]\nname = \"app\"  # display only\n\n[registries.acme-internal]\nindex = \"{}\"\norganisations = [\"acme\"]\n{pin}\n",
            file_url(&reg)
        ),
    );
    write(&app.join("src/books.bynk"), "commons books\n");
    let (code, out) = bynk(&app, &home, &["add", "acme/money"]);
    assert_eq!(code, 0, "{out}");
    let manifest = std::fs::read_to_string(app.join("bynk.toml")).unwrap();
    assert!(
        manifest.contains("# the shop") && manifest.contains("# display only"),
        "{manifest}"
    );
    assert!(
        manifest.contains("money = { package = \"acme/money\", version = \"^1.4.2\" }"),
        "{manifest}"
    );
    assert!(lock(&app).contains("version = \"1.4.2\""));
    let (code, out) = bynk(&app, &home, &["add", "acme/money"]);
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("already has `money`"), "{out}");
}
