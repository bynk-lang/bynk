//! The JSON surface editors drive (#2472): `bynk search`, `bynk deps`, and
//! `--format json` on `add`, `fetch` and `publish --dry-run`, over a local
//! `bynk registry serve`. No test touches the network.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_bynk"))
}

fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join("registry-query")
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

fn index_text(reg: &Path, name: &str) -> String {
    std::fs::read_to_string(reg.join("acme").join(name)).unwrap_or_default()
}

fn json(text: &str) -> serde_json::Value {
    serde_json::from_str(text.trim()).unwrap_or_else(|e| panic!("not JSON ({e}): {text}"))
}

/// Publish `acme/money` at each of `versions` to `server`.
fn publish_versions(root: &Path, server: &Server, token: &str, versions: &[&str]) {
    for (i, v) in versions.iter().enumerate() {
        let pkg = package(root, &server.index(), "money", v, i as i32 + 2);
        let (code, _, err) = bynk(
            &pkg,
            &root.join("home"),
            &["publish"],
            &[("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", token)],
        );
        assert_eq!(code, 0, "{err}");
    }
}

fn app(root: &Path, server: &Server) -> PathBuf {
    let app = root.join("app");
    write(
        &app.join("bynk.toml"),
        &format!("[project]\nname = \"app\"\n\n{}", routing(&server.index())),
    );
    write(
        &app.join("src/books.bynk"),
        "commons books\n\nfn one() -> Int {\n  1\n}\n",
    );
    app
}

