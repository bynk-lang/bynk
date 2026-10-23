//! The publish API (packaging slice 4c, #2287), over the real binaries:
//! `bynk registry serve` on a loopback port, `bynk registry token`,
//! `bynk publish` and `bynk yank`. No test touches the network. Each test has
//! its own `BYNK_HOME`.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("registry-publish")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(path: &Path, contents: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

/// `bynk <args>` in `cwd` with `BYNK_HOME = home` and `env`.
fn bynk(cwd: &Path, home: &Path, args: &[&str], env: &[(&str, &str)]) -> (i32, String, String) {
    let mut cmd = Command::new(bin());
    cmd.args(args)
        .current_dir(cwd)
        .env("BYNK_HOME", home)
        .env("XDG_CONFIG_HOME", home.join("config"))
        .env_remove("BYNK_REGISTRY_ACME_INTERNAL_TOKEN")
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

/// A registry at `root/registry` (id `acme`), its key at `root/acme.key`.
fn registry(root: &Path) -> (PathBuf, PathBuf) {
    let dir = root.join("registry");
    let key = root.join("acme.key");
    let (code, _, err) = bynk(
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
        &[],
    );
    assert_eq!(code, 0, "{err}");
    (dir, key)
}

/// Mint a token into `root/tokens.toml` for `orgs` that `can`.
fn token(root: &Path, orgs: &[&str], can: &str) -> String {
    let tokens = root.join("tokens.toml");
    let mut args = vec!["registry", "token", tokens.to_str().unwrap()];
    for o in orgs {
        args.extend(["--org", o]);
    }
    args.extend(["--can", can]);
    let (code, out, err) = bynk(root, &root.join("home"), &args, &[]);
    assert_eq!(code, 0, "{err}");
    out.trim().to_string()
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
        // Keep draining its stderr so it never blocks.
        std::thread::spawn(move || for _ in lines {});
        Server { child, port }
    }

    fn index(&self) -> String {
        format!("http://localhost:{}", self.port)
    }

    /// A raw request: `(status, body)`.
    fn request(&self, method: &str, path: &str, token: Option<&str>, body: &[u8]) -> (u16, String) {
        let mut s = TcpStream::connect(("127.0.0.1", self.port)).unwrap();
        let mut head = format!(
            "{method} {path} HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\nConnection: close\r\n",
            body.len()
        );
        if let Some(t) = token {
            head.push_str(&format!("Authorization: Bearer {t}\r\n"));
        }
        head.push_str("\r\n");
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

/// The `[registries]` table routing `acme` to `index`.
fn routing(index: &str) -> String {
    format!("[registries.acme-internal]\nindex = \"{index}\"\norganisations = [\"acme\"]\n")
}

/// A package `acme/<name>` at `version` under `root/pkgs`, routed to `index`.
fn package(root: &Path, index: &str, name: &str, version: &str, factor: i32) -> PathBuf {
    let pkg = root.join("pkgs").join(format!("{name}-{version}"));
    write(
        &pkg.join("bynk.toml"),
        &format!(
            "[package]\nname = \"acme/{name}\"\nversion = \"{version}\"\n\n{}",
            routing(index)
        ),
    );
    write(
        &pkg.join("src/currency.bynk"),
        &format!("commons currency\n\nfn double(c: Int) -> Int {{\n  c * {factor}\n}}\n"),
    );
    pkg
}

fn publish(pkg: &Path, home: &Path, token: Option<&str>, extra: &[&str]) -> (i32, String) {
    let mut args = vec!["publish"];
    args.extend(extra);
    let env: Vec<(&str, &str)> = token
        .map(|t| vec![("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", t)])
        .unwrap_or_default();
    let (code, _, err) = bynk(pkg, home, &args, &env);
    (code, err)
}

fn index_text(reg: &Path, name: &str) -> String {
    std::fs::read_to_string(reg.join("acme").join(name)).unwrap_or_default()
}

/// A stored archive under `dir`, found by its header.
fn an_archive(dir: &Path) -> Option<Vec<u8>> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(a) = an_archive(&p) {
                return Some(a);
            }
        } else if let Ok(bytes) = std::fs::read(&p)
            && bytes.starts_with(b"bynk-tree-v1\n")
        {
            return Some(bytes);
        }
    }
    None
}

