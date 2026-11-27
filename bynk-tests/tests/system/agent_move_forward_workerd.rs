//! #3565 (agent-context moves, slice 4b): a receiving group's phase 1 forwards a moving context's
//! calls to its old script unchanged, on a real `workerd`
//! (`design/tracks/agent-context-moves.md` §3.2).
//!
//! `shop.orders` (which hosts an agent) moves from its own Worker `shop-orders` into group
//! `core`. Three Workers run, joined through wrangler's dev registry:
//!
//! - `shop-orders` from the old build: the source, still serving the context;
//! - `core` from the new build, with its receive variant (`wrangler.receive.toml`), which runs
//!   none of `shop.orders`'s code and forwards its requests over `BYNK_FORWARD_SHOP_ORDERS`;
//! - `shop-web` from the new build: a caller already rebound to `core`.
//!
//! A signed `/_bynk/call` from the caller answering `200` with the callee's value is the check.
//! `shop.orders` verifies the caller's signature, so the forwarded request must reach it with its
//! body and signature unchanged, and the edge key must keep its value across the rebind.
//!
//! Skips loudly without the toolchain; a non-empty `BYNK_REQUIRE_WORKERD` (Linux CI sets `1`)
//! turns the skip into a failure.

use bynk_project::worker_moves::{Incoming, MoveVariants};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

#[path = "../require/mod.rs"]
mod require;
#[path = "../wrangler/mod.rs"]
mod wrangler;

const REQUIRE_ENV: &str = "BYNK_REQUIRE_WORKERD";
const WRANGLER: &str = wrangler::SPEC;

const ORDERS: &str = r#"context shop.orders

agent Counter {
  key id: String
  store n: Cell[Int] = 0

  on call current() -> Effect[Int] { Effect.pure(0) }
}

service answer {
  on call(x: Int) -> Effect[Int] by Caller {
    Effect.pure(x + 1)
  }
}
"#;

const PRICING: &str = r#"context shop.pricing

service papi from http {
  on GET("/price") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("1")
  }
}
"#;

const WEB: &str = r#"context shop.web

consumes shop.orders

service api from http {
  on GET("/ask") () -> Effect[HttpResult[String]] by v: Visitor {
    let n <- shop.orders.answer(41)
    match n == 42 {
      true => Ok("forwarded")
      false => Ok("wrong")
    }
  }
}
"#;

fn tool_exists(name: &str) -> bool {
    which::which(name).is_ok()
}

fn skip(reason: &str) -> bool {
    eprintln!("\n!!! AGENT MOVE FORWARD (WORKERD) SKIPPED !!!\n{reason}\n");
    if require::is_required(REQUIRE_ENV) {
        panic!("{REQUIRE_ENV} is set but {reason}");
    }
    true
}

/// Every `wrangler dev`, stopped together on every exit path.
struct Wranglers(Vec<wrangler::Wrangler>);

impl Drop for Wranglers {
    fn drop(&mut self) {
        wrangler::stop_all(&mut self.0);
    }
}

/// One `GET`, HTTP/1.1, connection-close: the status and the body.
fn get(port: u16, path: &str) -> Result<(u16, String), String> {
    let addr = format!("127.0.0.1:{port}");
    let mut stream = std::net::TcpStream::connect_timeout(
        &addr.parse().map_err(|e| format!("{e}"))?,
        Duration::from_secs(2),
    )
    .map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(20)))
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
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| format!("no status line: {:?}", response.lines().next()))?;
    let body = response
        .split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default();
    Ok((status, body))
}

fn read_log(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| format!("<could not read {path:?}: {e}>"))
}

/// The three contexts, built into `out` with `manifest` and `moves`.
fn build(root: &Path, out: &Path, manifest: &str, moves: MoveVariants) {
    let _ = fs::remove_dir_all(root);
    fs::create_dir_all(root.join("src/shop")).unwrap();
    fs::write(
        root.join("bynk.toml"),
        format!("[build]\ntarget = \"workers\"\n{manifest}"),
    )
    .unwrap();
    for (file, source) in [("orders", ORDERS), ("pricing", PRICING), ("web", WEB)] {
        fs::write(root.join(format!("src/shop/{file}.bynk")), source).unwrap();
    }
    let options = bynk_driver::try_project_options(root).expect("the project reads");
    let built = match bynk_driver::project::compile_project(
        &options
            .target(bynk_driver::project::BuildTarget::Workers)
            .moves(moves),
    ) {
        Ok(o) => o,
        Err(failure) => panic!(
            "compile the moving project:\n{}",
            bynk_driver::render::render_project_errors(&failure.flatten())
        ),
    };
    // TypeScript, as `bynk deploy` pushes it: wrangler bundles the entries, the variants' too.
    bynk_driver::write_output(&built, out).unwrap();
}

