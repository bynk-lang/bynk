//! #4482: a `Map`, a `Set`, an indexed `Map` and a `Log` renamed with `replaces`, on a real `workerd`.
//!
//! Version 1 stores more entries than one batch of the key move in a `Map`, a `Set`, an indexed `Map` and
//! a `Log`; version 2 renames them (`visited replaces seen`, and so on) and builds with no `migrate from`.
//! After a restart on the same persisted state the entries read back under the new names, every one
//! of them, and a write after the rename lands beside them. The move itself, its batching and its
//! resume after a failure are pinned against in-memory storage in `bynk-emit/runtime/test`.
//!
//! The same toolchain skip and `BYNK_REQUIRE_WORKERD` gate as the other workerd tests; see
//! `idempotency_workerd.rs`.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;
#[path = "../wrangler/mod.rs"]
mod wrangler;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";

/// More entries than the move's batch (128), so it takes several.
const ENTRIES: usize = 300;

/// The agent: a `Map`, a `Set`, an indexed `Map` and a `Log`, named `names` and carrying `clauses`
/// (each empty, or ` replaces old`), behind a plain HTTP service.
fn source(names: [&str; 4], clauses: [&str; 4]) -> String {
    let [map, set, orders, trail] = names;
    let [map_clause, set_clause, orders_clause, trail_clause] = clauses;
    format!(
        "context shop\n\nconsumes bynk {{ Clock }}\n\ntype Order = {{ customer: String, total: Int }}\n\nagent Cart {{\n\tkey id: String\n\n\tstore {map}: Map[String, Int]{map_clause}\n\tstore {set}: Set[String]{set_clause}\n\tstore {orders}: Map[String, Order] @indexed(by: customer){orders_clause}\n\tstore {trail}: Log[Int]{trail_clause}\n\n\ton call put(k: String, n: Int, who: String) -> Effect[()] given Clock {{\n\t\tlet _ <- {map}.put(k, n)\n\t\tlet _ <- {set}.add(k)\n\t\tlet _ <- {orders}.put(k, Order {{ customer: who, total: n }})\n\t\tlet _ <- {trail}.append(n)\n\t\tEffect.pure(())\n\t}}\n\n\ton call at(k: String) -> Effect[Option[Int]] {{\n\t\tlet v <- {map}.get(k)\n\t\tEffect.pure(v)\n\t}}\n\n\ton call has(k: String) -> Effect[Bool] {{\n\t\tlet h <- {set}.contains(k)\n\t\tEffect.pure(h)\n\t}}\n\n\ton call sizes() -> Effect[Int] {{\n\t\tlet a <- {map}.size()\n\t\tlet b <- {set}.size()\n\t\tEffect.pure(a + b)\n\t}}\n\n\ton call ordersOf(who: String) -> Effect[Int] {{\n\t\t{orders}.filter((o) => o.customer == who).count()\n\t}}\n\n\ton call logged() -> Effect[Int] {{\n\t\t{trail}.since(Instant.fromEpochMillis(0)).count()\n\t}}\n}}\n\nservice api from http {{\n\ton GET(\"/put/:id/:k/:n/:who\") (id: String, k: String, n: Int, who: String) -> Effect[HttpResult[String]] by Visitor given Clock {{\n\t\tlet _ <- Cart(id).put(k, n, who)\n\t\tOk(\"ok\")\n\t}}\n\n\ton GET(\"/at/:id/:k\") (id: String, k: String) -> Effect[HttpResult[Option[Int]]] by Visitor {{\n\t\tlet v <- Cart(id).at(k)\n\t\tOk(v)\n\t}}\n\n\ton GET(\"/has/:id/:k\") (id: String, k: String) -> Effect[HttpResult[Bool]] by Visitor {{\n\t\tlet h <- Cart(id).has(k)\n\t\tOk(h)\n\t}}\n\n\ton GET(\"/sizes/:id\") (id: String) -> Effect[HttpResult[Int]] by Visitor {{\n\t\tlet n <- Cart(id).sizes()\n\t\tOk(n)\n\t}}\n\n\ton GET(\"/orders/:id/:who\") (id: String, who: String) -> Effect[HttpResult[Int]] by Visitor {{\n\t\tlet n <- Cart(id).ordersOf(who)\n\t\tOk(n)\n\t}}\n\n\ton GET(\"/logged/:id\") (id: String) -> Effect[HttpResult[Int]] by Visitor {{\n\t\tlet n <- Cart(id).logged()\n\t\tOk(n)\n\t}}\n}}\n"
    )
}

