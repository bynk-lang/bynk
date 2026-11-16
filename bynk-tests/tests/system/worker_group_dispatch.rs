//! A `[deploy.workers.<key>]` group is one Worker (#3562, G2b step 2c-4b).
//!
//! Two things are pinned. The first needs only `bynk build`: which files belong
//! to the Worker (its `index.ts`, `wrangler.toml`) and which to each member
//! (`<member>.index.ts`, `.compose.ts`, `.handlers.ts`, the manifests), and that
//! the one `wrangler.toml` merges the members' bindings, crons and classes. The
//! second runs the built group's `fetch` under Node: the dispatcher tries members
//! in order, treats only a router's own miss as a miss, and answers a request no
//! member claims as a standalone Worker would.
//!
//! The run needs `tsc` and `node`; each skips loudly without it, and
//! `BYNK_REQUIRE_TSC=1` (CI) turns the skip into a failure.

#[path = "../require/mod.rs"]
mod require;
#[path = "../tools/mod.rs"]
mod tools;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const REQUIRE_ENV: &str = "BYNK_REQUIRE_TSC";

const MANIFEST: &str = "[unstable]\nallow = [\"bynk.cloudflare.R2\"]\n\n[build]\ntarget = \"workers\"\n\n[deploy.workers.core]\ncontexts = [\"shop.orders\", \"shop.pricing\"]\n";
const UNGROUPED: &str =
    "[unstable]\nallow = [\"bynk.cloudflare.R2\"]\n\n[build]\ntarget = \"workers\"\n";

/// `shop.orders`: owns `GET /orders` and a route that answers `404` itself, an
/// agent, a cron and the same R2 capability as `shop.pricing`.
const ORDERS: &str = r#"context shop.orders

consumes bynk.cloudflare { R2 }

capability Archives = R2

agent Cart {
	key id: String
	store n: Cell[Int] = 0

	on call bump() -> Effect[Int] { Effect.pure(1) }
}

service api from http {
  on GET("/orders") () -> Effect[HttpResult[String]] by v: Visitor {
    Ok("orders")
  }
  on GET("/gone") () -> Effect[HttpResult[String]] by v: Visitor {
    NotFound
  }
}

service tick from cron {
  on schedule("0 * * * *") () -> Effect[Result[(), String]] {
    Effect.pure(Ok(()))
  }
}

service order_count {
  on call(n: Int) -> Effect[Int] {
    Effect.pure(n)
  }
}
"#;

/// `shop.pricing`: owns `POST /orders` (the same path as `shop.orders`' `GET`),
/// echoes its body, and has its own cron.
const PRICING: &str = r#"context shop.pricing

consumes bynk.cloudflare { R2 }

capability Archives = R2

service papi from http {
  on POST("/orders") (body: String) -> Effect[HttpResult[String]] by v: Visitor {
    Ok(body)
  }
}

service sweep from cron {
  on schedule("30 * * * *") () -> Effect[Result[(), String]] {
    Effect.pure(Ok(()))
  }
}

service price {
  on call(n: Int) -> Effect[Int] {
    Effect.pure(n)
  }
}
"#;