#[test]
fn a_receiving_group_forwards_a_moving_contexts_calls_to_its_old_script() {
    if !tool_exists("npx") && skip("`npx` is not on PATH") {
        return;
    }
    if !tool_exists("node") && skip("`node` is not on PATH") {
        return;
    }

    let tmp = std::env::temp_dir().join(format!("bynk-move-forward-{}", std::process::id()));
    let _ = fs::remove_dir_all(&tmp);
    let (old, new): (PathBuf, PathBuf) = (tmp.join("old"), tmp.join("new"));
    build(&tmp.join("old-src"), &old, "", MoveVariants::default());
    build(
        &tmp.join("new-src"),
        &new,
        "\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pricing\"]\n",
        MoveVariants {
            receive: [(
                "core".to_string(),
                vec![Incoming {
                    context: "shop.orders".into(),
                    classes: vec!["Counter".into()],
                    from_script: "shop-orders".into(),
                }],
            )]
            .into(),
            ..MoveVariants::default()
        },
    );
    let source = old.join("workers/shop-orders");
    let receiver = new.join("workers/core");
    let caller = new.join("workers/shop-web");
    for (dir, config) in [
        (&source, "wrangler.toml"),
        (&receiver, "wrangler.receive.toml"),
        (&caller, "wrangler.toml"),
    ] {
        assert!(dir.join(config).is_file(), "expected {dir:?}/{config}");
    }

    // The caller is rebound to the receiver, so its call can only reach `shop.orders` through
    // the forward: the receiver's variant runs none of `shop.orders`'s code.
    let bound = fs::read_to_string(caller.join("wrangler.toml")).unwrap();
    assert!(bound.contains("service = \"core\""), "{bound}");
    assert!(!bound.contains("service = \"shop-orders\""), "{bound}");
    let receive = fs::read_to_string(receiver.join("wrangler.receive.toml")).unwrap();
    assert!(
        receive.contains("binding = \"BYNK_FORWARD_SHOP_ORDERS\"\nservice = \"shop-orders\""),
        "{receive}"
    );

    let [p0, p1, p2, i0, i1, i2] = wrangler::ports::free_ports();
    let (ports, inspectors) = ([p0, p1, p2], [i0, i1, i2]);
    let names = ["source", "receiver", "caller"];
    let runs = [
        (&source, "wrangler.toml"),
        (&receiver, "wrangler.receive.toml"),
        (&caller, "wrangler.toml"),
    ];
    let log_paths: Vec<PathBuf> = names
        .iter()
        .map(|n| tmp.join(format!("{n}-wrangler.log")))
        .collect();
    let spawn = |i: usize| -> std::io::Result<wrangler::Wrangler> {
        let out_log = fs::File::create(&log_paths[i])?;
        let err_log = out_log.try_clone()?;
        wrangler::Wrangler::spawn(
            wrangler::npx()
                .args(["dev", "--config", runs[i].1])
                // The caller signs under its edge's key, and the callee verifies it: the
                // rebind keeps the key's value.
                .args(wrangler::local_edge_vars(&[("shop.web", "shop.orders")]))
                .args([
                    "--port",
                    &ports[i].to_string(),
                    "--inspector-port",
                    &inspectors[i].to_string(),
                ])
                .current_dir(runs[i].0)
                .stdout(Stdio::from(out_log))
                .stderr(Stdio::from(err_log)),
            &tmp.join("registry"),
        )
    };
    let spawned: Vec<_> = (0..3).map(spawn).collect();
    if spawned.iter().any(|s| s.is_err()) {
        drop(Wranglers(spawned.into_iter().flatten().collect()));
        if skip("could not launch npx for one or more workers") {
            return;
        }
        unreachable!()
    }
    let wranglers = Wranglers(spawned.into_iter().flatten().collect());
    let logs = || {
        let mut text = String::new();
        for (n, p) in names.iter().zip(&log_paths) {
            text.push_str(&format!("--- {n} log ---\n{}\n", read_log(p)));
        }
        text.push_str(&format!(
            "--- dev registry ---\n{}",
            wrangler::registry_listing(&tmp.join("registry"))
        ));
        text
    };

    // The caller's call answers once all three are up and both Service Bindings have connected,
    // so it is the readiness probe and the check at once.
    let answered = std::cell::RefCell::new(None);
    if let Err(why) = wrangler::wait_until_all_ready(
        &[(&log_paths[0], ports[0]), (&log_paths[1], ports[1])],
        (&log_paths[2], ports[2]),
        &|| match get(ports[2], "/ask") {
            Ok((200, body)) => {
                *answered.borrow_mut() = Some(body);
                true
            }
            _ => false,
        },
    ) {
        let logs = logs();
        drop(wranglers);
        let _ = fs::remove_dir_all(&tmp);
        if skip(&format!(
            "{why} (likely no network to provision {WRANGLER})\n{logs}"
        )) {
            return;
        }
        unreachable!()
    }
    assert_eq!(
        answered.into_inner().expect("the call answered"),
        "\"forwarded\"",
        "`shop.web` → `core` (forwarding) → `shop.orders` on `shop-orders`, signature intact\n{}",
        logs()
    );
    drop(wranglers);
    let _ = fs::remove_dir_all(&tmp);
}