fn skip(reason: &str) -> bool {
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    eprintln!("\n!!! COLLECTION RENAME (WORKERD) SKIPPED !!!\n{reason}\n");
    true
}

/// `GET path` on `port`, as `(status, body)`.
fn get(port: u16, path: &str) -> Result<(u16, String), String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(60)))
        .map_err(|e| e.to_string())?;
    let request = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut response = String::new();
    stream
        .read_to_string(&mut response)
        .map_err(|e| e.to_string())?;
    let status = response
        .lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("unparseable response: {response:?}"))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.trim().to_string())
        .unwrap_or_default();
    Ok((status, body))
}

/// `bynk compile --target workers`, which also reads and writes the project's `bynk.schema.lock`.
fn compile(proj: &Path, out: &Path) -> Result<(), String> {
    let run = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["compile", "--target", "workers", "--emit", "js", "-o"])
        .arg(out)
        .arg(proj)
        .output()
        .expect("run bynk");
    if run.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{}{}",
            String::from_utf8_lossy(&run.stdout),
            String::from_utf8_lossy(&run.stderr)
        ))
    }
}

/// `wrangler dev` in `dir` on the state kept at `state`, once it serves; `None` (after a skip) when
/// it never does.
fn serve(dir: &Path, tmp: &Path, state: &Path, log: &str) -> Option<(wrangler::Wrangler, u16)> {
    let [port, inspector] = wrangler::ports::free_ports();
    let log_path = tmp.join(log);
    let out_log = fs::File::create(&log_path).unwrap();
    let err_log = out_log.try_clone().unwrap();
    let spawned = wrangler::Wrangler::spawn(
        wrangler::npx()
            .args([
                "dev",
                "--port",
                &port.to_string(),
                "--inspector-port",
                &inspector.to_string(),
                "--persist-to",
            ])
            .arg(state)
            .current_dir(dir)
            .stdout(Stdio::from(out_log))
            .stderr(Stdio::from(err_log)),
        &tmp.join("registry"),
    );
    let Ok(server) = spawned else {
        skip("could not launch npx");
        return None;
    };
    let deadline = Instant::now() + Duration::from_secs(180);
    while get(port, "/sizes/boot-probe").is_err() {
        if Instant::now() > deadline {
            let said = fs::read_to_string(&log_path).unwrap_or_default();
            drop(server);
            skip(&format!(
                "wrangler dev did not serve within the boot window (likely no network \
                 to provision {})\n--- wrangler log ---\n{said}",
                wrangler::SPEC
            ));
            return None;
        }
        std::thread::sleep(Duration::from_millis(500));
    }
    Some((server, port))
}