fn stage(tag: &str, manifest: &str) -> PathBuf {
    let root =
        std::env::temp_dir().join(format!("bynk-group-dispatch-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("src/shop")).unwrap();
    fs::write(root.join("bynk.toml"), manifest).unwrap();
    fs::write(root.join("src/shop/orders.bynk"), ORDERS).unwrap();
    fs::write(root.join("src/shop/pricing.bynk"), PRICING).unwrap();
    root
}

fn text(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn build(root: &Path) {
    let out = Command::new(env!("CARGO_BIN_EXE_bynk-under-test"))
        .args(["build", "."])
        .current_dir(root)
        .env("NO_COLOR", "1")
        .output()
        .expect("bynk runs");
    assert!(out.status.success(), "bynk build failed:\n{}", text(&out));
}

fn read(root: &Path, rel: &str) -> String {
    fs::read_to_string(root.join("out").join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

fn exists(root: &Path, rel: &str) -> bool {
    root.join("out").join(rel).is_file()
}

/// Which files belong to the Worker and which to each member.
#[test]
fn a_group_is_one_worker_whose_members_keep_their_own_files() {
    let root = stage("files", MANIFEST);
    build(&root);

    // The Worker's: one entry, one `wrangler.toml`, in the group's directory.
    for file in ["workers/core/index.ts", "workers/core/wrangler.toml"] {
        assert!(exists(&root, file), "{file} is the Worker's");
    }
    // Each member's: its entry, composition and handlers, under its prefix.
    for member in ["shop-orders", "shop-pricing"] {
        for file in ["index.ts", "compose.ts", "handlers.ts"] {
            let path = format!("workers/core/{member}.{file}");
            assert!(exists(&root, &path), "{path} is the member's");
        }
    }
    // Nothing is left at a context's own Worker directory.
    for member in ["shop-orders", "shop-pricing"] {
        assert!(
            !exists(&root, &format!("workers/{member}/index.ts")),
            "{member} has no Worker of its own"
        );
    }

    // The one `wrangler.toml` is named for the group and merges its members.
    let toml = read(&root, "workers/core/wrangler.toml");
    assert!(toml.contains("name = \"core\""), "{toml}");
    assert!(toml.contains("main = \"index.ts\""), "{toml}");
    assert_eq!(
        toml.matches("[[r2_buckets]]").count(),
        1,
        "an identical capability is one binding:\n{toml}"
    );
    assert!(
        toml.contains("0 * * * *") && toml.contains("30 * * * *"),
        "{toml}"
    );
    assert!(toml.contains("class_name = \"Cart\""), "{toml}");

    // The group's entry imports each member's, in order, and exports the
    // classes by name.
    let index = read(&root, "workers/core/index.ts");
    let first = index.find("./shop-orders.index.js").expect("first member");
    let second = index
        .find("./shop-pricing.index.js")
        .expect("second member");
    assert!(
        first < second,
        "members are tried in declaration order:\n{index}"
    );
    assert!(
        index.contains("export { Cart } from \"./shop-orders.index.js\";"),
        "{index}"
    );
    assert!(!index.contains("export *"), "{index}");

    // A member's entry marks its misses; nothing about that is a header.
    let member = read(&root, "workers/core/shop-orders.index.ts");
    assert!(member.contains("export function isMiss"), "{member}");
    assert!(!member.contains("x-bynk-miss"), "{member}");
    let _ = fs::remove_dir_all(&root);
}

/// Without a group the same project is two Workers and no entry marks anything.
#[test]
fn without_a_group_each_context_keeps_its_own_worker_and_marks_nothing() {
    let root = stage("ungrouped", UNGROUPED);
    build(&root);
    for member in ["shop-orders", "shop-pricing"] {
        let index = read(&root, &format!("workers/{member}/index.ts"));
        assert!(
            !index.contains("isMiss") && !index.contains("__miss"),
            "{index}"
        );
        assert!(exists(&root, &format!("workers/{member}/wrangler.toml")));
    }
    assert!(!exists(&root, "workers/core/index.ts"));
    let _ = fs::remove_dir_all(&root);
}

const SCRIPT: &str = r#"import worker from "./out-js/workers/core/index.js";
const ctx = { waitUntil() {} };
const results = [];
const show = async (label, request) => {
  const r = await worker.fetch(request, {}, ctx);
  results.push({
    label,
    status: r.status,
    body: await r.text(),
    allow: r.headers.get("allow"),
    nosniff: r.headers.get("x-content-type-options"),
    headers: [...r.headers.keys()],
  });
};
const u = (p) => "http://x" + p;
await show("get-orders", new Request(u("/orders")));
await show("post-orders", new Request(u("/orders"), { method: "POST", body: JSON.stringify("hello"), headers: { "content-type": "application/json" } }));
await show("owned-404", new Request(u("/gone")));
await show("delete-orders", new Request(u("/orders"), { method: "DELETE" }));
await show("options-orders", new Request(u("/orders"), { method: "OPTIONS" }));
await show("nothing", new Request(u("/nope")));
const big = "x".repeat(2_000_000);
for (const [name, method, body] of [["post", "POST", "[1]"], ["get", "GET", undefined], ["big", "POST", big]]) {
  await show(`call-known-${name}`, new Request(u("/_bynk/call/price"), { method, body }));
  await show(`call-unknown-${name}`, new Request(u("/_bynk/call/nothing"), { method, body }));
}
await show("event-unsigned", new Request(u("/_bynk/event/nothing"), { method: "POST", body: "{}" }));
console.log(JSON.stringify(results));
"#;

/// `bynk build`, `tsc`, then run the group's `fetch` under Node. `None` when
/// the test skips.
fn run_group(root: &Path) -> Option<Vec<serde_json::Value>> {
    let Some(mut tsc) = tools::tsc_command() else {
        tools::skip(
            "WORKER GROUP DISPATCH (tsc)",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "no TypeScript toolchain (`tsc` or `npx`) is on PATH",
        );
        return None;
    };
    if !tools::tool_exists("node") {
        tools::skip(
            "WORKER GROUP DISPATCH (node)",
            require::is_required(REQUIRE_ENV),
            REQUIRE_ENV,
            "`node` is not on PATH",
        );
        return None;
    }
    build(root);
    let emitted = tsc
        .arg("-p")
        .arg(root.join("out/tsconfig.json"))
        .current_dir(root)
        .output()
        .expect("run tsc");
    assert!(emitted.status.success(), "tsc failed:\n{}", text(&emitted));
    fs::write(root.join("out-js/package.json"), "{\"type\":\"module\"}").unwrap();
    fs::write(root.join("run.mjs"), SCRIPT).unwrap();
    let ran = Command::new("node")
        .arg("run.mjs")
        .current_dir(root)
        .output()
        .expect("run node");
    assert!(ran.status.success(), "node failed:\n{}", text(&ran));
    let line = String::from_utf8_lossy(&ran.stdout).trim().to_string();
    Some(serde_json::from_str(&line).unwrap_or_else(|e| panic!("{e}: {line}")))
}

fn find<'a>(results: &'a [serde_json::Value], label: &str) -> &'a serde_json::Value {
    results
        .iter()
        .find(|r| r["label"] == label)
        .unwrap_or_else(|| panic!("no result `{label}`"))
}

#[test]
fn a_groups_fetch_dispatches_to_the_member_that_owns_the_request() {
    let root = stage("dispatch", MANIFEST);
    let Some(results) = run_group(&root) else {
        let _ = fs::remove_dir_all(&root);
        return;
    };

    // The first member owns `GET /orders`.
    let get = find(&results, "get-orders");
    assert_eq!(
        (get["status"].as_u64(), get["body"].as_str()),
        (Some(200), Some("\"orders\""))
    );

    // `POST /orders` is a miss in the first member (it has only the `GET`) and
    // the second reads the body, which the first left alone. Not the first
    // member's `405`.
    let post = find(&results, "post-orders");
    assert_eq!(
        (post["status"].as_u64(), post["body"].as_str()),
        (Some(200), Some("\"hello\"")),
        "the second member answered with the body intact: {post}"
    );

    // A handler's own `404` for a route it owns is an answer, not a miss: it
    // carries the service's headers, which the group's own `404` does not.
    let owned = find(&results, "owned-404");
    assert_eq!(owned["status"], 404);
    assert_eq!(owned["nosniff"], "nosniff", "{owned}");

    // A method no member handles on a path members serve is a `405` whose
    // `Allow` lists what the whole Worker serves there: `GET` from the first
    // member and `POST` from the second, not one member's alone.
    let delete = find(&results, "delete-orders");
    assert_eq!(delete["status"], 405);
    assert_eq!(delete["allow"], "GET, HEAD, OPTIONS, POST", "{delete}");
    let options = find(&results, "options-orders");
    assert_eq!(options["status"], 204);
    assert_eq!(options["allow"], "GET, HEAD, OPTIONS, POST", "{options}");

    // No member owns the request: the single-context `Not Found`.
    let nothing = find(&results, "nothing");
    assert_eq!(
        (nothing["status"].as_u64(), nothing["body"].as_str()),
        (Some(404), Some("Not Found"))
    );

    // The marker is never visible: no response carries a header naming it.
    for r in &results {
        let names = r["headers"].as_array().unwrap();
        assert!(
            names.iter().all(|n| !n.as_str().unwrap().contains("miss")),
            "{r}"
        );
    }
    let _ = fs::remove_dir_all(&root);
}

/// An unsigned request answers the same whether or not the service exists, so a
/// caller without credentials cannot list a group's services.
#[test]
fn an_unsigned_internal_request_answers_the_same_for_a_known_and_an_unknown_service() {
    let root = stage("parity", MANIFEST);
    let Some(results) = run_group(&root) else {
        let _ = fs::remove_dir_all(&root);
        return;
    };
    for name in ["post", "get", "big"] {
        let known = find(&results, &format!("call-known-{name}"));
        let unknown = find(&results, &format!("call-unknown-{name}"));
        assert_eq!(
            known["status"], unknown["status"],
            "{name}: {known} / {unknown}"
        );
        assert_eq!(
            known["body"], unknown["body"],
            "{name}: {known} / {unknown}"
        );
        assert_eq!(known["allow"], unknown["allow"], "{name}");
    }
    assert_eq!(find(&results, "call-known-post")["status"], 401);
    assert_eq!(find(&results, "call-known-get")["status"], 405);
    assert_eq!(find(&results, "call-known-big")["status"], 413);
    // No member subscribes to events, so a group answers the prefix as a
    // standalone Worker without subscribers does.
    assert_eq!(find(&results, "event-unsigned")["status"], 404);
    let _ = fs::remove_dir_all(&root);
}