#[test]
fn a_published_package_is_signed_by_the_server_and_consumed() {
    let root = scratch("e2e");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish,yank");
    let tokens = std::fs::read_to_string(root.join("tokens.toml")).unwrap();
    assert!(!tokens.contains(&t), "only the hash is stored: {tokens}");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(root.join("tokens.toml"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the tokens file is private");
    }
    let server = Server::start(&root, &reg, &key);
    let config = std::fs::read_to_string(reg.join("config.json")).unwrap();
    assert!(
        config.contains("\"api\""),
        "serve records the API: {config}"
    );
    let home = root.join("home");

    let pkg = package(&root, &server.index(), "money", "1.0.0", 3);
    let (code, err) = publish(&pkg, &home, Some(&t), &[]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("published acme/money 1.0.0 (sha256-") && err.contains("`acme-internal`"),
        "{err}"
    );
    assert!(!err.contains(&t), "{err}");
    assert!(index_text(&reg, "money").contains("\"1.0.0\""));

    // The same tree again is a retry (#2311, D10): it succeeds, and nothing
    // changes.
    let before = index_text(&reg, "money");
    let (code, err) = publish(&pkg, &home, Some(&t), &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(index_text(&reg, "money"), before);

    // A different tree at the same version is refused, and nothing changes.
    let pkg = package(&root, &server.index(), "money", "1.0.0", 4);
    let (code, err) = publish(&pkg, &home, Some(&t), &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("error[bynk.registry.publish_refused]")
            && err.contains("HTTP 409")
            && err.contains("version_exists"),
        "{err}"
    );
    assert_eq!(index_text(&reg, "money"), before);

    // `--dry-run` checks and packs, and uploads nothing.
    let next = package(&root, &server.index(), "money", "1.1.0", 4);
    let (code, err) = publish(&next, &home, Some(&t), &["--dry-run"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("would publish acme/money 1.1.0"), "{err}");
    assert!(!index_text(&reg, "money").contains("1.1.0"));

    // A consumer resolves, verifies and locks it like any registry package.
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        &format!(
            "[project]\nname = \"app\"\n\n{}\n[dependencies]\nmoney = {{ package = \"acme/money\", version = \"^1\" }}\n",
            routing(&server.index())
        ),
    );
    write(
        &app.join("src/books.bynk"),
        "commons books\n\nuses money.currency\n\nfn total(n: Int) -> Int {\n  double(n)\n}\n",
    );
    let (code, _, err) = bynk(&app, &root.join("consumer"), &["check"], &[]);
    assert_eq!(code, 0, "{err}");
    let lock = std::fs::read_to_string(app.join("bynk.lock")).unwrap();
    assert!(
        lock.contains("version = \"1.0.0\"\nsource = \"registry+acme-internal\""),
        "{lock}"
    );
}

#[test]
fn a_publish_needs_a_token_for_the_organisation_and_the_action() {
    let root = scratch("auth");
    let (reg, key) = registry(&root);
    let other = token(&root, &["globex"], "publish,yank");
    let yank_only = token(&root, &["acme"], "yank");
    let server = Server::start(&root, &reg, &key);
    let home = root.join("home");
    let pkg = package(&root, &server.index(), "money", "1.0.0", 3);

    let (code, err) = publish(&pkg, &home, None, &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("HTTP 401") && err.contains("BYNK_REGISTRY_ACME_INTERNAL_TOKEN"),
        "{err}"
    );
    let (code, err) = publish(&pkg, &home, Some("bynk_not-a-real-token"), &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("HTTP 401") && err.contains("isn't known"),
        "{err}"
    );
    let (code, err) = publish(&pkg, &home, Some(&other), &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("HTTP 403") && err.contains("organisation `acme`"),
        "{err}"
    );
    let (code, err) = publish(&pkg, &home, Some(&yank_only), &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("HTTP 403") && err.contains("can't `publish`"),
        "{err}"
    );
    assert!(index_text(&reg, "money").is_empty(), "nothing was admitted");
}

#[test]
fn yank_and_unyank_go_through_the_api() {
    let root = scratch("yank");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish,yank");
    let publish_only = token(&root, &["acme"], "publish");
    let server = Server::start(&root, &reg, &key);
    let home = root.join("home");
    let pkg = package(&root, &server.index(), "money", "1.0.0", 3);
    let (code, err) = publish(&pkg, &home, Some(&t), &[]);
    assert_eq!(code, 0, "{err}");

    let yank = |token: &str, extra: &[&str]| {
        let mut args = vec!["yank", "acme/money", "1.0.0"];
        args.extend(extra);
        bynk(
            &pkg,
            &home,
            &args,
            &[("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", token)],
        )
    };
    let (code, _, err) = yank(&publish_only, &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("HTTP 403") && err.contains("can't `yank`"),
        "{err}"
    );
    assert!(index_text(&reg, "money").contains("\"yanked\":false"));

    let (code, _, err) = yank(&t, &[]);
    assert_eq!(code, 0, "{err}");
    assert!(
        err.contains("yanked acme/money 1.0.0 on `acme-internal`"),
        "{err}"
    );
    assert!(index_text(&reg, "money").contains("\"yanked\":true"));

    let (code, _, err) = yank(&t, &["--undo"]);
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("un-yanked acme/money 1.0.0"), "{err}");
    assert!(index_text(&reg, "money").contains("\"yanked\":false"));

    let (code, _, err) = bynk(
        &pkg,
        &home,
        &["yank", "acme/money", "9.9.9"],
        &[("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", &t)],
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("HTTP 404"), "{err}");
}

#[test]
fn the_server_admits_only_what_the_url_names_and_serves_only_the_directory() {
    let root = scratch("admission");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish,yank");
    let server = Server::start(&root, &reg, &key);
    let home = root.join("home");
    let pkg = package(&root, &server.index(), "money", "1.0.0", 3);
    let (code, err) = publish(&pkg, &home, Some(&t), &[]);
    assert_eq!(code, 0, "{err}");
    let archive = an_archive(&reg).expect("the server stored the archive");

    // An upload must be the package and version its URL names.
    let (status, body) = server.request("PUT", "/api/acme/other/1.0.0", Some(&t), &archive);
    assert_eq!(status, 400, "{body}");
    assert!(body.contains("invalid_package"), "{body}");
    let (status, body) = server.request("PUT", "/api/acme/money/2.0.0", Some(&t), &archive);
    assert_eq!(status, 400, "{body}");
    let (status, body) = server.request("PUT", "/api/acme/money/1.0.1", Some(&t), b"junk");
    assert_eq!(status, 400, "{body}");
    // A tampered archive whose path climbs out of the package.
    let mut evil = b"bynk-tree-v1\n../escape.txt\x005\x00hello".to_vec();
    evil.extend_from_slice(b"bynk.toml\x000\x00");
    let (status, body) = server.request("PUT", "/api/acme/money/1.0.2", Some(&t), &evil);
    assert_eq!(status, 400, "{body}");
    assert!(!root.join("escape.txt").exists() && !reg.join("escape.txt").exists());
    assert!(index_text(&reg, "other").is_empty());

    // An upload declared past the cap is refused before it is read.
    let mut s = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
    s.set_read_timeout(Some(std::time::Duration::from_secs(20)))
        .unwrap();
    write!(
        s,
        "PUT /api/acme/money/1.0.3 HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {t}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        65 * 1024 * 1024
    )
    .unwrap();
    let mut head = [0u8; 12];
    s.read_exact(&mut head).unwrap();
    assert_eq!(&head, b"HTTP/1.1 413", "{}", String::from_utf8_lossy(&head));
    drop(s);

    // Unknown endpoints, other methods, and paths out of the directory.
    let (status, _) = server.request("DELETE", "/api/acme/money/1.0.0", Some(&t), b"");
    assert_eq!(status, 404);
    let (status, _) = server.request("POST", "/config.json", None, b"");
    assert_eq!(status, 405);
    let (status, _) = server.request("GET", "/../acme.key", None, b"");
    assert_eq!(status, 404);
    let (status, _) = server.request("GET", "/../tokens.toml", None, b"");
    assert_eq!(status, 404);
    let (status, body) = server.request("GET", "/config.json", None, b"");
    assert_eq!(status, 200, "{body}");
}

#[test]
fn concurrent_publishes_are_serialised() {
    let root = scratch("concurrent");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish");
    let server = Server::start(&root, &reg, &key);
    let index = server.index();
    let handles: Vec<_> = (0..4)
        .map(|i| {
            let root = root.clone();
            let index = index.clone();
            let t = t.clone();
            std::thread::spawn(move || {
                let pkg = package(&root, &index, "money", &format!("1.{i}.0"), i + 2);
                publish(&pkg, &root.join(format!("home{i}")), Some(&t), &[])
            })
        })
        .collect();
    for h in handles {
        let (code, err) = h.join().unwrap();
        assert_eq!(code, 0, "{err}");
    }
    let text = index_text(&reg, "money");
    assert_eq!(text.lines().count(), 4, "{text}");
    for i in 0..4 {
        assert!(text.contains(&format!("\"1.{i}.0\"")), "{text}");
    }
}

#[test]
fn a_static_registry_points_to_registry_add() {
    let root = scratch("static");
    let (reg, _) = registry(&root);
    let path = reg.display().to_string().replace('\\', "/");
    let index = if path.starts_with('/') {
        format!("file://{path}")
    } else {
        format!("file:///{path}")
    };
    let pkg = package(&root, &index, "money", "1.0.0", 3);
    let (code, err) = publish(&pkg, &root.join("home"), None, &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("error[bynk.registry.publish_refused]") && err.contains("bynk registry add"),
        "{err}"
    );

    // An `api` the index's scheme doesn't allow is refused outright.
    let config = std::fs::read_to_string(reg.join("config.json")).unwrap();
    std::fs::write(
        reg.join("config.json"),
        config.replacen('{', "{\"api\":\"http://localhost:1/api\",", 1),
    )
    .unwrap();
    let (code, err) = publish(&pkg, &root.join("home"), None, &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("publish API at `http://localhost:1/api`") && err.contains("doesn't allow"),
        "{err}"
    );
}

/// A fake registry over `dir` that accepts every `PUT` and stores nothing —
/// or, with `redirect`, answers every `PUT` with a redirect.
fn liar(dir: PathBuf, redirect: bool) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut line = String::new();
            if reader.read_line(&mut line).is_err() {
                continue;
            }
            let mut parts = line.split_whitespace();
            let method = parts.next().unwrap_or_default().to_string();
            let path = parts.next().unwrap_or("/").to_string();
            let mut length = 0usize;
            loop {
                let mut h = String::new();
                if reader.read_line(&mut h).is_err() || h.trim().is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap_or(0);
                }
            }
            let mut body = vec![0; length];
            let _ = reader.read_exact(&mut body);
            let mut out = stream;
            if method == "PUT" && redirect {
                let _ = write!(
                    out,
                    "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:1/elsewhere\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                continue;
            }
            let (status, bytes) = if method == "PUT" {
                ("201 Created", b"{}".to_vec())
            } else {
                match std::fs::read(dir.join(path.trim_start_matches('/'))) {
                    Ok(b) => ("200 OK", b),
                    Err(_) => ("404 Not Found", Vec::new()),
                }
            };
            let _ = write!(
                out,
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                bytes.len()
            );
            let _ = out.write_all(&bytes);
        }
    });
    port
}