#[test]
fn a_renamed_map_and_set_keep_every_entry_on_workerd() {
    if !tools::tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    let tmp = std::env::temp_dir().join(format!(
        "bynk-collection-rename-workerd-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&tmp);
    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join("src/shop")).unwrap();
    fs::write(
        proj.join("bynk.toml"),
        "[project]\nname = \"collection-rename\"\nversion = \"0.1.0\"\n",
    )
    .unwrap();
    let state = tmp.join("state");
    let mut failures = Vec::new();

    // Version 1, as deployed: the entries live under `seen` and `tags`.
    fs::write(
        proj.join("src/shop/cart.bynk"),
        source(["seen", "tags", "orders", "trail"], ["", "", "", ""]),
    )
    .unwrap();
    if let Err(said) = compile(&proj, &tmp.join("out1")) {
        panic!("version 1 does not compile:\n{said}");
    }
    {
        let dir = tmp.join("out1/workers/shop");
        let Some((server, port)) = serve(&dir, &tmp, &state, "first.log") else {
            let _ = fs::remove_dir_all(&tmp);
            return;
        };
        for i in 0..ENTRIES {
            match get(
                port,
                &format!(
                    "/put/ada/k{i:03}/{i}/{}",
                    if i % 2 == 0 { "ada" } else { "bob" }
                ),
            ) {
                Ok((200, _)) => {}
                other => {
                    failures.push(format!("seeding entry {i}: {other:?}"));
                    break;
                }
            }
        }
        drop(server);
    }

    // Version 2: renamed with the clauses, and built with no `migrate from`.
    fs::write(
        proj.join("src/shop/cart.bynk"),
        source(
            ["visited", "labels", "purchases", "journal"],
            [
                " replaces seen",
                " replaces tags",
                " replaces orders",
                " replaces trail",
            ],
        ),
    )
    .unwrap();
    if let Err(said) = compile(&proj, &tmp.join("out2")) {
        panic!("a collection rename needs no migration, but the build says:\n{said}");
    }
    {
        let dir = tmp.join("out2/workers/shop");
        let Some((server, port)) = serve(&dir, &tmp, &state, "second.log") else {
            let _ = fs::remove_dir_all(&tmp);
            return;
        };
        match get(port, "/sizes/ada") {
            Ok((200, body)) if body == (2 * ENTRIES).to_string() => {}
            other => failures.push(format!(
                "after the rename, the two collections hold {other:?} entries, expected {}",
                2 * ENTRIES
            )),
        }
        // The first, a middle one (past the first batch) and the last, in the Map and the Set.
        for i in [0, 127, 128, 200, ENTRIES - 1] {
            match get(port, &format!("/at/ada/k{i:03}")) {
                Ok((200, body)) if body.contains(&i.to_string()) => {}
                other => failures.push(format!("Map entry {i} under its new name: {other:?}")),
            }
            match get(port, &format!("/has/ada/k{i:03}")) {
                Ok((200, body)) if body == "true" => {}
                other => failures.push(format!("Set member {i} under its new name: {other:?}")),
            }
        }
        // The index postings moved with the entries (an equality filter on the index reads them),
        // and so did the `Log`.
        for (who, expected) in [("ada", ENTRIES / 2), ("bob", ENTRIES / 2)] {
            match get(port, &format!("/orders/ada/{who}")) {
                Ok((200, body)) if body == expected.to_string() => {}
                other => failures.push(format!(
                    "orders of {who} through the moved index: {other:?}, expected {expected}"
                )),
            }
        }
        match get(port, "/logged/ada") {
            Ok((200, body)) if body == ENTRIES.to_string() => {}
            other => failures.push(format!("the moved log holds {other:?}, expected {ENTRIES}")),
        }
        // A write after the rename lands beside what moved.
        match get(port, "/put/ada/fresh/9/ada") {
            Ok((200, _)) => {}
            other => failures.push(format!("writing after the rename: {other:?}")),
        }
        match get(port, "/sizes/ada") {
            Ok((200, body)) if body == (2 * ENTRIES + 2).to_string() => {}
            other => failures.push(format!("after one more write: {other:?}")),
        }
        drop(server);
    }
    let first = fs::read_to_string(tmp.join("first.log")).unwrap_or_default();
    let second = fs::read_to_string(tmp.join("second.log")).unwrap_or_default();
    let _ = fs::remove_dir_all(&tmp);
    assert!(
        failures.is_empty(),
        "a renamed Map and Set on workerd:\n{}\n--- first wrangler log ---\n{first}\n--- second wrangler log ---\n{second}",
        failures.join("\n")
    );
}
