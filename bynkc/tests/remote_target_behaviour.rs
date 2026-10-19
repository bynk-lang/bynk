//! #2138 (end-to-end testing track §3.5, §3.7, §3.8): `bynkc test --against
//! <name>` runs the `system` suites against a deployment named under
//! `[test.targets]`.
//!
//! The "deployment" here is a **local stand-in**: a Node HTTP server that
//! serves the emitted Worker's own `fetch`, as `wrangler dev` or Cloudflare
//! would, with the secret it verifies with. Nothing reaches the network beyond
//! `127.0.0.1`. The test checks:
//!
//! - a case drives the stand-in over HTTP, signing with the secret the
//!   runner's environment holds, and the document reports
//!   `transport: "remote:local"`;
//! - a wrong secret in the environment is rejected by the deployment, so the
//!   secret really comes from the environment, not a fixed test value;
//! - a case a remote run can't honour (`advance Clock`) refuses the run, and
//!   `--skip-unsupported` skips it, naming why;
//! - a bare URL, an unknown name, a `production = true` target and a missing
//!   secret are each refused before any request.
//!
//! Skipped when no TypeScript runner is on `PATH`, unless `BYNK_REQUIRE_TSC`
//! is set (the same contract as `behaviour_fixtures.rs`). The stand-in runs in
//! its own process group, stopped by a guard on drop.

#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

mod require;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const SOURCE: &str = r#"context shop.api

type UserId = String where NonEmpty
actor User { auth = Bearer(secret = "AUTH_SECRET"), identity = UserId }

type Item = { sku: String }

agent Cart {
  key owner: UserId
  store items: Map[String, Item]

  on call add(sku: String) -> Effect[Item] {
    let item = Item { sku: sku }
    let _ <- items.put(sku, item)
    item
  }
  on call size() -> Effect[Int] {
    items.values.count()
  }
}

service api from http {
  on POST("/cart") (body: Item) -> Effect[HttpResult[Item]] by u: User {
    let item <- Cart(u.identity).add(body.sku)
    Created(item)
  }
  on GET("/cart/size") () -> Effect[HttpResult[Int]] by u: User {
    let n <- Cart(u.identity).size()
    Ok(n)
  }
}
"#;

const SUITE: &str = r#"suite shop.api as system {
  case "a created item is counted over the wire" {
    let item <- api.POST("/cart", Item { sku: "w" }) by User("alice")
    expect item is Created(_)
    let n <- api.GET("/cart/size") by User("alice")
    expect n is Ok(1)
  }
}
"#;

const ADVANCES: &str = r#"suite shop.api as system {
  case "the clock moves between calls" {
    advance Clock by 16.minutes
    let n <- api.GET("/cart/size") by User("carol")
    expect n is Ok(0)
  }
}
"#;

/// Serves the emitted Worker's `fetch` on an ephemeral `127.0.0.1` port,
/// printing the port on stdout once it listens.
const STAND_IN: &str = r#"import http from "node:http";
const [out, secret] = process.argv.slice(2);
const worker = (await import(`${out}/workers/shop-api/index.js`)).default;
const handlers = await import(`${out}/workers/shop-api/handlers.js`);
const runtime = await import(`${out}/runtime.js`);
const env = { AUTH_SECRET: secret };
env.CART = runtime.__makeIntegrationDoNamespace(handlers.Cart, env);
const server = http.createServer(async (req, res) => {
  const chunks = [];
  for await (const c of req) chunks.push(c);
  const body = chunks.length ? Buffer.concat(chunks) : undefined;
  const request = new Request(`http://127.0.0.1${req.url}`, { method: req.method, headers: req.headers, body });
  const response = await worker.fetch(request, env);
  res.writeHead(response.status, Object.fromEntries(response.headers));
  res.end(Buffer.from(await response.arrayBuffer()));
});
server.listen(0, "127.0.0.1", () => console.log(server.address().port));
"#;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("bynk-remote-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn project(dir: &Path, suites: &[&str], manifest: &str) -> PathBuf {
    let proj = dir.join("proj");
    fs::create_dir_all(proj.join("src/shop")).unwrap();
    fs::create_dir_all(proj.join("tests")).unwrap();
    fs::write(proj.join("src/shop/api.bynk"), SOURCE).unwrap();
    for (i, suite) in suites.iter().enumerate() {
        fs::write(proj.join(format!("tests/s{i}.bynk")), suite).unwrap();
    }
    fs::write(
        proj.join("bynk.toml"),
        format!(
            "[project]\nname = \"remote\"\n\n[paths]\ninclude = [\"src\", \"tests\"]\n\n{manifest}"
        ),
    )
    .unwrap();
    proj
}