#[test]
fn a_server_that_signs_another_tree_is_caught_at_read_back() {
    let root = scratch("liar");
    let (reg, key) = registry(&root);
    // The registry already signed a different 1.0.0; the liar claims to have
    // taken ours.
    let index = format!("http://localhost:{}", liar(reg.clone(), false));
    let theirs = package(&root, &index, "money", "1.0.0", 2);
    let (code, _, err) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "add",
            reg.to_str().unwrap(),
            theirs.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
        ],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let config = std::fs::read_to_string(reg.join("config.json")).unwrap();
    std::fs::write(
        reg.join("config.json"),
        config.replacen('{', "{\"api\":\"api\",", 1),
    )
    .unwrap();
    std::fs::remove_dir_all(root.join("pkgs")).unwrap();
    let ours = package(&root, &index, "money", "1.0.0", 3);
    let (code, err) = publish(&ours, &root.join("home"), Some("anything"), &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("bynk.package.checksum_mismatch") && err.contains("but bynk sent"),
        "{err}"
    );
}

#[test]
fn a_path_dependency_is_refused_before_upload() {
    let root = scratch("path-dep");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish");
    let server = Server::start(&root, &reg, &key);
    write(
        &root.join("helper/bynk.toml"),
        "[package]\nname = \"acme/helper\"\nversion = \"0.1.0\"\n",
    );
    write(
        &root.join("helper/src/help.bynk"),
        "commons help\n\nfn one() -> Int {\n  1\n}\n",
    );
    let pkg = package(&root, &server.index(), "money", "1.0.0", 3);
    let manifest = std::fs::read_to_string(pkg.join("bynk.toml")).unwrap();
    write(
        &pkg.join("bynk.toml"),
        &format!(
            "{manifest}\n[dependencies]\nhelper = {{ package = \"acme/helper\", path = \"../../helper\" }}\n"
        ),
    );
    let (code, err) = publish(&pkg, &root.join("home"), Some(&t), &[]);
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("publish_refused") && err.contains("path or git dependency"),
        "{err}"
    );
    assert!(index_text(&reg, "money").is_empty());
}