#[test]
fn search_lists_the_versions_and_the_newest_usable_one() {
    let root = scratch("search");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish,yank");
    let server = Server::start(&root, &reg, &key);
    publish_versions(&root, &server, &t, &["1.0.0", "1.1.0", "2.0.0"]);
    let home = root.join("home");
    let app = app(&root, &server);
    let yank = |undo: bool| {
        let mut args = vec!["yank", "acme/money", "2.0.0"];
        if undo {
            args.push("--undo");
        }
        let env = [("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", t.as_str())];
        let (code, _, err) = bynk(&app, &home, &args, &env);
        assert_eq!(code, 0, "{err}");
    };

    let (code, out, err) = bynk(
        &app,
        &home,
        &["search", "acme/money", "--format", "json"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    let doc = json(&out);
    assert_eq!(doc["package"], "acme/money");
    assert_eq!(doc["registry"], "acme-internal");
    assert_eq!(doc["latest"], "2.0.0");
    let versions: Vec<_> = doc["versions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["version"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(versions, ["1.0.0", "1.1.0", "2.0.0"]);
    assert_eq!(doc["versions"][0]["yanked"], false);
    assert!(
        doc["versions"][0]["checksum"]
            .as_str()
            .unwrap()
            .starts_with("sha256-")
    );

    // A yanked version is listed but is never "latest".
    yank(false);
    let (_, out, _) = bynk(
        &app,
        &home,
        &["search", "acme/money", "--format", "json"],
        &[],
    );
    let doc = json(&out);
    assert_eq!(doc["latest"], "1.1.0");
    assert_eq!(doc["versions"][2]["yanked"], true);
    yank(true);

    // The short form is for people.
    let (code, out, _) = bynk(&app, &home, &["search", "acme/money"], &[]);
    assert_eq!(code, 0);
    assert!(
        out.contains("acme/money on `acme-internal`: 2.0.0") && out.contains("  1.1.0"),
        "{out}"
    );
}

#[test]
fn search_fails_cleanly_for_an_unknown_package_and_an_unreachable_registry() {
    let root = scratch("search-fail");
    let (reg, key) = registry(&root);
    token(&root, &["acme"], "publish");
    let server = Server::start(&root, &reg, &key);
    let home = root.join("home");
    let app = app(&root, &server);
    let (code, out, err) = bynk(
        &app,
        &home,
        &["search", "acme/nothing", "--format", "json"],
        &[],
    );
    assert_eq!(code, 1);
    assert!(out.trim().is_empty(), "no JSON on failure: {out}");
    assert!(err.contains("has no package `acme/nothing`"), "{err}");

    let (code, _, err) = bynk(
        &app,
        &home,
        &["search", "not-a-package", "--format", "json"],
        &[],
    );
    assert_eq!(code, 1);
    assert!(err.contains("not-a-package"), "{err}");

    // An organisation nothing routes goes to the first-party registry, which
    // isn't live: the failure says so and exits non-zero (offline-tolerant).
    let (code, _, err) = bynk(
        &app,
        &home,
        &["search", "elsewhere/x", "--format", "json"],
        &[],
    );
    assert_eq!(code, 1);
    assert!(
        err.contains("bynk.registry.unavailable") || err.contains("isn't live yet"),
        "{err}"
    );
}

#[test]
fn add_fetch_and_deps_report_json_and_agree() {
    let root = scratch("add-fetch-deps");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish,yank");
    let server = Server::start(&root, &reg, &key);
    publish_versions(&root, &server, &t, &["1.0.0", "1.1.0"]);
    let home = root.join("home");
    let app = app(&root, &server);

    let (code, out, err) = bynk(&app, &home, &["deps", "--format", "json"], &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(json(&out)["dependencies"].as_array().unwrap().len(), 0);

    let (code, out, err) = bynk(
        &app,
        &home,
        &["add", "acme/money@^1", "--format", "json"],
        &[],
    );
    assert_eq!(code, 0, "{err}");
    assert_eq!(
        json(&out)["added"],
        serde_json::json!({ "alias": "money", "package": "acme/money", "version": "^1" })
    );
    assert!(
        std::fs::read_to_string(app.join("bynk.toml"))
            .unwrap()
            .contains("version = \"^1\"")
    );

    let (code, out, err) = bynk(&app, &home, &["deps", "--format", "json"], &[]);
    assert_eq!(code, 0, "{err}");
    let dep = &json(&out)["dependencies"][0];
    assert_eq!(dep["alias"], "money");
    assert_eq!(dep["package"], "acme/money");
    assert_eq!(dep["kind"], "registry");
    assert_eq!(dep["requested"], "^1");
    assert_eq!(dep["source"], "acme-internal");
    assert_eq!(dep["locked"], "1.1.0");
    assert_eq!(dep["dev"], false);

    // Nothing left to fetch: the lock is current.
    let (code, out, err) = bynk(&app, &home, &["fetch", "--format", "json"], &[]);
    assert_eq!(code, 0, "{err}");
    let doc = json(&out);
    assert_eq!(doc["wrote_lock"], false);
    assert_eq!(doc["fetched"].as_array().unwrap().len(), 0);

    // Without a lock, `deps` still names the registry the organisation routes to.
    std::fs::remove_file(app.join("bynk.lock")).unwrap();
    let (code, out, err) = bynk(&app, &home, &["deps", "--format", "json"], &[]);
    assert_eq!(code, 0, "{err}");
    let dep = &json(&out)["dependencies"][0];
    assert_eq!(dep["source"], "acme-internal");
    assert_eq!(dep["locked"], serde_json::Value::Null);

    // The next fetch writes the lock.
    let (code, out, err) = bynk(&app, &home, &["fetch", "--format", "json"], &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(json(&out)["wrote_lock"], true);
}

#[test]
fn publish_dry_run_reports_the_plan_as_json() {
    let root = scratch("publish-plan");
    let (reg, key) = registry(&root);
    let t = token(&root, &["acme"], "publish,yank");
    let server = Server::start(&root, &reg, &key);
    let home = root.join("home");
    let pkg = package(&root, &server.index(), "money", "1.0.0", 3);
    write(&pkg.join(".env"), "SECRET=1");
    write(&pkg.join("out/built.ts"), "built");
    let env = [("BYNK_REGISTRY_ACME_INTERNAL_TOKEN", t.as_str())];
    let (code, out, err) = bynk(
        &pkg,
        &home,
        &["publish", "--dry-run", "--format", "json"],
        &env,
    );
    assert_eq!(code, 0, "{err}");
    let doc = json(&out);
    assert_eq!(doc["package"], "acme/money");
    assert_eq!(doc["version"], "1.0.0");
    assert_eq!(doc["registry"], "acme-internal");
    assert_eq!(doc["dry_run"], true);
    assert!(doc["checksum"].as_str().unwrap().starts_with("sha256-"));
    let files: Vec<_> = doc["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        files,
        ["bynk.toml", "src/currency.bynk"],
        "no secrets, no build output"
    );
    assert!(doc["files"][1]["bytes"].as_u64().unwrap() > 0);
    assert!(
        index_text(&reg, "money").is_empty(),
        "a dry run uploads nothing"
    );

    // A tree that changes after it was planned is refused, and nothing is sent.
    let planned = doc["checksum"].as_str().unwrap().to_string();
    write(
        &pkg.join("src/currency.bynk"),
        "commons currency\n\nfn double(c: Int) -> Int {\n  c * 9\n}\n",
    );
    let (code, out, err) = bynk(
        &pkg,
        &home,
        &["publish", "--expect-checksum", &planned, "--format", "json"],
        &env,
    );
    assert_eq!(code, 1, "{err}");
    assert!(out.trim().is_empty(), "{out}");
    assert!(err.contains("changed since it was planned"), "{err}");
    assert!(index_text(&reg, "money").is_empty(), "nothing was uploaded");
    // Planned again, the new checksum is accepted.
    let (code, out, err) = bynk(
        &pkg,
        &home,
        &["publish", "--dry-run", "--format", "json"],
        &env,
    );
    assert_eq!(code, 0, "{err}");
    let planned = json(&out)["checksum"].as_str().unwrap().to_string();
    assert_ne!(
        planned,
        doc["checksum"].as_str().unwrap(),
        "the edit changed the checksum"
    );

    // The real publish prints the same document, `dry_run: false`.
    let (code, out, err) = bynk(
        &pkg,
        &home,
        &["publish", "--expect-checksum", &planned, "--format", "json"],
        &env,
    );
    assert_eq!(code, 0, "{err}");
    let doc = json(&out);
    assert_eq!(doc["dry_run"], false);
    assert!(index_text(&reg, "money").contains("\"1.0.0\""));
}