/// Stops the stand-in's process group on drop.
struct StandIn(Child);

impl Drop for StandIn {
    fn drop(&mut self) {
        // The stand-in leads its own process group (`process_group(0)`), so
        // `-<pid>` names exactly that group and nothing else on the machine.
        let group = Command::new("kill")
            .args(["-s", "KILL", "--"])
            .arg(format!("-{}", self.0.id()))
            .status();
        if !group.is_ok_and(|s| s.success()) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

fn start_stand_in(dir: &Path, out_js: &Path, secret: &str) -> (StandIn, u16) {
    let script = dir.join("stand-in.mjs");
    fs::write(&script, STAND_IN).unwrap();
    let mut cmd = Command::new("node");
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd
        .arg(&script)
        .arg(out_js)
        .arg(secret)
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn the stand-in");
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .expect("the stand-in prints its port");
    let port = line.trim().parse().expect("a port");
    (StandIn(child), port)
}

fn bynkc_test(
    proj: &Path,
    out: &Path,
    extra: &[&str],
    env: &[(&str, &str)],
) -> (bool, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_bynkc"));
    cmd.arg("test")
        .arg(proj)
        .arg("--output")
        .arg(out)
        .arg("--format")
        .arg("json");
    cmd.args(extra);
    cmd.env_remove("AUTH_SECRET");
    for (k, v) in env {
        cmd.env(k, v);
    }
    let o = cmd.output().expect("run bynkc test");
    (
        o.status.success(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn json(stdout: &str, stderr: &str) -> serde_json::Value {
    serde_json::from_str(stdout).unwrap_or_else(|e| {
        panic!("not a JSON document ({e})\nstdout:\n{stdout}\nstderr:\n{stderr}")
    })
}

fn runner_available() -> bool {
    if tool_exists("node") && (tool_exists("tsc") || tool_exists("npx") || tool_exists("tsx")) {
        return true;
    }
    if require::is_required(REQUIRE_ENV) {
        panic!("no TypeScript runner on PATH, but {REQUIRE_ENV} is set");
    }
    eprintln!("skipping: no TypeScript runner on PATH");
    false
}

#[test]
fn system_suites_run_against_a_named_target() {
    if !runner_available() {
        return;
    }
    let dir = scratch("run");
    let target = |port: u16| {
        format!(
            "[test.targets.local]\nurl.shop-api = \"http://127.0.0.1:{port}\"\nsecrets = [\"AUTH_SECRET\"]\n"
        )
    };
    // An in-process run first: it writes the compiled Worker the stand-in
    // serves (`out-js/workers/shop-api`).
    let proj = project(&dir, &[SUITE], "");
    let (ok, stdout, stderr) = bynkc_test(&proj, &dir.join("build/out"), &[], &[]);
    assert!(ok, "the in-process run passes\n{stdout}\n{stderr}");
    let (_stand_in, port) = start_stand_in(&dir, &dir.join("build/out-js"), "deployed-secret");
    let proj = project(&dir, &[SUITE], &target(port));

    // Against the stand-in, signing with the secret from the environment.
    let (ok, stdout, stderr) = bynkc_test(
        &proj,
        &dir.join("remote/out"),
        &["--against", "local"],
        &[("AUTH_SECRET", "deployed-secret")],
    );
    let doc = json(&stdout, &stderr);
    assert!(ok, "the remote run passes\n{stdout}\n{stderr}");
    assert_eq!(doc["passed"], 1, "{doc}");
    assert_eq!(doc["transport"], "remote:local", "{doc}");
    assert!(
        stderr.contains("against target `local`")
            && stderr.contains(&format!("http://127.0.0.1:{port}")),
        "the target is printed before the run: {stderr}"
    );

    // A different secret in the environment: the deployment rejects the
    // credential, so the runner really signs with the environment's value.
    let (ok, stdout, stderr) = bynkc_test(
        &proj,
        &dir.join("wrong/out"),
        &["--against", "local"],
        &[("AUTH_SECRET", "not-the-deployed-secret")],
    );
    let doc = json(&stdout, &stderr);
    assert!(!ok, "a wrong secret fails the run");
    assert_eq!(doc["failed"], 1, "{doc}");

    // A case a remote run can't honour refuses the run before any request...
    let proj = project(&dir, &[SUITE, ADVANCES], &target(port));
    let (ok, stdout, stderr) = bynkc_test(
        &proj,
        &dir.join("refused/out"),
        &["--against", "local"],
        &[("AUTH_SECRET", "deployed-secret")],
    );
    assert!(!ok);
    let doc = json(&stdout, &stderr);
    assert_eq!(doc["error"]["kind"], "compile", "{doc}");
    assert!(
        doc["error"]["diagnostics"]
            .to_string()
            .contains("bynk.test.remote_unsupported_advance"),
        "{doc}"
    );

    // ...and `--skip-unsupported` skips it, saying why.
    let (ok, stdout, stderr) = bynkc_test(
        &proj,
        &dir.join("skipped/out"),
        &["--against", "local", "--skip-unsupported"],
        &[("AUTH_SECRET", "deployed-secret")],
    );
    let doc = json(&stdout, &stderr);
    assert!(ok, "the supported case still passes\n{stdout}\n{stderr}");
    assert_eq!(doc["skipped"], 1, "{doc}");
    let cases = doc["suites"].to_string();
    assert!(
        cases.contains("\"skipped\"") && cases.contains("moves the harness clock"),
        "{doc}"
    );
    // The skipped case is reported in the suite the passing one ran in.
    let suites = doc["suites"].as_array().expect("suites");
    assert_eq!(suites.len(), 1, "one suite: {doc}");
    let outcomes: Vec<&str> = suites[0]["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .map(|c| c["outcome"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(outcomes, ["pass", "skipped"], "{doc}");

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn unsafe_targets_are_refused_before_any_request() {
    // No runner is needed: each refusal happens before the build.
    let dir = scratch("refuse");
    let manifest = "[test.targets.prod]\nurl.shop-api = \"http://127.0.0.1:9\"\nproduction = true\n\n[test.targets.preview]\nurl.shop-api = \"http://127.0.0.1:9\"\nsecrets = [\"AUTH_SECRET\"]\n";
    let proj = project(&dir, &[SUITE], manifest);
    for (args, needle) in [
        (
            vec!["--against", "https://shop.example.workers.dev"],
            "names a URL",
        ),
        (
            vec!["--against", "staging"],
            "the targets are `preview`, `prod`",
        ),
        (vec!["--against", "prod"], "marked `production = true`"),
        (
            vec!["--against", "preview"],
            "needs the secret `AUTH_SECRET`",
        ),
        (vec!["--against", "dev"], "run `bynk test --against dev`"),
    ] {
        let (ok, stdout, stderr) = bynkc_test(&proj, &dir.join("out"), &args, &[]);
        assert!(!ok, "{args:?} is refused");
        let doc = json(&stdout, &stderr);
        let message = doc["error"]["message"].as_str().unwrap_or_default();
        assert!(message.contains(needle), "{args:?}: {message}");
        assert!(!dir.join("out").exists(), "{args:?}: nothing is built");
    }
    let _ = fs::remove_dir_all(&dir);
}