#[test]
fn serve_refuses_a_key_or_tokens_file_inside_the_registry() {
    let root = scratch("inside");
    let (reg, key) = registry(&root);
    let inner = reg.join("acme.key");
    std::fs::copy(&key, &inner).unwrap();
    write(&root.join("tokens.toml"), "");
    let (code, _, err) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "serve",
            reg.to_str().unwrap(),
            "--key",
            inner.to_str().unwrap(),
            "--tokens",
            root.join("tokens.toml").to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
        ],
        &[],
    );
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("inside the registry directory"), "{err}");
    write(&reg.join("tokens.toml"), "");
    let (code, _, err) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "serve",
            reg.to_str().unwrap(),
            "--key",
            key.to_str().unwrap(),
            "--tokens",
            reg.join("tokens.toml").to_str().unwrap(),
            "--listen",
            "127.0.0.1:0",
        ],
        &[],
    );
    assert_ne!(code, 0, "{err}");
    assert!(err.contains("inside the registry directory"), "{err}");
}

#[test]
fn a_redirected_publish_is_refused_not_followed() {
    let root = scratch("redirect");
    let (reg, _) = registry(&root);
    let config = std::fs::read_to_string(reg.join("config.json")).unwrap();
    std::fs::write(
        reg.join("config.json"),
        config.replacen('{', "{\"api\":\"api\",", 1),
    )
    .unwrap();
    let index = format!("http://localhost:{}", liar(reg.clone(), true));
    let pkg = package(&root, &index, "money", "1.0.0", 3);
    let (code, err) = publish(&pkg, &root.join("home"), Some("anything"), &[]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("never follows"), "{err}");
}

#[test]
fn a_token_names_only_valid_organisations() {
    let root = scratch("token-org");
    let tokens = root.join("tokens.toml");
    let (code, _, err) = bynk(
        &root,
        &root.join("home"),
        &[
            "registry",
            "token",
            tokens.to_str().unwrap(),
            "--org",
            "ac\"me",
        ],
        &[],
    );
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("isn't an organisation"), "{err}");
    assert!(!tokens.exists());
}

/// `pkg`'s `bynk.toml` with `acme-internal` also served by the mirror `near`
/// at `index`.
fn through_mirror(pkg: &Path, index: &str) {
    let manifest = pkg.join("bynk.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    write(
        &manifest,
        &format!("{text}\n[registries.near]\nindex = \"{index}\"\nmirror-of = \"acme-internal\"\n"),
    );
}

#[test]
fn a_publish_or_yank_never_goes_through_a_mirror() {
    let root = scratch("mirror");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish,yank");
    let server = Server::start(&root, &reg, &key);
    let home = root.join("home");
    // The mirror is the registry's own server, and carries a valid token: a
    // write through it would succeed, so only the refusal stops it.
    let tokens = [
        ("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", t.as_str()),
        ("BYNK_REGISTRY_NEAR_TOKEN", t.as_str()),
    ];
    let mirrored = package(&root, &server.index(), "money", "1.0.0", 3);
    through_mirror(&mirrored, &server.index());

    for extra in [&[][..], &["--dry-run"][..]] {
        let mut args = vec!["publish"];
        args.extend(extra);
        let (code, _, err) = bynk(&mirrored, &home, &args, &tokens);
        assert_eq!(code, 1, "{extra:?}: {err}");
        assert!(
            err.contains("error[bynk.registry.publish_refused]")
                && err.contains("through the mirror `near`"),
            "{err}"
        );
        assert!(!err.contains(&t), "{err}");
    }
    assert!(index_text(&reg, "money").is_empty(), "nothing was admitted");

    // Published directly, it can't be yanked through the mirror either.
    let direct = package(&root, &server.index(), "money", "1.0.1", 3);
    let (code, err) = publish(&direct, &home, Some(&t), &[]);
    assert_eq!(code, 0, "{err}");
    for undo in [&[][..], &["--undo"][..]] {
        let mut args = vec!["yank", "acme/money", "1.0.1"];
        args.extend(undo);
        let (code, _, err) = bynk(&mirrored, &home, &args, &tokens);
        assert_eq!(code, 1, "{undo:?}: {err}");
        assert!(err.contains("through the mirror `near`"), "{err}");
    }
    assert!(index_text(&reg, "money").contains("\"yanked\":false"));

    // The refusal comes from `[registries]` alone: an unreachable mirror is
    // refused the same way, not reported unavailable.
    let dead = package(&root, &server.index(), "money", "1.0.2", 3);
    through_mirror(&dead, "http://localhost:1");
    let (code, _, err) = bynk(&dead, &home, &["publish"], &tokens);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("through the mirror `near`"), "{err}");
}

/// R3.2 (#2383): a package `<org>/money` at `version`, with `acme` and
/// `globex` both routed to `index`.
fn org_package(root: &Path, index: &str, org: &str, version: &str) -> PathBuf {
    let pkg = root.join("pkgs").join(format!("{org}-{version}"));
    write(
        &pkg.join("bynk.toml"),
        &format!(
            "[package]\nname = \"{org}/money\"\nversion = \"{version}\"\n\n[registries.acme-internal]\nindex = \"{index}\"\norganisations = [\"acme\", \"globex\"]\n"
        ),
    );
    write(
        &pkg.join("src/currency.bynk"),
        "commons currency\n\nfn double(c: Int) -> Int {\n  c * 2\n}\n",
    );
    pkg
}

#[test]
fn each_organisation_publishes_with_its_own_saved_token() {
    let root = scratch("per-org");
    let (reg, key) = registry(&root);
    let acme = token(&root, &["acme"], "publish,yank");
    let globex = token(&root, &["globex"], "publish,yank");
    let server = Server::start(&root, &reg, &key);
    let home = root.join("home");
    let credentials = home.join("config/bynk/credentials.toml");
    let save = |text: String| write(&credentials, &text);
    let own = |org: &str, t: &str| {
        format!("[registries.acme-internal.organisations.{org}]\ntoken = \"{t}\"\n")
    };

    // Two organisations on one registry, each with its own slot, no variable.
    save(format!("{}{}", own("acme", &acme), own("globex", &globex)));
    for org in ["acme", "globex"] {
        let (code, err) = publish(
            &org_package(&root, &server.index(), org, "1.0.0"),
            &home,
            None,
            &[],
        );
        assert_eq!(code, 0, "{org}: {err}");
    }
    assert!(index_text(&reg, "money").contains("1.0.0"));
    // A yank acts for the organisation too.
    let (code, _, err) = bynk(
        &org_package(&root, &server.index(), "globex", "1.0.0"),
        &home,
        &["yank", "globex/money", "1.0.0"],
        &[],
    );
    assert_eq!(code, 0, "{err}");

    // An organisation without a slot falls back to the registry-wide one.
    save(format!(
        "[registries.acme-internal]\ntoken = \"{globex}\"\n\n{}",
        own("acme", &acme)
    ));
    let (code, err) = publish(
        &org_package(&root, &server.index(), "globex", "1.1.0"),
        &home,
        None,
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let (code, err) = publish(
        &org_package(&root, &server.index(), "acme", "1.1.0"),
        &home,
        None,
        &[],
    );
    assert_eq!(
        code, 0,
        "the own slot wins over the registry-wide one: {err}"
    );

    // The wrong organisation's token: the refusal names where it came from.
    save(format!(
        "[registries.acme-internal]\ntoken = \"{globex}\"\n"
    ));
    let (code, err) = publish(
        &org_package(&root, &server.index(), "acme", "1.2.0"),
        &home,
        None,
        &[],
    );
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("HTTP 403")
            && err.contains("the token came from `[registries.acme-internal] token`")
            && err.contains("bynk org token mint acme --save"),
        "{err}"
    );
    save(own("acme", &globex));
    let (code, err) = publish(
        &org_package(&root, &server.index(), "acme", "1.2.0"),
        &home,
        None,
        &[],
    );
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("the token came from `[registries.acme-internal.organisations.acme] token`"),
        "{err}"
    );

    // The variable still overrides every slot.
    save(own("acme", &acme));
    let (code, err) = publish(
        &org_package(&root, &server.index(), "acme", "1.2.0"),
        &home,
        Some(&globex),
        &[],
    );
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("the token came from `BYNK_REGISTRY_ACME_INTERNAL_TOKEN`"),
        "{err}"
    );

    // No token anywhere for `globex`: the hint says how to save one.
    let (code, err) = publish(
        &org_package(&root, &server.index(), "globex", "1.2.0"),
        &home,
        None,
        &[],
    );
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("HTTP 401") && err.contains("bynk org token mint globex --save"),
        "{err}"
    );
}
